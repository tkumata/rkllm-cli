use crate::llm::{RKLLMConfig, RKLLM};
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

                    print!("\nAssistant: ");
                    io::stdout().flush().unwrap();

                    match self.rkllm.run(input, |_text| {
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
