use crate::file_detector;
use crate::file_ops;
use crate::file_output_parser;
use crate::llm::{RKLLMConfig, RKLLM};
use crate::mcp::{McpClient, McpConfig};
use crate::prompt_builder::{build_chat_prompt, has_file_operation_intent};
use crate::tool_detector::ToolCallDetector;
use anyhow::{Context, Result};
use crossterm::{
    cursor,
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    style::{Color, Print, SetForegroundColor, ResetColor},
    terminal::{self},
};
use std::io::{self, stdout, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub struct ChatSession {
    rkllm: RKLLM,
    mcp_client: Option<McpClient>,
    tool_detector: ToolCallDetector,
    last_ctrl_c: Arc<Mutex<Option<Instant>>>,
    preview_prompt: bool,
    confirm_writes: bool,
}

impl ChatSession {
    pub async fn new(
        model_path: String,
        mcp_config_path: Option<PathBuf>,
        preview_prompt: bool,
        confirm_writes: bool,
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

        Ok(Self {
            rkllm,
            mcp_client,
            tool_detector: ToolCallDetector::new(),
            last_ctrl_c: Arc::new(Mutex::new(None)),
            preview_prompt,
            confirm_writes,
        })
    }

    pub async fn start(&self) -> Result<()> {
        self.print_separator(Color::Rgb { r: 100, g: 149, b: 237 });
        self.print_banner();

        terminal::enable_raw_mode().context("Failed to enable raw mode")?;

        let mut stdout = stdout();
        execute!(stdout, EnableBracketedPaste).context("Failed to enable bracketed paste")?;

        let result = self.run_chat_loop(&mut stdout).await;

        execute!(stdout, DisableBracketedPaste).context("Failed to disable bracketed paste")?;
        terminal::disable_raw_mode().context("Failed to disable raw mode")?;
        println!(); // Final newline

        result
    }

    async fn run_chat_loop(&self, stdout: &mut std::io::Stdout) -> Result<()> {
        loop {
            // Display prompt
            self.print_separator(Color::Rgb { r: 100, g: 149, b: 237 });
            execute!(stdout, Print("❯ "))?;

            // Read multiline input
            let input = match self.read_multiline_input(stdout)? {
                Some(text) => text,
                None => break, // User pressed Ctrl+C or Ctrl+D
            };

            let trimmed = input.trim();

            if trimmed.is_empty() {
                continue;
            }

            if trimmed.eq_ignore_ascii_case("exit") || trimmed.eq_ignore_ascii_case("quit") {
                execute!(stdout, Print("\r\nSee you!\r\n"))?;
                break;
            }

            // Disable raw mode temporarily for LLM output
            terminal::disable_raw_mode().context("Failed to disable raw mode")?;

            // ファイルパスを検出
            let file_paths = file_detector::detect_file_paths(&trimmed);
            let has_file_op_intent = has_file_operation_intent(&trimmed);

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
                } else if has_file_op_intent && likely_output_only(&trimmed) {
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
                    println!("[Treating as output targets (not loaded): {}]", output_targets.join(", "));
                }
            }

            let tool_info = self.build_tool_info();

            // プロンプトを構築（system/user/context/tools の4段）
            let prompt =
                build_chat_prompt(&trimmed, &files, &errors, tool_info.as_deref(), &output_targets);
            if self.preview_prompt || std::env::var("RKLLM_DEBUG_PROMPT").is_ok() {
                eprintln!("\n[DEBUG prompt length={}]", prompt.len());
                eprintln!("{}", prompt);
            }

            self.print_separator(Color::Rgb { r: 100, g: 100, b: 100 });
            print!("\n🔹 ");
            io::stdout().flush().unwrap();

            match self.rkllm.run(&prompt, |_text| {
                // Text is already printed in the callback
            }) {
                Ok(response) => {
                    println!(); // Add a newline after the response

                    // ファイル操作を処理
                    if let Err(e) = self.process_file_operations(&response, &provided_files, &output_targets) {
                        eprintln!("\nError processing file operations: {}", e);
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

            // Re-enable raw mode for next input
            terminal::enable_raw_mode().context("Failed to enable raw mode")?;
        }

        Ok(())
    }

    fn read_multiline_input(&self, stdout: &mut std::io::Stdout) -> Result<Option<String>> {
        const PROMPT: &str = "❯ ";
        const INDENT: &str = "  ";
        let prompt_width = UnicodeWidthStr::width(PROMPT);
        let indent_width = UnicodeWidthStr::width(INDENT);

        // プロンプト行を起点に、毎回カーソルを戻して全体再描画する。
        let mut rendered_rows: usize = 1; // プロンプトのみの1行
        let mut buffer = String::new();

        let redraw = |stdout: &mut std::io::Stdout,
                      rendered_rows: &mut usize,
                      buffer: &str|
         -> Result<()> {
            // 前回の表示開始位置（プロンプト行の先頭）に戻る
            execute!(stdout, cursor::MoveToColumn(0))?;
            if *rendered_rows > 1 {
                execute!(stdout, cursor::MoveUp((*rendered_rows - 1) as u16))?;
            }
            let rows_used = render_input(stdout, PROMPT, INDENT, prompt_width, indent_width, buffer)?;
            *rendered_rows = rows_used;
            Ok(())
        };

        loop {
            if event::poll(std::time::Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(key_event) => match key_event {
                        // Ctrl+C - need to press twice within 2 seconds to exit
                        KeyEvent {
                            code: KeyCode::Char('c'),
                            modifiers: KeyModifiers::CONTROL,
                            ..
                        } => {
                            let now = Instant::now();
                            let mut last_time = self.last_ctrl_c.lock().unwrap();

                            if let Some(last) = *last_time {
                                // Check if within 2 seconds
                                if now.duration_since(last).as_secs() < 2 {
                                    return Ok(None);
                                }
                            }

                            // First Ctrl+C or timeout - show message and update time
                            *last_time = Some(now);
                            execute!(stdout, Print("\r\n[Press Ctrl+C again to exit]\r\n"))?;
                            execute!(stdout, Print(PROMPT))?;
                            rendered_rows = 1;
                            redraw(stdout, &mut rendered_rows, &buffer)?;
                        }

                        // Ctrl+D to exit
                        KeyEvent {
                            code: KeyCode::Char('d'),
                            modifiers: KeyModifiers::CONTROL,
                            ..
                        } => {
                            return Ok(None);
                        }

                        // 一部端末（例: macOS/iTerm2）で Shift+Enter が Ctrl+J として送られるケース
                        KeyEvent {
                            code: KeyCode::Char('j'),
                            modifiers,
                            ..
                        } if modifiers.contains(KeyModifiers::CONTROL) => {
                            buffer.push('\n');
                            redraw(stdout, &mut rendered_rows, &buffer)?;
                        }

                        // Shift+Enter for newline
                        KeyEvent {
                            code: KeyCode::Enter,
                            modifiers: KeyModifiers::SHIFT,
                            ..
                        } => {
                            buffer.push('\n');
                            redraw(stdout, &mut rendered_rows, &buffer)?;
                        }

                        // Enter to submit
                        KeyEvent {
                            code: KeyCode::Enter,
                            modifiers: KeyModifiers::NONE,
                            ..
                        } => {
                            redraw(stdout, &mut rendered_rows, &buffer)?;
                            execute!(stdout, Print("\r\n"))?;
                            return Ok(Some(buffer));
                        }

                        // Backspace
                        KeyEvent {
                            code: KeyCode::Backspace,
                            ..
                        } => {
                            if pop_last_grapheme_width(&mut buffer).is_some() {
                                redraw(stdout, &mut rendered_rows, &buffer)?;
                            }
                        }

                        // Regular character input
                        KeyEvent {
                            code: KeyCode::Char(c),
                            modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
                            ..
                        } => {
                            buffer.push(c);
                            redraw(stdout, &mut rendered_rows, &buffer)?;
                        }

                        _ => {
                            // Ignore other keys
                        }
                    },
                    Event::Paste(content) => {
                        // Normalize CRLF/CR to LF so表示が潰れない
                        let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
                        buffer.push_str(&normalized);
                        redraw(stdout, &mut rendered_rows, &buffer)?;
                    }
                    _ => {
                        // Ignore other events
                    }
                }
                stdout.flush()?;
            }
        }
    }

    fn print_banner(&self) {
        let cyan = Color::Rgb { r: 135, g: 206, b: 235 }; // Light cyan/sky blue

        print!("{}", SetForegroundColor(cyan));
        print!("▗ ████████ ▖");
        print!("{}", ResetColor);
        println!(" RKLLM Chat CLI");

        print!(" ");
        print!("{}", SetForegroundColor(cyan));
        print!("▚█▙████▟█▞");
        print!("{}", ResetColor);
        println!("  Type your message and press Enter to chat.");

        print!("  ");
        print!("{}", SetForegroundColor(cyan));
        print!("████████");
        print!("{}", ResetColor);
        println!();

        print!("  ");
        print!("{}", SetForegroundColor(cyan));
        print!("▜      ▛");
        print!("{}", ResetColor);
        println!("   Type 'exit' or press Ctrl+C twice to quit.\n");
    }

    fn print_separator(&self, color: Color) {
        // Get terminal width, fallback to 80 if unable to detect
        let width = if let Ok((cols, _)) = terminal::size() {
            cols as usize
        } else {
            80
        };

        print!("{}", SetForegroundColor(color));
        print!("{}", "─".repeat(width));
        print!("{}", ResetColor);
    }

    fn build_tool_info(&self) -> Option<String> {
        // デフォルトは短縮版を表示。環境変数で抑止/詳細化。
        let hide = std::env::var("RKLLM_HIDE_TOOL_LIST").is_ok();
        let show_full = std::env::var("RKLLM_SHOW_TOOL_LIST_FULL").is_ok();
        let show_short = std::env::var("RKLLM_SHOW_TOOL_LIST").is_ok() || (!hide && !show_full);
        if hide || (!show_short && !show_full) {
            return None;
        }

        let Some(mcp_client) = &self.mcp_client else {
            return None;
        };
        let tools = mcp_client.list_all_tools();
        if tools.is_empty() {
            return None;
        }

        let mut info = String::from("\n## Available Tools\n\n");
        if show_full {
            info.push_str("You have access to the following tools. Use them when appropriate:\n\n");
        } else {
            info.push_str("Available tools (short list):\n\n");
        }

        for (_server_name, tool) in &tools {
            info.push_str(&format!("### {}\n", tool.name));
            if let Some(desc) = &tool.description {
                info.push_str(&format!("{}\n", desc));
            }
            if show_full {
                info.push_str("\nArguments:\n");
                if let Some(properties) = &tool.input_schema.properties {
                    for (arg_name, arg_schema) in properties {
                        let arg_type = arg_schema
                            .get("type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("string");
                        let arg_desc = arg_schema
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let required_mark = if tool
                            .input_schema
                            .required
                            .as_ref()
                            .map(|r| r.contains(arg_name))
                            .unwrap_or(false)
                        {
                            " (required)"
                        } else {
                            ""
                        };
                        info.push_str(&format!(
                            "- `{}` ({}){}: {}\n",
                            arg_name, arg_type, required_mark, arg_desc
                        ));
                    }
                }
                info.push_str("\n");
            }
        }

        info.push_str("\nTo use a tool, output:\n\n");
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

}

fn pop_last_grapheme_width(buffer: &mut String) -> Option<usize> {
    let mut iter = buffer.grapheme_indices(true);
    if let Some((idx, grapheme)) = iter.next_back() {
        let width = UnicodeWidthStr::width(grapheme);
        buffer.truncate(idx);
        Some(width)
    } else {
        None
    }
}

fn render_input(
    stdout: &mut std::io::Stdout,
    prompt: &str,
    indent: &str,
    prompt_width: usize,
    indent_width: usize,
    buffer: &str,
) -> Result<usize> {
    let term_width = terminal::size().map(|(w, _)| w as usize).unwrap_or(80).max(1);

    // 先頭に戻して以降をクリア
    execute!(
        stdout,
        cursor::MoveToColumn(0),
        terminal::Clear(terminal::ClearType::FromCursorDown),
        Print(prompt)
    )?;

    let mut col = prompt_width;
    let mut rows_used = 1usize;

    for grapheme in buffer.graphemes(true) {
        if grapheme == "\n" {
            execute!(stdout, Print("\r\n"), Print(indent))?;
            rows_used += 1;
            col = indent_width;
            continue;
        }

        let w = UnicodeWidthStr::width(grapheme).max(1);
        if col + w > term_width {
            execute!(stdout, Print("\r\n"), Print(indent))?;
            rows_used += 1;
            col = indent_width;
        }

        execute!(stdout, Print(grapheme))?;
        col += w;
    }

    execute!(stdout, cursor::MoveToColumn(col as u16))?;
    stdout.flush()?;
    Ok(rows_used)
}

/// 出力専用と推定できるキーワードを含むか判定
fn likely_output_only(input: &str) -> bool {
    let lower = input.to_lowercase();
    let hints = [
        "保存", "書き", "出力", "生成", "作成", "save", "write", "output", "generate", "create",
    ];
    hints.iter().any(|kw| lower.contains(kw))
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
    use super::pop_last_grapheme_width;

    #[test]
    fn pop_ascii_grapheme() {
        let mut buffer = "abc".to_string();
        assert_eq!(pop_last_grapheme_width(&mut buffer), Some(1));
        assert_eq!(buffer, "ab");
    }

    #[test]
    fn pop_fullwidth_grapheme() {
        let mut buffer = "あい".to_string();
        assert_eq!(pop_last_grapheme_width(&mut buffer), Some(2));
        assert_eq!(buffer, "あ");
    }

    #[test]
    fn pop_emoji_grapheme() {
        let mut buffer = "ok😊".to_string();
        assert_eq!(pop_last_grapheme_width(&mut buffer), Some(2));
        assert_eq!(buffer, "ok");
    }

    #[test]
    fn pop_combining_grapheme() {
        let mut buffer = "e\u{0301}".to_string(); // e + combining acute
        assert_eq!(pop_last_grapheme_width(&mut buffer), Some(1));
        assert_eq!(buffer, "");
    }
}
