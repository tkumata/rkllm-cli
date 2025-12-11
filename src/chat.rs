use crate::file_detector;
use crate::file_ops;
use crate::llm::{RKLLMConfig, RKLLM};
use crate::prompt_builder;
use anyhow::{Context, Result};
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::io::{self, Write};

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
        println!("RKLLM Chat CLI");
        println!("Type your message and press Enter to chat.");
        println!("Type 'exit' or press Ctrl+C to quit.");
        println!("─────────────────────────────────────────");

        let mut rl = DefaultEditor::new().context("Failed to create readline editor")?;

        loop {
            let readline = rl.readline("\n> ");

            match readline {
                Ok(line) => {
                    let input = line.trim();

                    if input.is_empty() {
                        continue;
                    }

                    if input.eq_ignore_ascii_case("exit") || input.eq_ignore_ascii_case("quit") {
                        println!("Goodbye!");
                        break;
                    }

                    let _ = rl.add_history_entry(input);

                    // ファイルパスを検出
                    let file_paths = file_detector::detect_file_paths(input);

                    // プロンプトを構築
                    let prompt = if file_paths.is_empty() {
                        // ファイルがない場合はシンプルなプロンプト
                        prompt_builder::build_simple_prompt(input)
                    } else {
                        // ファイルを読み込む
                        println!("\n[Detected files: {}]", file_paths.join(", "));
                        let (files, errors) = file_ops::read_files(&file_paths);

                        // 成功したファイルを表示
                        if !files.is_empty() {
                            println!(
                                "[Successfully loaded {} file(s)]",
                                files.len()
                            );
                        }

                        // エラーを表示
                        for (path, error) in &errors {
                            eprintln!("[Error loading '{}': {}]", path, error);
                        }

                        // プロンプトを構築
                        prompt_builder::build_prompt(input, &files, &errors)
                    };

                    print!("\nAssistant: ");
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
                }
                Err(ReadlineError::Interrupted) => {
                    println!("^C");
                    println!("Goodbye!");
                    break;
                }
                Err(ReadlineError::Eof) => {
                    println!("^D");
                    println!("Goodbye!");
                    break;
                }
                Err(err) => {
                    eprintln!("Error: {:?}", err);
                    break;
                }
            }
        }

        Ok(())
    }
}
