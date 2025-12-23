use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{self};
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use ratatui::prelude::*;
use ratatui::style::{Color, Style};
use ratatui::widgets::Paragraph;
use std::io::{self, Stdout};
use std::time::{Duration, Instant};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy)]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

pub enum UiEvent {
    Submit(String),
    Exit,
    None,
}

struct ChatMessage {
    role: MessageRole,
    content: String,
}

#[derive(Default)]
struct InputBuffer {
    graphemes: Vec<String>,
    cursor: usize,
    preferred_col: Option<usize>,
}

impl InputBuffer {
    fn is_empty(&self) -> bool {
        self.graphemes.is_empty()
    }

    fn to_string(&self) -> String {
        self.graphemes.concat()
    }

    fn clear(&mut self) {
        self.graphemes.clear();
        self.cursor = 0;
        self.preferred_col = None;
    }

    fn insert_str(&mut self, s: &str) {
        for g in s.graphemes(true) {
            self.graphemes.insert(self.cursor, g.to_string());
            self.cursor += 1;
        }
        self.preferred_col = None;
    }

    fn backspace(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.cursor -= 1;
        self.graphemes.remove(self.cursor);
        self.preferred_col = None;
        true
    }

    fn delete(&mut self) -> bool {
        if self.cursor >= self.graphemes.len() {
            return false;
        }
        self.graphemes.remove(self.cursor);
        self.preferred_col = None;
        true
    }

    fn move_left(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.cursor -= 1;
        self.preferred_col = None;
        true
    }

    fn move_right(&mut self) -> bool {
        if self.cursor >= self.graphemes.len() {
            return false;
        }
        self.cursor += 1;
        self.preferred_col = None;
        true
    }

    fn layout_positions(&self, term_width: usize) -> Vec<(usize, usize)> {
        let width = term_width.max(1);
        let mut positions = Vec::with_capacity(self.graphemes.len() + 1);
        let mut row = 0usize;
        let mut col = 0usize;
        positions.push((row, col));

        for g in &self.graphemes {
            if g == "\n" {
                row += 1;
                col = 0;
                positions.push((row, col));
                continue;
            }

            let w = UnicodeWidthStr::width(g.as_str()).max(1);
            if col + w > width {
                row += 1;
                col = 0;
            }
            col += w;
            positions.push((row, col));
        }

        positions
    }

    fn move_vertical(&mut self, delta_row: isize, term_width: usize) -> bool {
        let positions = self.layout_positions(term_width);
        let (current_row, current_col) = positions
            .get(self.cursor)
            .copied()
            .unwrap_or((0, 0));
        let target_col = self.preferred_col.unwrap_or(current_col);
        let target_row = current_row as isize + delta_row;
        if target_row < 0 {
            return false;
        }
        let target_row = target_row as usize;

        let mut best: Option<(usize, usize)> = None;
        for (idx, &(row, col)) in positions.iter().enumerate() {
            if row != target_row {
                continue;
            }
            let dist = if col > target_col { col - target_col } else { target_col - col };
            match best {
                Some((_, best_dist)) if dist >= best_dist => {}
                _ => best = Some((idx, dist)),
            }
        }

        if let Some((idx, _)) = best {
            self.cursor = idx;
            self.preferred_col = Some(target_col);
            true
        } else {
            false
        }
    }

    fn wrapped_lines(&self, term_width: usize) -> Vec<String> {
        let width = term_width.max(1);
        let mut lines = vec![String::new()];
        let mut col = 0usize;

        for g in &self.graphemes {
            if g == "\n" {
                lines.push(String::new());
                col = 0;
                continue;
            }

            let w = UnicodeWidthStr::width(g.as_str()).max(1);
            if col + w > width {
                lines.push(String::new());
                col = 0;
            }
            lines.last_mut().unwrap().push_str(g);
            col += w;
        }

        lines
    }

    fn cursor_position(&self, term_width: usize) -> (usize, usize) {
        let positions = self.layout_positions(term_width);
        positions.get(self.cursor).copied().unwrap_or((0, 0))
    }
}

pub struct ChatUi {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    messages: Vec<ChatMessage>,
    status: String,
    input: InputBuffer,
    last_ctrl_c: Option<Instant>,
    input_width: usize,
    chat_height: usize,
    banner: Vec<Line<'static>>,
}

