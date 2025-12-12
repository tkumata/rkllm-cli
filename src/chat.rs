use crate::file_detector;
use crate::file_ops;
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

pub struct ChatSession {
    rkllm: RKLLM,
}

impl ChatSession {
    pub fn new(model_path: String) -> Result<Self> {
        let config = RKLLMConfig {
            model_path,
            ..Default::default()
        };

        let rkllm = RKLLM::new(config).context("Failed to initialize RKLLM")?;

        Ok(Self { rkllm })
    }

    pub fn start(&self) -> Result<()> {
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
            self.print_separator();
            execute!(stdout, Print("> "))?;

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
                execute!(stdout, Print("\r\nGoodbye!\r\n"))?;
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

            print!("\n🪨 ");
            io::stdout().flush().unwrap();

            match self.rkllm.run(&prompt, |_text| {
                // Text is already printed in the callback
            }) {
                Ok(_) => {
                    println!(); // Add a newline after the response
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
        let mut current_line = 0; // Track which line we're currently on (0-indexed)

        loop {
            if event::poll(std::time::Duration::from_millis(100))? {
                if let Event::Key(key_event) = event::read()? {
                    match key_event {
                        // Ctrl+C or Ctrl+D to exit
                        KeyEvent {
                            code: KeyCode::Char('c'),
                            modifiers: KeyModifiers::CONTROL,
                            ..
                        }
                        | KeyEvent {
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
                            current_line += 1;
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
                                // Use current_line (before deletion) to go back to start
                                self.redraw_buffer(stdout, &buffer, current_line)?;
                                // Update current_line based on new buffer
                                current_line = buffer.chars().filter(|&c| c == '\n').count();
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
            Print("> ")
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
        println!("   Type 'exit' or press Ctrl+C to quit.\n");
    }

    fn print_separator(&self) {
        // Get terminal width, fallback to 80 if unable to detect
        let width = if let Ok((cols, _)) = terminal::size() {
            cols as usize
        } else {
            80
        };

        // Use light blue color (cyan)
        print!("{}", SetForegroundColor(Color::Rgb { r: 100, g: 149, b: 237 }));
        println!("{}", "─".repeat(width));
        print!("{}", ResetColor);
    }

}
