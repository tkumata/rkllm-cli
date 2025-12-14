use crate::file_detector;
use crate::file_ops;
use crate::file_output_parser;
use crate::llm::{RKLLMConfig, RKLLM};
use crate::prompt_builder;
use anyhow::{Context, Result};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    style::{Color, Print, SetForegroundColor, ResetColor},
    terminal::{self},
};
use std::io::{self, stdout, Write};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub struct ChatSession {
    rkllm: RKLLM,
    last_ctrl_c: Arc<Mutex<Option<Instant>>>,
}

impl ChatSession {
    pub fn new(model_path: String) -> Result<Self> {
        let config = RKLLMConfig {
            model_path,
            ..Default::default()
        };

        let rkllm = RKLLM::new(config).context("Failed to initialize RKLLM")?;

        Ok(Self {
            rkllm,
            last_ctrl_c: Arc::new(Mutex::new(None)),
        })
    }

    pub fn start(&self) -> Result<()> {
        self.print_separator(Color::Rgb { r: 100, g: 149, b: 237 });
        self.print_banner();

        terminal::enable_raw_mode().context("Failed to enable raw mode")?;
        let mut stdout = stdout();

        let result = self.run_chat_loop(&mut stdout);

        terminal::disable_raw_mode().context("Failed to disable raw mode")?;
        println!(); // Final newline

        result
    }

    fn run_chat_loop(&self, stdout: &mut std::io::Stdout) -> Result<()> {
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
            let prompt = if file_paths.is_empty() {
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
                            let num_newlines = buffer.chars().filter(|&c| c == '\n').count();
                            execute!(stdout, Print("\r\n[Press Ctrl+C again to exit]\r\n❯ "))?;
                            self.redraw_buffer(stdout, &buffer, num_newlines)?;
                        }

                        // Ctrl+D to exit
                        KeyEvent {
                            code: KeyCode::Char('d'),
                            modifiers: KeyModifiers::CONTROL,
                            ..
                        } => {
                            return Ok(None);
                        }

                        // Shift+Enter (detected as Ctrl+J) for newline
                        KeyEvent {
                            code: KeyCode::Char('j'),
                            modifiers: KeyModifiers::CONTROL,
                            ..
                        } => {
                            buffer.push('\n');
                            execute!(stdout, Print("\r\n  "))?;
                        }

                        // Enter to submit
                        KeyEvent {
                            code: KeyCode::Enter,
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
                                // Simply move cursor back and clear from cursor to end
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
                            // Ignore other keys (Left, Right, etc.)
                        }
                    }
                    stdout.flush()?;
                }
            }
        }
    }

    fn redraw_buffer(&self, stdout: &mut std::io::Stdout, buffer: &str, from_line: usize) -> Result<()> {
        // Move up to the first line (where the "> " prompt is)
        if from_line > 0 {
            execute!(stdout, cursor::MoveUp(from_line as u16))?;
        }

        // Move to start of line and clear everything below
        execute!(
            stdout,
            Print("\r"),
            terminal::Clear(terminal::ClearType::FromCursorDown),
            Print("❯ ")
        )?;

        // Print buffer, converting newlines to actual line breaks with indent
        for c in buffer.chars() {
            if c == '\n' {
                execute!(stdout, Print("\r\n  "))?;
            } else {
                execute!(stdout, Print(c))?;
            }
        }

        Ok(())
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
        println!("   Use Shift+Enter for new lines.");

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

}