impl ChatUi {
    pub fn new() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnableBracketedPaste)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;

        Ok(Self {
            terminal,
            messages: Vec::new(),
            status: "Ready".to_string(),
            input: InputBuffer::default(),
            last_ctrl_c: None,
            input_width: 0,
            chat_height: 0,
            banner: build_rkllm_banner(),
        })
    }

    pub fn shutdown(&mut self) -> Result<()> {
        terminal::disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), DisableBracketedPaste)?;
        Ok(())
    }

    pub fn add_message(&mut self, role: MessageRole, content: impl Into<String>) {
        self.messages.push(ChatMessage {
            role,
            content: content.into(),
        });
    }

    pub fn add_notice(&mut self, content: impl Into<String>) {
        self.add_message(MessageRole::System, content);
    }

    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    pub fn draw(&mut self) -> Result<()> {
        let messages = &self.messages;
        let status = self.status.clone();
        let input = &self.input;

        self.terminal.draw(|f| {
            let size = f.area();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(5),
                    Constraint::Length(1),
                    Constraint::Length(5),
                    Constraint::Length(1),
                ])
                .split(size);

            let chat_inner = chunks[0];
            let mut chat_lines = self.banner.clone();
            if !chat_lines.is_empty() {
                chat_lines.push(Line::from(""));
            }
            chat_lines.extend(build_chat_lines(messages, chat_inner.width as usize));
            self.chat_height = chat_inner.height as usize;
            let chat = Paragraph::new(chat_lines);
            f.render_widget(chat, chunks[0]);

            let separator = "─".repeat(size.width as usize);
            let separator_widget = Paragraph::new(separator);
            f.render_widget(separator_widget, chunks[1]);

            let input_inner = chunks[2];
            self.input_width = input_inner.width as usize;
            let (input_lines, cursor) = build_input_lines(input, input_inner.width as usize, input_inner.height as usize);
            let input_widget = Paragraph::new(input_lines);
            f.render_widget(input_widget, chunks[2]);
            if let Some((cursor_row, cursor_col)) = cursor {
                let x = input_inner.x + cursor_col as u16;
                let y = input_inner.y + cursor_row as u16;
                f.set_cursor_position((x, y));
            }

            let status_widget = Paragraph::new(status);
            f.render_widget(status_widget, chunks[3]);
        })?;

        Ok(())
    }

    pub fn next_event(&mut self, timeout: Duration) -> Result<UiEvent> {
        if !event::poll(timeout)? {
            return Ok(UiEvent::None);
        }

        match event::read()? {
            Event::Key(key_event) => Ok(self.handle_key_event(key_event)),
            Event::Paste(content) => {
                let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
                self.input.insert_str(&normalized);
                Ok(UiEvent::None)
            }
            _ => Ok(UiEvent::None),
        }
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) -> UiEvent {
        match key_event {
            KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                let now = Instant::now();
                if let Some(last) = self.last_ctrl_c {
                    if now.duration_since(last).as_secs() < 2 {
                        return UiEvent::Exit;
                    }
                }
                self.last_ctrl_c = Some(now);
                self.status = "Press Ctrl+C again to exit.".to_string();
                UiEvent::None
            }
            KeyEvent {
                code: KeyCode::Char('d'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => UiEvent::Exit,
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                let text = self.input.to_string();
                self.input.clear();
                UiEvent::Submit(text)
            }
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::SHIFT,
                ..
            } => {
                self.input.insert_str("\n");
                UiEvent::None
            }
            KeyEvent {
                code: KeyCode::Char('j'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.insert_str("\n");
                UiEvent::None
            }
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            } => {
                self.input.backspace();
                UiEvent::None
            }
            KeyEvent {
                code: KeyCode::Delete,
                ..
            } => {
                self.input.delete();
                UiEvent::None
            }
            KeyEvent {
                code: KeyCode::Left,
                ..
            } => {
                self.input.move_left();
                UiEvent::None
            }
            KeyEvent {
                code: KeyCode::Right,
                ..
            } => {
                self.input.move_right();
                UiEvent::None
            }
            KeyEvent {
                code: KeyCode::Up,
                ..
            } => {
                let width = self.input_width.max(1);
                self.input.move_vertical(-1, width);
                UiEvent::None
            }
            KeyEvent {
                code: KeyCode::Down,
                ..
            } => {
                let width = self.input_width.max(1);
                self.input.move_vertical(1, width);
                UiEvent::None
            }
            KeyEvent {
                code: KeyCode::Char(c),
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
                ..
            } => {
                self.input.insert_str(&c.to_string());
                UiEvent::None
            }
            _ => UiEvent::None,
        }
    }


    pub fn confirm(&mut self, prompt: &str) -> Result<bool> {
        let previous_status = self.status.clone();
        self.status = format!("{} (y/N)", prompt);
        self.draw()?;
        loop {
            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(key_event) = event::read()? {
                    match key_event.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            self.status = previous_status;
                            return Ok(true);
                        }
                        KeyCode::Char('n')
                        | KeyCode::Char('N')
                        | KeyCode::Esc
                        | KeyCode::Enter => {
                            self.status = previous_status;
                            return Ok(false);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

fn build_chat_lines(messages: &[ChatMessage], width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let usable_width = width.max(1);

    for message in messages {
        let prefix = match message.role {
            MessageRole::User => "User: ",
            MessageRole::Assistant => "Assistant: ",
            MessageRole::System => "System: ",
        };
        let mut combined = String::new();
        combined.push_str(prefix);
        combined.push_str(&message.content);

        for line in wrap_text(&combined, usable_width) {
            lines.push(Line::from(line));
        }
        lines.push(Line::from(""));
    }

    lines
}

fn build_rkllm_banner() -> Vec<Line<'static>> {
    let red = Style::default().fg(Color::Red);
    let yellow = Style::default().fg(Color::Yellow);
    let green = Style::default().fg(Color::Green);
    let cyan = Style::default().fg(Color::Cyan);
    let gray = Style::default().fg(Color::DarkGray);

    vec![
        Line::from(vec![
            Span::styled("███████", red),
            Span::styled(" ", gray),
            Span::styled("██  ██", yellow),
            Span::styled("  ", gray),
            Span::styled("██      ", green),
            Span::styled("██      ", green),
            Span::styled("██   ██", cyan),
        ]),
        Line::from(vec![
            Span::styled("██   ██", red),
            Span::styled(" ", gray),
            Span::styled("██ ██", yellow),
            Span::styled("   ", gray),
            Span::styled("██      ", green),
            Span::styled("██      ", green),
            Span::styled("███ ███", cyan),
        ]),
        Line::from(vec![
            Span::styled("██████", red),
            Span::styled("  ", gray),
            Span::styled("████", yellow),
            Span::styled("    ", gray),
            Span::styled("██      ", green),
            Span::styled("██      ", green),
            Span::styled("███████", cyan),
        ]),
        Line::from(vec![
            Span::styled("██  ██", red),
            Span::styled("  ", gray),
            Span::styled("██ ██", yellow),
            Span::styled("   ", gray),
            Span::styled("██      ", green),
            Span::styled("██      ", green),
            Span::styled("██   ██", cyan),
        ]),
        Line::from(vec![
            Span::styled("██   ██", red),
            Span::styled(" ", gray),
            Span::styled("██  ██", yellow),
            Span::styled("  ", gray),
            Span::styled("███████ ", green),
            Span::styled("███████ ", green),
            Span::styled("██   ██", cyan),
        ]),
        Line::from(vec![Span::styled("Rockchip NPU Agentic CLI", gray)]),
    ]
}

fn build_input_lines(
    input: &InputBuffer,
    width: usize,
    height: usize,
) -> (Vec<Line<'static>>, Option<(usize, usize)>) {
    if height == 0 {
        return (Vec::new(), None);
    }
    let usable_width = width.max(1);
    let raw_lines = input.wrapped_lines(usable_width);
    let (cursor_row, cursor_col) = input.cursor_position(usable_width);

    let mut start_line = 0usize;
    if cursor_row >= height {
        start_line = cursor_row + 1 - height;
    }
    if raw_lines.len().saturating_sub(start_line) < height {
        start_line = raw_lines.len().saturating_sub(height);
    }

    let visible_lines: Vec<Line<'static>> = raw_lines
        .iter()
        .skip(start_line)
        .take(height.max(1))
        .map(|line| Line::from(line.clone()))
        .collect();

    let visible_cursor_row = cursor_row.saturating_sub(start_line);
    let cursor = if visible_cursor_row < height {
        Some((visible_cursor_row, cursor_col))
    } else {
        None
    };

    (visible_lines, cursor)
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut col = 0usize;
    let width = width.max(1);

    for g in text.graphemes(true) {
        if g == "\n" {
            lines.push(std::mem::take(&mut current));
            col = 0;
            continue;
        }
        let w = UnicodeWidthStr::width(g).max(1);
        if col + w > width {
            lines.push(std::mem::take(&mut current));
            col = 0;
        }
        current.push_str(g);
        col += w;
    }

    lines.push(current);
    lines
}
