use crate::config::AppConfig;
use crate::file_detector;
use crate::file_ops;
use crate::file_output_parser;
use crate::llm::{RKLLMConfig, RKLLM};
use crate::mcp::{McpClient, McpConfig};
use crate::mcp::types::Tool;
use crate::intent::{has_file_operation_intent, prefers_output_only};
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
use serde_json::{self, json};
use std::collections::HashSet;
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

        let session = Self {
            rkllm,
            mcp_client,
            tool_detector: ToolCallDetector::new(),
            last_ctrl_c: Arc::new(Mutex::new(None)),
            preview_prompt,
            confirm_writes,
            tool_only,
            config: AppConfig::load(),
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

            let has_file_op_intent = has_file_operation_intent(&trimmed);
            if self.tool_only && has_file_op_intent {
                println!("\n[tool-only] Local file writes are disabled. Use MCP tools for any file outputs.");
            }

            let file_paths = if has_file_op_intent && !self.config.detect_extensions.is_empty() {
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

                if has_file_op_intent && file_paths.len() >= 2 {
                    let mut iter = file_paths.iter();
                    if let Some(first) = iter.next() {
                        input_candidates.push(first.clone());
                    }
                    for p in iter {
                        output_candidates.push(p.clone());
                    }
                } else if has_file_op_intent && prefers_output_only(&trimmed) {
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

            // プロンプトを構築（system/user/context/tools の4段）
            let prompt = build_chat_prompt(
                &trimmed,
                &files,
                &errors,
                tool_info.as_deref(),
                &output_targets,
                has_file_op_intent,
                !self.tool_only,
            );
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
                Ok(response) => {
                    println!();

                    // ファイル操作を処理（ユーザーに意図がある場合のみ）
                    if has_file_op_intent {
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
                        } else if let Err(e) =
                            self.process_file_operations(&response, &provided_files, &output_targets)
                        {
                            eprintln!("\nError processing file operations: {}", e);
                        }
                    }

                    // MCP ツール呼び出しを処理
                    if self.mcp_client.is_some() {
                        if let Err(e) = self.process_tool_calls(&response).await {
                            eprintln!("\nError processing tool calls: {}", e);
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
                ("██████ ", Color::Red),
                (" ", Color::Reset),
                ("██  ██", Color::Yellow),
                ("  ", Color::Reset),
                ("██      ", Color::Green),
                ("██      ", Color::Green),
                ("██   ██", Color::Cyan),
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
        let line = format!("[Status: {} | MCP: {} | Mode: {}]", status, mcp, mode);
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

    fn build_tool_sample_block(tool: &Tool) -> String {
        let sample_args = Self::build_sample_arguments(tool);
        let sample_call = json!({
            "name": tool.name.clone(),
            "arguments": sample_args,
        });
        let pretty = serde_json::to_string_pretty(&sample_call).unwrap_or_else(|_| "{}".to_string());

        format!("[TOOL_CALL]\n{}\n[END_TOOL_CALL]\n", pretty)
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
        info.push_str("[TOOL_CALL]\n");
        info.push_str("{\n");
        info.push_str("  \"name\": \"tool_name\",\n");
        info.push_str("  \"arguments\": {\n");
        info.push_str("    \"argument_name\": \"value\"\n");
        info.push_str("  }\n");
        info.push_str("}\n");
        info.push_str("[END_TOOL_CALL]\n");

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
    async fn process_tool_calls(&self, output: &str) -> Result<()> {
        let tool_calls = self.tool_detector.detect(output);

        if tool_calls.is_empty() {
            return Ok(());
        }

        println!("\n[Detected {} tool call(s)]", tool_calls.len());

        if let Some(client) = &self.mcp_client {
            for call in tool_calls {
                match client.call_tool(&call.name, call.arguments).await {
                    Ok(result) => {
                        if result.success {
                            println!("\n[Tool '{}' output:]", call.name);
                            println!("{}", result.output);
                        } else {
                            eprintln!("\n[Tool '{}' failed:]", call.name);
                            eprintln!("{}", result.output);
                        }
                    }
                    Err(e) => {
                        eprintln!("\n[Failed to call tool '{}': {}]", call.name, e);
                    }
                }
            }
        }

        Ok(())
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
        assert!(block.contains("[TOOL_CALL]"));
        assert!(block.contains("\"name\": \"echo\""));
        assert!(block.contains("\"arguments\""));
        assert!(block.contains("[END_TOOL_CALL]"));
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
