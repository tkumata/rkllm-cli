use crate::file_detector;
use crate::file_ops;
use crate::file_output_parser;
use crate::llm::{RKLLMConfig, RKLLM};
use crate::mcp::{McpClient, McpConfig};
use crate::prompt_builder;
use crate::tool_detector::ToolCallDetector;
use anyhow::{Context, Result};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    style::{Color, Print, SetForegroundColor, ResetColor},
    terminal::{self},
};
use std::io::{self, stdout, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub struct ChatSession {
    rkllm: RKLLM,
    mcp_client: Option<McpClient>,
    tool_detector: ToolCallDetector,
    last_ctrl_c: Arc<Mutex<Option<Instant>>>,
}

impl ChatSession {
    pub async fn new(model_path: String, mcp_config_path: Option<PathBuf>) -> Result<Self> {
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
        })
    }

    pub async fn start(&self) -> Result<()> {
        self.print_separator(Color::Rgb { r: 100, g: 149, b: 237 });
        self.print_banner();

        terminal::enable_raw_mode().context("Failed to enable raw mode")?;

        let mut stdout = stdout();

        let result = self.run_chat_loop(&mut stdout).await;

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

            // プロンプトを構築
            let mut prompt = if file_paths.is_empty() {
                // ファイルがない場合はシンプルなプロンプト
                prompt_builder::build_simple_prompt(&trimmed)
            } else {
                // ファイルを読み込む
                println!("\n[Detected files: {}]", file_paths.join(", "));
                let (files, errors) = file_ops::read_files(&file_paths);

                // 成功したファイルを表示
                if !files.is_empty() {
                    println!("[Successfully loaded {} file(s)]", files.len());
                }

                // エラーを表示
                for (path, error) in &errors {
                    eprintln!("[Error loading '{}': {}]", path, error);
                }

                // プロンプトを構築
                prompt_builder::build_prompt(&trimmed, &files, &errors)
            };

            // MCPツール情報をプロンプトに追加
            if let Some(mcp_client) = &self.mcp_client {
                let tools = mcp_client.list_all_tools();
                if !tools.is_empty() {
                    let mut tool_info = String::from("\n## Available Tools\n\n");
                    tool_info.push_str("You have access to the following tools. Use them when appropriate:\n\n");

                    for (_server_name, tool) in &tools {
                        tool_info.push_str(&format!("### {}\n", tool.name));
                        if let Some(desc) = &tool.description {
                            tool_info.push_str(&format!("{}\n\n", desc));
                        }

                        // スキーマから引数情報を取得
                        tool_info.push_str("Arguments:\n");
                        if let Some(properties) = &tool.input_schema.properties {
                            for (arg_name, arg_schema) in properties {
                                let arg_type = arg_schema.get("type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("string");
                                let arg_desc = arg_schema.get("description")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");

                                let required_mark = if tool.input_schema.required.as_ref()
                                    .map(|r| r.contains(arg_name))
                                    .unwrap_or(false) {
                                    " (required)"
                                } else {
                                    ""
                                };

                                tool_info.push_str(&format!(
                                    "- `{}` ({}){}: {}\n",
                                    arg_name, arg_type, required_mark, arg_desc
                                ));
                            }
                        }
                        tool_info.push_str("\n");
                    }

                    tool_info.push_str("To use a tool, output:\n\n");
                    tool_info.push_str("[TOOL_CALL]\n");
                    tool_info.push_str("{\n");
                    tool_info.push_str("  \"name\": \"tool_name\",\n");
                    tool_info.push_str("  \"arguments\": {\n");
                    tool_info.push_str("    \"argument_name\": \"value\"\n");
                    tool_info.push_str("  }\n");
                    tool_info.push_str("}\n");
                    tool_info.push_str("[END_TOOL_CALL]\n\n");

                    // プロンプトの先頭にツール情報を追加
                    prompt = tool_info + &prompt;
                }
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
                    if let Err(e) = self.process_file_operations(&response) {
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
        let mut buffer = String::new();

        loop {
            if event::poll(std::time::Duration::from_millis(100))? {
                if let Event::Key(key_event) = event::read()? {
                    match key_event {
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
                            execute!(stdout, Print("\r\n[Press Ctrl+C again to exit]\r\n❯ "))?;
                            execute!(stdout, Print(&buffer))?;
                        }

                        // Ctrl+D to exit
                        KeyEvent {
                            code: KeyCode::Char('d'),
                            modifiers: KeyModifiers::CONTROL,
                            ..
                        } => {
                            return Ok(None);
                        }

                        // Shift+Enter for newline
                        KeyEvent {
                            code: KeyCode::Enter,
                            modifiers: KeyModifiers::SHIFT,
                            ..
                        } => {
                            buffer.push('\n');
                            execute!(stdout, Print("\r\n  "))?;
                        }

                        // Enter to submit
                        KeyEvent {
                            code: KeyCode::Enter,
                            modifiers: KeyModifiers::NONE,
                            ..
                        } => {
                            execute!(stdout, Print("\r\n"))?;
                            return Ok(Some(buffer));
                        }

                        // Backspace
                        KeyEvent {
                            code: KeyCode::Backspace,
                            ..
                        } => {
                            if buffer.pop().is_some() {
                                execute!(
                                    stdout,
                                    cursor::MoveLeft(1),
                                    terminal::Clear(terminal::ClearType::FromCursorDown)
                                )?;
                            }
                        }

                        // Regular character input
                        KeyEvent {
                            code: KeyCode::Char(c),
                            modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
                            ..
                        } => {
                            buffer.push(c);
                            execute!(stdout, Print(c))?;
                        }

                        _ => {
                            // Ignore other keys
                        }
                    }
                    stdout.flush()?;
                }
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

    /// ファイル上書きの確認を求める
    ///
    /// # 引数
    /// * `path` - ファイルパス
    ///
    /// # 戻り値
    /// ユーザーが'y'を入力した場合はtrue、それ以外はfalse
    fn confirm_overwrite(&self, path: &str) -> Result<bool> {
        print!("\n[File '{}' already exists. Overwrite? (y/N): ", path);
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
    fn process_file_operations(&self, output: &str) -> Result<()> {
        let operations = file_output_parser::parse_file_operations(output);

        if operations.is_empty() {
            return Ok(());
        }

        println!("\n[Detected {} file operation(s)]", operations.len());

        for op in operations {
            match op.operation_type {
                file_output_parser::FileOperationType::Create => {
                    // ファイルが既に存在する場合は確認
                    if file_ops::file_exists(&op.path) {
                        if !self.confirm_overwrite(&op.path)? {
                            println!("[Skipped: {}]", op.path);
                            continue;
                        }
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
