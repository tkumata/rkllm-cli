use crate::config::AppConfig;
use crate::file_detector;
use crate::file_ops;
use crate::file_output_parser;
use crate::llm::{RKLLMConfig, RKLLM};
use crate::mcp::{McpClient, McpConfig};
use crate::mcp::types::{Tool, ToolCall, ToolResult};
use crate::intent::{has_file_operation_intent, has_file_read_intent, prefers_output_only};
use crate::prompt_builder::build_chat_prompt;
use crate::tool_detector::ToolCallDetector;
use anyhow::{Context, Result};
use crossterm::{
    cursor,
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal,
};
use once_cell::sync::OnceCell;
use regex::Regex;
use serde_json::{self, json};
use std::collections::HashSet;
use std::cmp::Reverse;
use std::io::{self, stdout, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub struct ChatSession {
    rkllm: RKLLM,
    mcp_client: Option<McpClient>,
    tool_detector: ToolCallDetector,
    last_ctrl_c: Arc<Mutex<Option<Instant>>>,
    preview_prompt: bool,
    confirm_writes: bool,
    tool_only: bool,
    config: AppConfig,
    execution_dir: String,
}

#[derive(Copy, Clone)]
enum ToolCallAllowance {
    All,
    WriteOnly,
}

#[derive(Default)]
struct InputBuffer {
    graphemes: Vec<String>,
    cursor: usize,
    // 垂直移動時に保持したい表示上の列
    preferred_col: Option<usize>,
}

impl InputBuffer {
    fn to_string(&self) -> String {
        self.graphemes.concat()
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

    fn layout_positions(
        &self,
        prompt_width: usize,
        indent_width: usize,
        term_width: usize,
    ) -> Vec<(usize, usize)> {
        let mut positions = Vec::with_capacity(self.graphemes.len() + 1);
        let mut row = 0usize;
        let mut col = prompt_width;
        positions.push((row, col));

        for g in &self.graphemes {
            if g == "\n" {
                row += 1;
                col = indent_width;
                positions.push((row, col));
                continue;
            }

            let w = UnicodeWidthStr::width(g.as_str()).max(1);
            if col + w > term_width {
                row += 1;
                col = indent_width;
            }
            col += w;
            positions.push((row, col));
        }

        positions
    }

    fn move_vertical(
        &mut self,
        delta_row: isize,
        prompt_width: usize,
        indent_width: usize,
        term_width: usize,
    ) -> bool {
        let positions = self.layout_positions(prompt_width, indent_width, term_width);
        let (current_row, current_col) = positions
            .get(self.cursor)
            .copied()
            .unwrap_or((0, prompt_width));
        let target_col = self.preferred_col.unwrap_or(current_col);
        let target_row = current_row as isize + delta_row;
        if target_row < 0 {
            return false;
        }
        let target_row_usize = target_row as usize;

        let mut best: Option<(usize, usize)> = None; // (idx, distance)
        for (idx, &(row, col)) in positions.iter().enumerate() {
            if row != target_row_usize {
                continue;
            }
            let dist = if col > target_col { col - target_col } else { target_col - col };
            match best {
                Some((_, best_dist)) if dist >= best_dist => {}
                _ => {
                    best = Some((idx, dist));
                }
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
}

impl ChatSession {
    const PROMPT: &'static str = "❯ ";
    const INDENT: &'static str = "  ";
    const PROMPT_BG: Color = Color::Rgb { r: 58, g: 58, b: 58 };
    const PROMPT_FG: Color = Color::White;
    const INPUT_BG: Color = Color::Rgb { r: 58, g: 58, b: 58 };
    const INPUT_FG: Color = Color::White;

    pub async fn new(
        model_path: String,
        mcp_config_path: Option<PathBuf>,
        preview_prompt: bool,
        confirm_writes: bool,
        tool_only: bool,
    ) -> Result<Self> {
        let config = RKLLMConfig {
            model_path,
            ..Default::default()
        };

        let rkllm = RKLLM::new(config).context("Failed to initialize RKLLM")?;

        // Initialize MCP client if config file is provided
        let mcp_client = if let Some(config_path) = mcp_config_path {
            if config_path.exists() {
                println!("Loading MCP configuration from: {}", config_path.display());
                match McpConfig::load(&config_path) {
                    Ok(mcp_config) => {
                        if !mcp_config.is_empty() {
                            match McpClient::new(mcp_config).await {
                                Ok(client) => Some(client),
                                Err(e) => {
                                    eprintln!("Failed to initialize MCP client: {}", e);
                                    None
                                }
                            }
                        } else {
                            println!("[MCP: Configuration file is empty]");
                            None
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to load MCP configuration: {}", e);
                        None
                    }
                }
            } else {
                eprintln!("MCP configuration file not found: {}", config_path.display());
                None
            }
        } else {
            None
        };

        let execution_dir = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .to_string_lossy()
            .to_string();

        let session = Self {
            rkllm,
            mcp_client,
            tool_detector: ToolCallDetector::new(),
            last_ctrl_c: Arc::new(Mutex::new(None)),
            preview_prompt,
            confirm_writes,
            tool_only,
            config: AppConfig::load(),
            execution_dir,
        };

        if session.tool_only && session.mcp_client.is_none() {
            anyhow::bail!("--tool-only requires MCP tools. Please provide a valid --mcp-config.");
        }

        Ok(session)
    }

    pub async fn start(&self) -> Result<()> {
        unsafe {
            std::env::set_var("RKLLM_TUI", "1");
        }
        self.print_banner();

        terminal::enable_raw_mode().context("Failed to enable raw mode")?;
        let mut stdout = stdout();
        execute!(stdout, EnableBracketedPaste).context("Failed to enable bracketed paste")?;

        let result = self.run_chat_loop(&mut stdout).await;

        execute!(stdout, DisableBracketedPaste).context("Failed to disable bracketed paste")?;
        terminal::disable_raw_mode().context("Failed to disable raw mode")?;
        println!();

        result
    }

    async fn run_chat_loop(&self, stdout: &mut std::io::Stdout) -> Result<()> {
        loop {
            self.print_status_line(stdout, "Ready")?;

            let input = match self.read_multiline_input(stdout)? {
                Some(text) => text,
                None => break,
            };

            let trimmed = input.trim();

            if trimmed.is_empty() {
                continue;
            }

            if trimmed.eq_ignore_ascii_case("exit") || trimmed.eq_ignore_ascii_case("quit") {
                execute!(stdout, Print("\r\nSee you!\r\n"))?;
                break;
            }

            if trimmed.eq_ignore_ascii_case("tools") {
                self.show_tools_command(stdout)?;
                continue;
            }

            if trimmed.eq_ignore_ascii_case("help") {
                self.show_help_command(stdout)?;
                continue;
            }

            let has_file_write_intent = has_file_operation_intent(&trimmed);
            let has_file_read_intent = has_file_read_intent(&trimmed);
            if self.tool_only && has_file_write_intent {
                println!("\n[tool-only] Local file writes are disabled. Use MCP tools for any file outputs.");
            }

            let file_paths = if (has_file_write_intent || has_file_read_intent)
                && !self.config.detect_extensions.is_empty()
            {
                file_detector::detect_file_paths_with_exts(&trimmed, &self.config.detect_extensions)
            } else {
                Vec::new()
            };

            // ファイル読み込み（既存ファイルのみ）、未存在は出力ターゲットとして扱う
            let mut provided_files = std::collections::HashMap::new();
            let mut output_targets = Vec::new();
            let mut files = Vec::new();
            let mut errors = Vec::new();

            if !file_paths.is_empty() {
                // 入出力の推定: ファイル操作意図があり、2つ以上のファイルが指定された場合は
                // 先頭を入力、それ以降を出力ターゲットとして扱う。
                // 単一ファイルかつファイル操作意図が強い場合（保存/書き込みなどを含む）は出力優先。
                let mut input_candidates = Vec::new();
                let mut output_candidates = Vec::new();

                if has_file_write_intent && file_paths.len() >= 2 {
                    let mut iter = file_paths.iter();
                    if let Some(first) = iter.next() {
                        input_candidates.push(first.clone());
                    }
                    for p in iter {
                        output_candidates.push(p.clone());
                    }
                } else if has_file_write_intent && prefers_output_only(&trimmed) {
                    output_candidates.extend(file_paths.clone());
                } else {
                    input_candidates.extend(file_paths.clone());
                }

                println!("\n[Detected files: {}]", file_paths.join(", "));

                for path in &input_candidates {
                    if file_ops::file_exists(path) {
                        match file_ops::read_file(path) {
                            Ok(content) => {
                                provided_files.insert(content.original_path.clone(), content.content.clone());
                                files.push(content);
                            }
                            Err(e) => errors.push((path.clone(), e.to_string())),
                        }
                    } else {
                        errors.push((path.clone(), "File not found".to_string()));
                    }
                }

                output_targets.extend(output_candidates);

                if !files.is_empty() {
                    println!("[Successfully loaded {} file(s)]", files.len());
                }
                for (path, error) in &errors {
                    eprintln!("[Error loading '{}': {}]", path, error);
                }
                if !output_targets.is_empty() {
                    println!(
                        "[Treating as output targets (not loaded): {}]",
                        output_targets.join(", ")
                    );
                }
            }

            let tool_info = self.build_tool_info();

            let prompt_build = build_prompt_with_context_limit(
                &trimmed,
                &files,
                &errors,
                tool_info.as_deref(),
                &output_targets,
                has_file_write_intent,
                !self.tool_only,
                &[],
            );
            for notice in &prompt_build.notices {
                println!(
                    "[Truncated file content: {} ({} -> {} tokens)]",
                    notice.path, notice.original_tokens, notice.kept_tokens
                );
            }
            if prompt_build.overflow {
                eprintln!(
                    "[Context] Prompt exceeds max context. Reduce input or set RKLLM_MAX_CONTEXT_TOKENS."
                );
                continue;
            }
            let prompt = prompt_build.prompt;
            if self.preview_prompt || std::env::var("RKLLM_DEBUG_PROMPT").is_ok() {
                eprintln!("\n[DEBUG prompt length={}]", prompt.len());
                eprintln!("{}", prompt);
            }

            terminal::disable_raw_mode().context("Failed to disable raw mode")?;
            print!("\n");
            io::stdout().flush().unwrap();
            match self.rkllm.run(&prompt, |text| {
                print!("{}", text);
                let _ = io::stdout().flush();
            }) {
                Ok(mut response) => {
                    println!();

                    let mut tool_rounds = 0usize;
                    let mut seen_tool_calls: HashSet<String> = HashSet::new();
                    loop {
                        // ファイル操作を処理（ユーザーに意図がある場合のみ）
                        if has_file_write_intent {
                            if self.tool_only {
                                if let Err(e) = self
                                    .process_file_operations_via_tools(
                                        &response,
                                        &provided_files,
                                        &output_targets,
                                    )
                                    .await
                                {
                                    eprintln!("\nError processing file operations via MCP tools: {}", e);
                                }
                            } else if let Err(e) = self.process_file_operations(
                                &response,
                                &provided_files,
                                &output_targets,
                            ) {
                                eprintln!("\nError processing file operations: {}", e);
                            }
                        }

                        let allowance = if tool_rounds == 0 {
                            ToolCallAllowance::All
                        } else {
                            ToolCallAllowance::WriteOnly
                        };
                        let (tool_results, blocked_repeat) = match self
                            .process_tool_calls(&response, &mut seen_tool_calls, allowance)
                            .await
                        {
                            Ok(result) => result,
                            Err(e) => {
                                eprintln!("\nError processing tool calls: {}", e);
                                (Vec::new(), false)
                            }
                            };
                        if blocked_repeat {
                            eprintln!("\n[Repeated tool call blocked]");
                            break;
                        }
                        if tool_results.is_empty() {
                            break;
                        }

                        tool_rounds += 1;
                        if tool_rounds >= 3 {
                            eprintln!("\n[Tool call limit reached]");
                            break;
                        }

                        let followup_build = build_prompt_with_context_limit(
                            &trimmed,
                            &files,
                            &errors,
                            tool_info.as_deref(),
                            &output_targets,
                            has_file_write_intent,
                            !self.tool_only,
                            &tool_results,
                        );
                        for notice in &followup_build.notices {
                            println!(
                                "[Truncated file content: {} ({} -> {} tokens)]",
                                notice.path, notice.original_tokens, notice.kept_tokens
                            );
                        }
                        if followup_build.overflow {
                            eprintln!(
                                "[Context] Prompt exceeds max context. Reduce input or set RKLLM_MAX_CONTEXT_TOKENS."
                            );
                            break;
                        }
                        let followup_prompt = followup_build.prompt;
                        if self.preview_prompt || std::env::var("RKLLM_DEBUG_PROMPT").is_ok() {
                            eprintln!("\n[DEBUG prompt length={}]", followup_prompt.len());
                            eprintln!("{}", followup_prompt);
                        }

                        let buffered = Arc::new(Mutex::new(String::new()));
                        let buffered_ref = Arc::clone(&buffered);
                        match self.rkllm.run(&followup_prompt, move |text| {
                            if let Ok(mut buf) = buffered_ref.lock() {
                                buf.push_str(text);
                            }
                        }) {
                            Ok(next_response) => {
                                let display = buffered
                                    .lock()
                                    .map(|buf| Self::strip_tool_calls(&buf))
                                    .unwrap_or_default();
                                print!("{}", display);
                                let _ = io::stdout().flush();
                                println!();
                                response = next_response;
                            }
                            Err(e) => {
                                eprintln!("\nError during inference: {}", e);
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("\nError during inference: {}", e);
                }
            }
            self.print_separator(Color::DarkGrey);
            terminal::enable_raw_mode().context("Failed to enable raw mode")?;
        }

        Ok(())
    }

    fn read_multiline_input(&self, stdout: &mut std::io::Stdout) -> Result<Option<String>> {
        let prompt_width = UnicodeWidthStr::width(Self::PROMPT);
        let indent_width = UnicodeWidthStr::width(Self::INDENT);

        // プロンプト行を起点に、毎回カーソルを戻して全体再描画する。
        let mut rendered_rows: usize = 1; // プロンプトのみの1行
        let mut buffer = InputBuffer::default();
        let (pos_col, pos_row) = cursor::position().unwrap_or((0, 0));
        let _ = pos_col;
        let anchor_col = 0;
        let mut anchor_row = pos_row;
        let mut cursor_row_offset: u16 = 0;

            let redraw = |stdout: &mut std::io::Stdout,
                      rendered_rows: &mut usize,
                      buffer: &InputBuffer,
                      anchor_row: &mut u16,
                      cursor_row_offset: &mut u16|
         -> Result<()> {
            let (_, current_row) = cursor::position().unwrap_or((*anchor_row, 0));
            *anchor_row = current_row.saturating_sub(*cursor_row_offset);

            execute!(stdout, cursor::MoveTo(anchor_col, *anchor_row))?;
            let term_width = terminal::size().map(|(w, _)| w as usize).unwrap_or(80).max(1);
            let (rows_used, cursor_pos) =
                render_input(
                    stdout,
                    Self::PROMPT,
                    Self::INDENT,
                    prompt_width,
                    indent_width,
                    term_width,
                    buffer,
                    Self::PROMPT_BG,
                    Self::PROMPT_FG,
                    Self::INPUT_BG,
                    Self::INPUT_FG,
                )?;
            *rendered_rows = rows_used;
            *cursor_row_offset = cursor_pos.0 as u16;
            Ok(())
        };

        redraw(stdout, &mut rendered_rows, &buffer, &mut anchor_row, &mut cursor_row_offset)?;

        loop {
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(key_event) => match key_event {
                        KeyEvent {
                            code: KeyCode::Char('c'),
                            modifiers: KeyModifiers::CONTROL,
                            ..
                        } => {
                            let now = Instant::now();
                            let mut last_time = self.last_ctrl_c.lock().unwrap();

                            if let Some(last) = *last_time {
                                if now.duration_since(last).as_secs() < 2 {
                                    return Ok(None);
                                }
                            }

                            *last_time = Some(now);
                            execute!(stdout, Print("\r\n[Press Ctrl+C again to exit]\r\n"))?;
                            execute!(
                                stdout,
                                SetBackgroundColor(Self::PROMPT_BG),
                                SetForegroundColor(Self::PROMPT_FG),
                                Print(Self::PROMPT),
                                ResetColor
                            )?;
                            rendered_rows = 1;
                            redraw(stdout, &mut rendered_rows, &buffer, &mut anchor_row, &mut cursor_row_offset)?;
                        }
                        KeyEvent {
                            code: KeyCode::Char('d'),
                            modifiers: KeyModifiers::CONTROL,
                            ..
                        } => {
                            return Ok(None);
                        }
                        KeyEvent {
                            code: KeyCode::Char('j'),
                            modifiers,
                            ..
                        } if modifiers.contains(KeyModifiers::CONTROL) => {
                            buffer.insert_str("\n");
                            redraw(stdout, &mut rendered_rows, &buffer, &mut anchor_row, &mut cursor_row_offset)?;
                        }
                        KeyEvent {
                            code: KeyCode::Enter,
                            modifiers: KeyModifiers::SHIFT,
                            ..
                        } => {
                            buffer.insert_str("\n");
                            redraw(stdout, &mut rendered_rows, &buffer, &mut anchor_row, &mut cursor_row_offset)?;
                        }
                        KeyEvent {
                            code: KeyCode::Enter,
                            modifiers: KeyModifiers::NONE,
                            ..
                        } => {
                            redraw(stdout, &mut rendered_rows, &buffer, &mut anchor_row, &mut cursor_row_offset)?;
                            execute!(stdout, Print("\r\n"))?;
                            return Ok(Some(buffer.to_string()));
                        }
                        KeyEvent {
                            code: KeyCode::Backspace,
                            ..
                        } => {
                            if buffer.backspace() {
                                redraw(stdout, &mut rendered_rows, &buffer, &mut anchor_row, &mut cursor_row_offset)?;
                            }
                        }
                        KeyEvent {
                            code: KeyCode::Delete,
                            ..
                        } => {
                            if buffer.delete() {
                                redraw(stdout, &mut rendered_rows, &buffer, &mut anchor_row, &mut cursor_row_offset)?;
                            }
                        }
                        KeyEvent {
                            code: KeyCode::Left,
                            ..
                        } => {
                            if buffer.move_left() {
                                redraw(stdout, &mut rendered_rows, &buffer, &mut anchor_row, &mut cursor_row_offset)?;
                            }
                        }
                        KeyEvent {
                            code: KeyCode::Right,
                            ..
                        } => {
                            if buffer.move_right() {
                                redraw(stdout, &mut rendered_rows, &buffer, &mut anchor_row, &mut cursor_row_offset)?;
                            }
                        }
                        KeyEvent {
                            code: KeyCode::Up,
                            ..
                        } => {
                            let term_width = terminal::size().map(|(w, _)| w as usize).unwrap_or(80).max(1);
                            if buffer.move_vertical(-1, prompt_width, indent_width, term_width) {
                                redraw(stdout, &mut rendered_rows, &buffer, &mut anchor_row, &mut cursor_row_offset)?;
                            }
                        }
                        KeyEvent {
                            code: KeyCode::Down,
                            ..
                        } => {
                            let term_width = terminal::size().map(|(w, _)| w as usize).unwrap_or(80).max(1);
                            if buffer.move_vertical(1, prompt_width, indent_width, term_width) {
                                redraw(stdout, &mut rendered_rows, &buffer, &mut anchor_row, &mut cursor_row_offset)?;
                            }
                        }
                        KeyEvent {
                            code: KeyCode::Char(c),
                            modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
                            ..
                        } => {
                            buffer.insert_str(&c.to_string());
                            redraw(stdout, &mut rendered_rows, &buffer, &mut anchor_row, &mut cursor_row_offset)?;
                        }
                        _ => {}
                    },
                    Event::Paste(content) => {
                        let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
                        buffer.insert_str(&normalized);
                        redraw(stdout, &mut rendered_rows, &buffer, &mut anchor_row, &mut cursor_row_offset)?;
                    }
                    _ => {}
                }
                stdout.flush()?;
            }
        }
    }

    fn print_banner(&self) {
        let mut stdout = stdout();
        let lines = [
            [
                ("██████ ", Color::Red),    // R
                (" ", Color::Reset),
                ("██  ██", Color::Yellow),  // K
                ("  ", Color::Reset),
                ("██      ", Color::Green), // L
                ("██      ", Color::Green), // L
                ("██   ██", Color::Cyan),   // M
            ],
            [
                ("██   ██", Color::Red),
                (" ", Color::Reset),
                ("██ ██", Color::Yellow),
                ("   ", Color::Reset),
                ("██      ", Color::Green),
                ("██      ", Color::Green),
                ("███ ███", Color::Cyan),
            ],
            [
                ("██████", Color::Red),
                ("  ", Color::Reset),
                ("████", Color::Yellow),
                ("    ", Color::Reset),
                ("██      ", Color::Green),
                ("██      ", Color::Green),
                ("███████", Color::Cyan),
            ],
            [
                ("██  ██", Color::Red),
                ("  ", Color::Reset),
                ("██ ██", Color::Yellow),
                ("   ", Color::Reset),
                ("██      ", Color::Green),
                ("██      ", Color::Green),
                ("██   ██", Color::Cyan),
            ],
            [
                ("██   ██", Color::Red),
                (" ", Color::Reset),
                ("██  ██", Color::Yellow),
                ("  ", Color::Reset),
                ("███████ ", Color::Green),
                ("███████ ", Color::Green),
                ("██   ██", Color::Cyan),
            ],
            [
                ("░░   ░░", Color::Red),
                (" ", Color::Reset),
                ("░░  ░░", Color::Yellow),
                ("  ", Color::Reset),
                ("░░░░░░░ ", Color::Green),
                ("░░░░░░░ ", Color::Green),
                ("░░   ░░", Color::Cyan),
            ],
        ];

        for line in &lines {
            for (text, color) in line {
                execute!(stdout, SetForegroundColor(*color), Print(*text)).ok();
            }
            execute!(stdout, ResetColor, Print("\n")).ok();
        }
        execute!(
            stdout,
            SetForegroundColor(Color::DarkGrey),
            Print("Rockchip NPU Agentic CLI\n\n"),
            ResetColor
        )
        .ok();
    }

    fn print_status_line(&self, stdout: &mut std::io::Stdout, status: &str) -> Result<()> {
        let mcp = if self.mcp_client.is_some() { "on" } else { "off" };
        let mode = if self.tool_only { "tool-only" } else { "chat" };
        let line = format!(
            "[Dir: {} | Status: {} | MCP: {} | Mode: {}]",
            self.execution_dir, status, mcp, mode
        );
        execute!(
            stdout,
            ResetColor,
            cursor::MoveToColumn(0),
            terminal::Clear(terminal::ClearType::CurrentLine),
            SetForegroundColor(Color::DarkGrey),
            Print(line),
            ResetColor,
            Print("\r\n")
        )?;
        Ok(())
    }

    fn show_tools_command(&self, stdout: &mut std::io::Stdout) -> Result<()> {
        execute!(stdout, Print("\r\n"))?;
        let Some(mcp_client) = &self.mcp_client else {
            execute!(stdout, Print("[No MCP client configured]\r\n"))?;
            return Ok(());
        };
        let tools = mcp_client.list_all_tools();
        if tools.is_empty() {
            execute!(stdout, Print("[No tools available]\r\n"))?;
            return Ok(());
        }

        execute!(
            stdout,
            SetForegroundColor(Color::Cyan),
            Print("Available Tools:\r\n"),
            ResetColor
        )?;
        for (_server_name, tool) in &tools {
            execute!(
                stdout,
                SetForegroundColor(Color::Yellow),
                Print(format!("  {}\r\n", tool.name)),
                ResetColor
            )?;
            if let Some(desc) = &tool.description {
                execute!(stdout, Print(format!("    {}\r\n", desc)))?;
            }
        }
        execute!(stdout, Print("\r\n"))?;
        Ok(())
    }

    fn show_help_command(&self, stdout: &mut std::io::Stdout) -> Result<()> {
        execute!(stdout, Print("\r\n"))?;
        execute!(
            stdout,
            SetForegroundColor(Color::Cyan),
            Print("Available Commands:\r\n"),
            ResetColor
        )?;
        execute!(stdout, Print("  help   - Show this help message\r\n"))?;
        execute!(stdout, Print("  tools  - List available MCP tools\r\n"))?;
        execute!(stdout, Print("  quit   - Exit the application (also 'exit')\r\n"))?;
        execute!(stdout, Print("\r\n"))?;
        Ok(())
    }

    fn print_separator(&self, color: Color) {
        let width = if let Ok((cols, _)) = terminal::size() {
            cols as usize
        } else {
            80
        };
        print!("{}", SetForegroundColor(color));
        print!("{}", "─".repeat(width));
        print!("{}", ResetColor);
        print!("\r\n");
    }

    fn strip_tool_calls(text: &str) -> String {
        // Remove <tool_call ...>...</tool_call> blocks from display output.
        let pattern = Regex::new(r#"(?s)<tool_call\s+name="[^"]+"\s*>.*?</tool_call>"#)
            .unwrap();
        pattern.replace_all(text, "").to_string()
    }

    fn build_tool_sample_block(tool: &Tool) -> String {
        let sample_args = Self::build_sample_arguments(tool);
        let pretty = serde_json::to_string_pretty(&sample_args).unwrap_or_else(|_| "{}".to_string());

        format!("<tool_call name=\"{}\">\n{}\n</tool_call>\n", tool.name, pretty)
    }

    fn build_sample_arguments(tool: &Tool) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        let mut added = false;
        let required: HashSet<&str> = tool
            .input_schema
            .required
            .as_ref()
            .map(|keys| keys.iter().map(|k| k.as_str()).collect())
            .unwrap_or_default();

        if let Some(properties) = &tool.input_schema.properties {
            let mut keys: Vec<&String> = if required.is_empty() {
                properties.keys().collect()
            } else {
                properties
                    .keys()
                    .filter(|key| required.contains(key.as_str()))
                    .collect()
            };
            keys.sort();

            for key in keys {
                if let Some(schema) = properties.get(key) {
                    map.insert(key.clone(), Self::sample_value_for_schema(schema));
                    added = true;
                }
            }
        }

        if !added {
            map.insert("example".to_string(), serde_json::Value::String("value".to_string()));
        }

        serde_json::Value::Object(map)
    }

    fn sample_value_for_schema(schema: &serde_json::Value) -> serde_json::Value {
        if let Some(default) = schema.get("default") {
            return default.clone();
        }

        if let Some(enum_values) = schema.get("enum").and_then(|v| v.as_array()) {
            if let Some(first) = enum_values.first() {
                return first.clone();
            }
        }

        match schema.get("type").and_then(|v| v.as_str()) {
            Some("string") => serde_json::Value::String("example".to_string()),
            Some("integer") | Some("number") => json!(0),
            Some("boolean") => serde_json::Value::Bool(true),
            Some("array") => {
                if let Some(items) = schema.get("items") {
                    serde_json::Value::Array(vec![Self::sample_value_for_schema(items)])
                } else {
                    serde_json::Value::Array(vec![])
                }
            }
            Some("object") => serde_json::Value::Object(serde_json::Map::new()),
            _ => serde_json::Value::String("value".to_string()),
        }
    }

    fn build_tool_info(&self) -> Option<String> {
        let Some(mcp_client) = &self.mcp_client else {
            return None;
        };
        let tools = mcp_client.list_all_tools();
        if tools.is_empty() {
            return None;
        }

        let mut info = String::from("\n## Available Tools\n\n");
        info.push_str("Available tools (short list):\n\n");

        for (_server_name, tool) in &tools {
            info.push_str(&format!("### {}\n", tool.name));
            if let Some(desc) = &tool.description {
                info.push_str(&format!("{}\n", desc));
            }
            let sample_block = ChatSession::build_tool_sample_block(tool);
            info.push_str("\nSample:\n");
            info.push_str(&sample_block);
        }

        info.push_str("\nTo use a tool, output (see per-tool samples above):\n\n");
        info.push_str("<tool_call name=\"tool_name\">\n");
        info.push_str("{\n");
        info.push_str("  \"argument_name\": \"value\"\n");
        info.push_str("}\n");
        info.push_str("</tool_call>\n");

        Some(info)
    }

    /// ファイル上書きの確認を求める
    ///
    /// # 引数
    /// * `path` - ファイルパス
    ///
    /// # 戻り値
    /// ユーザーが'y'を入力した場合はtrue、それ以外はfalse
    fn confirm_overwrite(&self, path: &str) -> Result<bool> {
        self.prompt_confirm(&format!(
            "\n[File '{}' already exists. Overwrite? (y/N): ",
            path
        ))
    }

    /// 書き込み確認（--confirm-writes 用）
    fn confirm_write(&self, path: &str, exists: bool) -> Result<bool> {
        let prefix = if exists {
            "[File exists]"
        } else {
            "[Write]"
        };
        self.prompt_confirm(&format!("\n{} '{}' ? (y/N): ", prefix, path))
    }

    fn prompt_confirm(&self, message: &str) -> Result<bool> {
        print!("{}", message);
        io::stdout().flush()?;

        // 一時的にraw modeを無効化
        let was_raw_mode = terminal::is_raw_mode_enabled()?;
        if was_raw_mode {
            terminal::disable_raw_mode()?;
        }

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        // raw modeを元に戻す
        if was_raw_mode {
            terminal::enable_raw_mode()?;
        }

        Ok(input.trim().eq_ignore_ascii_case("y"))
    }

    /// tool-only モード時にファイル操作を MCP ツールに委譲する
    async fn process_file_operations_via_tools(
        &self,
        output: &str,
        provided_files: &std::collections::HashMap<String, String>,
        output_targets: &[String],
    ) -> Result<()> {
        let Some(mcp_client) = &self.mcp_client else {
            eprintln!("[tool-only] MCP client is not available.");
            return Ok(());
        };

        let mut operations = file_output_parser::parse_file_operations(output);
        if operations.is_empty() {
            return Ok(());
        }

        println!(
            "\n[Detected {} file operation(s) (tool-only)]",
            operations.len()
        );

        // 入力と同一内容はスキップ
        operations = operations
            .into_iter()
            .filter(|op| {
                if let Some((input_path, _)) =
                    provided_files
                        .iter()
                        .find(|(_, content)| contents_equal(content, &op.content))
                {
                    println!(
                        "[Skipped unchanged (matches input {}): {}]",
                        input_path, op.path
                    );
                    return false;
                }
                true
            })
            .collect();

        if operations.is_empty() {
            println!("[No file operations after filtering unchanged content]");
            return Ok(());
        }

        // もし出力パスが未指定で、かつ出力ターゲットが1つだけならリマップ
        if !output_targets.is_empty() && output_targets.len() == 1 {
            let target = &output_targets[0];
            let all_input_paths: bool = operations
                .iter()
                .all(|op| provided_files.contains_key(&op.path));
            if all_input_paths {
                for op in operations.iter_mut() {
                    println!("[Remap {} -> {}]", op.path, target);
                    op.path = target.clone();
                }
            }
        }

        let tools = mcp_client.list_all_tools();
        let Some(write_tool_name) = Self::select_write_tool_name(&tools) else {
            eprintln!("[tool-only] No suitable MCP write tool found. Skipping file outputs.");
            return Ok(());
        };

        for op in operations {
            let args = json!({
                "path": op.path,
                "content": op.content,
            });

            match mcp_client.call_tool(&write_tool_name, args).await {
                Ok(result) => {
                    if result.success {
                        println!(
                            "[tool-only] Wrote via tool '{}': {}",
                            write_tool_name, op.path
                        );
                    } else {
                        eprintln!(
                            "[tool-only] Tool '{}' failed for {}: {}",
                            write_tool_name, op.path, result.output
                        );
                    }
                }
                Err(e) => {
                    eprintln!(
                        "[tool-only] Failed to call tool '{}' for {}: {}",
                        write_tool_name, op.path, e
                    );
                }
            }
        }

        Ok(())
    }

    /// LLMの応答からファイル操作を処理する
    ///
    /// # 引数
    /// * `output` - LLMの出力テキスト
    /// * `provided_files` - 入力として読み込んだファイル内容
    /// * `output_targets` - 入力で未存在だった出力候補パス
    fn process_file_operations(
        &self,
        output: &str,
        provided_files: &std::collections::HashMap<String, String>,
        output_targets: &[String],
    ) -> Result<()> {
        let mut operations = file_output_parser::parse_file_operations(output);

        if operations.is_empty() {
            return Ok(());
        }

        println!("\n[Detected {} file operation(s)]", operations.len());

        // 入力と同一内容はスキップ
        operations = operations
            .into_iter()
            .filter(|op| {
                if let Some((input_path, _)) =
                    provided_files
                        .iter()
                        .find(|(_, content)| contents_equal(content, &op.content))
                {
                    println!(
                        "[Skipped unchanged (matches input {}): {}]",
                        input_path, op.path
                    );
                    return false;
                }
                true
            })
            .collect();

        if operations.is_empty() {
            println!("[No file operations after filtering unchanged content]");
            return Ok(());
        }

        // もし出力パスが未指定で、かつ出力ターゲットが1つだけならリマップ
        if !output_targets.is_empty() && output_targets.len() == 1 {
            let target = &output_targets[0];
            let all_input_paths: bool = operations
                .iter()
                .all(|op| provided_files.contains_key(&op.path));
            if all_input_paths {
                for op in operations.iter_mut() {
                    println!("[Remap {} -> {}]", op.path, target);
                    op.path = target.clone();
                }
            }
        }

        for op in operations {
            match op.operation_type {
                file_output_parser::FileOperationType::Create => {
                    let exists = file_ops::file_exists(&op.path);

                    // 書き込み前の確認
                    if self.confirm_writes {
                        if !self.confirm_write(&op.path, exists)? {
                            println!("[Skipped by confirm: {}]", op.path);
                            continue;
                        }
                    } else if exists && !self.confirm_overwrite(&op.path)? {
                        println!("[Skipped: {}]", op.path);
                        continue;
                    }

                    // ファイルを書き込む
                    match file_ops::write_file(&op.path, &op.content, false) {
                        Ok(_) => {
                            println!("[Created/Updated: {}]", op.path);
                        }
                        Err(e) => {
                            eprintln!("[Error writing '{}': {}]", op.path, e);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// LLMの応答からツール呼び出しを処理する
    ///
    /// # 引数
    /// * `output` - LLMの出力テキスト
    async fn process_tool_calls(
        &self,
        output: &str,
        seen_tool_calls: &mut HashSet<String>,
        allowance: ToolCallAllowance,
    ) -> Result<(Vec<ToolResult>, bool)> {
        let tool_calls = self.tool_detector.detect(output);

        if tool_calls.is_empty() {
            return Ok((Vec::new(), false));
        }

        println!("\n[Detected {} tool call(s)]", tool_calls.len());

        let mut results = Vec::new();
        let mut blocked_repeat = false;

        for call in tool_calls {
            if matches!(allowance, ToolCallAllowance::WriteOnly) && call.name != "write_file" {
                blocked_repeat = true;
                continue;
            }

            if seen_tool_calls.contains(&call.name) {
                blocked_repeat = true;
                continue;
            }
            seen_tool_calls.insert(call.name.clone());

            match call.name.as_str() {
                "read_file" => {
                    results.push(self.handle_read_file_tool_call(&call));
                }
                "write_file" => {
                    results.push(self.handle_write_file_tool_call(&call)?);
                }
                _ => {
                    if let Some(client) = &self.mcp_client {
                        match client.call_tool(&call.name, call.arguments).await {
                            Ok(mut result) => {
                                result.name = call.name.clone();
                                if result.success {
                                    if result.output.trim().is_empty() {
                                        println!("\n[Tool '{}' success with empty output]", call.name);
                                    } else {
                                        println!("\n[Tool '{}' output:]", call.name);
                                        println!("{}", result.output);
                                    }
                                } else {
                                    eprintln!("\n[Tool '{}' failed:]", call.name);
                                    eprintln!("{}", result.output);
                                }
                                results.push(result);
                            }
                            Err(e) => {
                                eprintln!("\n[Failed to call tool '{}': {}]", call.name, e);
                                results.push(Self::tool_result_json(
                                    &call.name,
                                    false,
                                    json!({"error": e.to_string()}),
                                ));
                            }
                        }
                    } else {
                        eprintln!("\n[Unknown tool '{}': no MCP client]", call.name);
                        results.push(Self::tool_result_json(
                            &call.name,
                            false,
                            json!({"error": "No MCP client available"}),
                        ));
                    }
                }
            }
        }

        Ok((results, blocked_repeat))
    }

    fn handle_read_file_tool_call(&self, call: &ToolCall) -> ToolResult {
        let path = call
            .arguments
            .get("path")
            .and_then(|v| v.as_str())
            .map(|v| v.to_string());

        let Some(path) = path else {
            return Self::tool_result_json(
                "read_file",
                false,
                json!({"error": "Missing required argument: path"}),
            );
        };

        match file_ops::read_file(&path) {
            Ok(content) => Self::tool_result_json(
                "read_file",
                true,
                json!({"path": content.original_path, "content": content.content}),
            ),
            Err(e) => Self::tool_result_json(
                "read_file",
                false,
                json!({"path": path, "error": e.to_string()}),
            ),
        }
    }

    fn handle_write_file_tool_call(&self, call: &ToolCall) -> Result<ToolResult> {
        if self.tool_only {
            return Ok(Self::tool_result_json(
                "write_file",
                false,
                json!({"error": "Local writes are disabled in tool-only mode"}),
            ));
        }

        let path = call
            .arguments
            .get("path")
            .and_then(|v| v.as_str())
            .map(|v| v.to_string());
        let content = call
            .arguments
            .get("content")
            .and_then(|v| v.as_str())
            .map(|v| v.to_string());

        let Some(path) = path else {
            return Ok(Self::tool_result_json(
                "write_file",
                false,
                json!({"error": "Missing required argument: path"}),
            ));
        };
        let Some(content) = content else {
            return Ok(Self::tool_result_json(
                "write_file",
                false,
                json!({"error": "Missing required argument: content"}),
            ));
        };

        let exists = file_ops::file_exists(&path);
        if self.confirm_writes {
            if !self.confirm_write(&path, exists)? {
                return Ok(Self::tool_result_json(
                    "write_file",
                    false,
                    json!({"path": path, "skipped": true}),
                ));
            }
        } else if exists && !self.confirm_overwrite(&path)? {
            return Ok(Self::tool_result_json(
                "write_file",
                false,
                json!({"path": path, "skipped": true}),
            ));
        }

        match file_ops::write_file(&path, &content, false) {
            Ok(_) => Ok(Self::tool_result_json(
                "write_file",
                true,
                json!({"path": path, "written": true}),
            )),
            Err(e) => Ok(Self::tool_result_json(
                "write_file",
                false,
                json!({"path": path, "error": e.to_string()}),
            )),
        }
    }

    fn tool_result_json(name: &str, success: bool, payload: serde_json::Value) -> ToolResult {
        let output = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());
        ToolResult {
            name: name.to_string(),
            success,
            output,
        }
    }

    /// MCPツールから書き込み用ツール名を推定する
    fn select_write_tool_name(tools: &[(&str, &Tool)]) -> Option<String> {
        let mut best: Option<(u8, String)> = None;

        for (_server, tool) in tools {
            let name_lower = tool.name.to_lowercase();
            let rank = if name_lower == "write_file" || name_lower == "writefile" {
                Some(0)
            } else if name_lower.contains("write") && name_lower.contains("file") {
                Some(1)
            } else if let Some(props) = &tool.input_schema.properties {
                if props.contains_key("path") && props.contains_key("content") {
                    Some(2)
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(r) = rank {
                let should_replace = best.as_ref().map(|(current, _)| r < *current).unwrap_or(true);
                if should_replace {
                    best = Some((r, tool.name.clone()));
                }
            }
        }

        best.map(|(_, name)| name)
    }

}

fn render_input(
    stdout: &mut std::io::Stdout,
    prompt: &str,
    indent: &str,
    prompt_width: usize,
    indent_width: usize,
    term_width: usize,
    buffer: &InputBuffer,
    prompt_bg: Color,
    prompt_fg: Color,
    input_bg: Color,
    input_fg: Color,
) -> Result<(usize, (usize, usize))> {
    let positions = buffer.layout_positions(prompt_width, indent_width, term_width);
    let cursor_pos = positions
        .get(buffer.cursor)
        .copied()
        .unwrap_or((0, prompt_width));

    execute!(
        stdout,
        cursor::MoveToColumn(0),
        terminal::Clear(terminal::ClearType::FromCursorDown)
    )?;
    prepare_input_line(stdout, term_width, input_bg, input_fg)?;
    execute!(stdout, Print("\r\n"))?;
    prepare_input_line(stdout, term_width, input_bg, input_fg)?;
    execute!(
        stdout,
        SetBackgroundColor(prompt_bg),
        SetForegroundColor(prompt_fg),
        Print(prompt),
        SetBackgroundColor(input_bg),
        SetForegroundColor(input_fg)
    )?;

    let mut col = prompt_width;
    let mut rows_used = 1usize;

    for grapheme in &buffer.graphemes {
        if grapheme == "\n" {
            fill_input_line(stdout, term_width, col, input_bg, input_fg)?;
            execute!(stdout, Print("\r\n"))?;
            prepare_input_line(stdout, term_width, input_bg, input_fg)?;
            execute!(
                stdout,
                SetBackgroundColor(input_bg),
                SetForegroundColor(input_fg),
                Print(indent)
            )?;
            rows_used += 1;
            col = indent_width;
            continue;
        }

        let w = UnicodeWidthStr::width(grapheme.as_str()).max(1);
        if col + w > term_width {
            fill_input_line(stdout, term_width, col, input_bg, input_fg)?;
            execute!(stdout, Print("\r\n"))?;
            prepare_input_line(stdout, term_width, input_bg, input_fg)?;
            execute!(
                stdout,
                SetBackgroundColor(input_bg),
                SetForegroundColor(input_fg),
                Print(indent)
            )?;
            rows_used += 1;
            col = indent_width;
        }

        execute!(stdout, Print(grapheme))?;
        col += w;
    }

    fill_input_line(stdout, term_width, col, input_bg, input_fg)?;
    execute!(stdout, Print("\r\n"))?;
    prepare_input_line(stdout, term_width, input_bg, input_fg)?;
    execute!(stdout, ResetColor)?;

    let padding_rows = 1usize;
    let bottom_padding = 1usize;
    let cursor_row = cursor_pos.0 + padding_rows;
    let current_row = padding_rows + rows_used.saturating_sub(1) + bottom_padding;
    let rows_above_cursor = current_row.saturating_sub(cursor_row);
    if rows_above_cursor > 0 {
        execute!(stdout, cursor::MoveUp(rows_above_cursor as u16))?;
    }
    execute!(stdout, cursor::MoveToColumn(cursor_pos.1 as u16))?;
    stdout.flush()?;
    Ok((rows_used + padding_rows + bottom_padding, (cursor_row, cursor_pos.1)))
}

fn fill_input_line(
    stdout: &mut std::io::Stdout,
    term_width: usize,
    col: usize,
    input_bg: Color,
    input_fg: Color,
) -> Result<()> {
    if col >= term_width {
        return Ok(());
    }
    let remaining = term_width - col;
    execute!(
        stdout,
        SetBackgroundColor(input_bg),
        SetForegroundColor(input_fg),
        Print(" ".repeat(remaining))
    )?;
    Ok(())
}

fn prepare_input_line(
    stdout: &mut std::io::Stdout,
    term_width: usize,
    input_bg: Color,
    input_fg: Color,
) -> Result<()> {
    if term_width == 0 {
        return Ok(());
    }
    execute!(
        stdout,
        SetBackgroundColor(input_bg),
        SetForegroundColor(input_fg),
        Print(" ".repeat(term_width)),
        cursor::MoveToColumn(0)
    )?;
    Ok(())
}

/// 改行差分や末尾空白を無視して内容一致を判定
fn contents_equal(a: &str, b: &str) -> bool {
    fn normalize(s: &str) -> String {
        s.replace("\r\n", "\n").trim_end().to_string()
    }
    normalize(a) == normalize(b)
}

struct TruncationNotice {
    path: String,
    original_tokens: usize,
    kept_tokens: usize,
}

struct PromptWithLimit {
    prompt: String,
    notices: Vec<TruncationNotice>,
    overflow: bool,
}

static MAX_CONTEXT_TOKENS: OnceCell<usize> = OnceCell::new();
static CONTEXT_RESERVED_TOKENS: OnceCell<usize> = OnceCell::new();

fn build_prompt_with_context_limit(
    user_input: &str,
    files: &[file_ops::FileContent],
    errors: &[(String, String)],
    tool_info: Option<&str>,
    output_targets: &[String],
    has_file_op_intent: bool,
    file_writes_enabled: bool,
    tool_results: &[ToolResult],
) -> PromptWithLimit {
    let max_tokens = max_context_tokens();
    let reserved_tokens = context_reserved_tokens();

    let base_prompt = build_chat_prompt(
        user_input,
        &[],
        errors,
        tool_info,
        output_targets,
        has_file_op_intent,
        file_writes_enabled,
        tool_results,
    );
    let base_tokens = estimate_tokens(&base_prompt);
    if base_tokens >= max_tokens {
        return PromptWithLimit {
            prompt: base_prompt,
            notices: Vec::new(),
            overflow: true,
        };
    }

    let budget_tokens = max_tokens.saturating_sub(base_tokens + reserved_tokens);
    let (trimmed_files, notices) = truncate_files_to_budget(files, budget_tokens);
    let prompt = build_chat_prompt(
        user_input,
        &trimmed_files,
        errors,
        tool_info,
        output_targets,
        has_file_op_intent,
        file_writes_enabled,
        tool_results,
    );
    let overflow = estimate_tokens(&prompt) > max_tokens;
    if overflow {
        return PromptWithLimit {
            prompt: base_prompt,
            notices,
            overflow: true,
        };
    }

    PromptWithLimit {
        prompt,
        notices,
        overflow: false,
    }
}

fn truncate_files_to_budget(
    files: &[file_ops::FileContent],
    budget_tokens: usize,
) -> (Vec<file_ops::FileContent>, Vec<TruncationNotice>) {
    if files.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let original_tokens: Vec<usize> = files
        .iter()
        .map(|file| estimate_tokens(&file.content))
        .collect();
    let total_tokens: usize = original_tokens.iter().sum();

    if budget_tokens == 0 {
        let notices = files
            .iter()
            .enumerate()
            .map(|(idx, file)| TruncationNotice {
                path: file.original_path.clone(),
                original_tokens: original_tokens[idx],
                kept_tokens: 0,
            })
            .collect();
        return (Vec::new(), notices);
    }

    if total_tokens <= budget_tokens {
        return (files.to_vec(), Vec::new());
    }

    let mut allocations = vec![0usize; files.len()];
    if total_tokens > 0 {
        for i in 0..files.len() {
            allocations[i] = budget_tokens * original_tokens[i] / total_tokens;
        }
    }

    let allocated: usize = allocations.iter().sum();
    let mut remainder = budget_tokens.saturating_sub(allocated);
    let mut order: Vec<usize> = (0..files.len()).collect();
    order.sort_by_key(|&i| Reverse(original_tokens[i]));
    for idx in order {
        if remainder == 0 {
            break;
        }
        allocations[idx] += 1;
        remainder -= 1;
    }

    let mut trimmed_files = Vec::with_capacity(files.len());
    let mut notices = Vec::new();

    for (idx, file) in files.iter().enumerate() {
        let limit_tokens = allocations[idx];
        if limit_tokens == 0 {
            notices.push(TruncationNotice {
                path: file.original_path.clone(),
                original_tokens: original_tokens[idx],
                kept_tokens: 0,
            });
            continue;
        }

        let (content, kept_tokens, truncated) =
            truncate_file_content(&file.content, limit_tokens);
        if truncated {
            notices.push(TruncationNotice {
                path: file.original_path.clone(),
                original_tokens: original_tokens[idx],
                kept_tokens,
            });
        }
        trimmed_files.push(file_ops::FileContent {
            content,
            original_path: file.original_path.clone(),
        });
    }

    (trimmed_files, notices)
}

fn truncate_file_content(content: &str, limit_tokens: usize) -> (String, usize, bool) {
    if content.is_empty() || limit_tokens == 0 {
        return (String::new(), 0, !content.is_empty());
    }

    let max_bytes = limit_tokens.saturating_mul(3).max(1);
    if content.as_bytes().len() <= max_bytes {
        return (content.to_string(), estimate_tokens(content), false);
    }

    let marker = "\n[...truncated...]\n";
    let marker_bytes = marker.as_bytes().len();
    let keep_bytes = max_bytes.saturating_sub(marker_bytes);
    if keep_bytes == 0 {
        let head = take_head_by_bytes(content, max_bytes);
        let kept_tokens = estimate_tokens(head);
        return (head.to_string(), kept_tokens, true);
    }

    let head_bytes = keep_bytes * 2 / 3;
    let tail_bytes = keep_bytes.saturating_sub(head_bytes);
    let head = take_head_by_bytes(content, head_bytes);
    let tail = if tail_bytes > 0 {
        take_tail_by_bytes(content, tail_bytes)
    } else {
        ""
    };
    let truncated = format!("{}{}{}", head, marker, tail);
    let kept_tokens = estimate_tokens(&truncated);
    (truncated, kept_tokens, true)
}

fn take_head_by_bytes(text: &str, max_bytes: usize) -> &str {
    if max_bytes == 0 || text.is_empty() {
        return "";
    }
    if text.as_bytes().len() <= max_bytes {
        return text;
    }
    let mut end = 0usize;
    for (idx, ch) in text.char_indices() {
        let next = idx + ch.len_utf8();
        if next > max_bytes {
            break;
        }
        end = next;
    }
    &text[..end]
}

fn take_tail_by_bytes(text: &str, max_bytes: usize) -> &str {
    if max_bytes == 0 || text.is_empty() {
        return "";
    }
    if text.as_bytes().len() <= max_bytes {
        return text;
    }
    let start_target = text.as_bytes().len().saturating_sub(max_bytes);
    for (idx, _) in text.char_indices() {
        if idx >= start_target {
            return &text[idx..];
        }
    }
    ""
}

fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let bytes = text.as_bytes().len();
    (bytes + 2) / 3
}

fn max_context_tokens() -> usize {
    *MAX_CONTEXT_TOKENS.get_or_init(|| {
        std::env::var("RKLLM_MAX_CONTEXT_TOKENS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(4096)
    })
}

fn context_reserved_tokens() -> usize {
    *CONTEXT_RESERVED_TOKENS.get_or_init(|| {
        std::env::var("RKLLM_CONTEXT_RESERVED_TOKENS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(256)
    })
}

#[cfg(test)]
mod tests {
    use super::ChatSession;
    use crate::mcp::types::{Tool, ToolInputSchema};
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn build_sample_arguments_prefers_required() {
        let mut props = HashMap::new();
        props.insert("path".to_string(), json!({"type": "string", "default": "/tmp"}));
        props.insert("recursive".to_string(), json!({"type": "boolean"}));
        let tool = Tool {
            name: "list_directory".to_string(),
            description: None,
            input_schema: ToolInputSchema {
                schema_type: "object".to_string(),
                properties: Some(props),
                required: Some(vec!["path".to_string()]),
                additional_properties: None,
            },
        };

        let args = ChatSession::build_sample_arguments(&tool);
        let obj = args.as_object().unwrap();
        assert_eq!(obj.len(), 1);
        assert_eq!(obj.get("path"), Some(&json!("/tmp")));
    }

    #[test]
    fn build_sample_arguments_adds_placeholder_when_empty() {
        let tool = Tool {
            name: "ping".to_string(),
            description: None,
            input_schema: ToolInputSchema {
                schema_type: "object".to_string(),
                properties: None,
                required: None,
                additional_properties: None,
            },
        };

        let args = ChatSession::build_sample_arguments(&tool);
        let obj = args.as_object().unwrap();
        assert_eq!(obj.get("example"), Some(&json!("value")));
    }

    #[test]
    fn build_tool_sample_block_contains_wrappers() {
        let mut props = HashMap::new();
        props.insert("message".to_string(), json!({"type": "string"}));
        let tool = Tool {
            name: "echo".to_string(),
            description: None,
            input_schema: ToolInputSchema {
                schema_type: "object".to_string(),
                properties: Some(props),
                required: None,
                additional_properties: None,
            },
        };

        let block = ChatSession::build_tool_sample_block(&tool);
        assert!(block.contains("<tool_call name=\"echo\">"));
        assert!(block.contains("\"message\""));
        assert!(block.contains("</tool_call>"));
    }

    #[test]
    fn select_write_tool_prefers_exact_match() {
        let mut props = HashMap::new();
        props.insert("path".to_string(), json!({"type": "string"}));
        props.insert("content".to_string(), json!({"type": "string"}));

        let tools = vec![
            ("fs", Tool {
                name: "save_file".to_string(),
                description: None,
                input_schema: ToolInputSchema {
                    schema_type: "object".to_string(),
                    properties: Some(props.clone()),
                    required: None,
                    additional_properties: None,
                },
            }),
            ("fs", Tool {
                name: "write_file".to_string(),
                description: None,
                input_schema: ToolInputSchema {
                    schema_type: "object".to_string(),
                    properties: Some(props.clone()),
                    required: None,
                    additional_properties: None,
                },
            }),
        ];

        let wrapped: Vec<(&str, &Tool)> = tools
            .iter()
            .map(|(server, tool)| (*server, tool))
            .collect();
        let selected = ChatSession::select_write_tool_name(&wrapped);
        assert_eq!(selected.as_deref(), Some("write_file"));
    }

    #[test]
    fn select_write_tool_falls_back_on_props() {
        let mut props = HashMap::new();
        props.insert("path".to_string(), json!({"type": "string"}));
        props.insert("content".to_string(), json!({"type": "string"}));

        let tools = vec![(
            "fs",
            Tool {
                name: "store".to_string(),
                description: None,
                input_schema: ToolInputSchema {
                    schema_type: "object".to_string(),
                    properties: Some(props),
                    required: None,
                    additional_properties: None,
                },
            },
        )];

        let wrapped: Vec<(&str, &Tool)> = tools
            .iter()
            .map(|(server, tool)| (*server, tool))
            .collect();
        let selected = ChatSession::select_write_tool_name(&wrapped);
        assert_eq!(selected.as_deref(), Some("store"));
    }
}
