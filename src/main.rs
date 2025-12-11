mod chat;
mod ffi;
mod file_detector;
mod file_ops;
mod llm;
mod prompt_builder;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "rkllm-cli")]
#[command(about = "RKLLM CLI - Chat with LLM models on Rockchip NPU", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start an interactive chat session with the model
    Chat {
        /// Path to the RKLLM model file
        #[arg(short, long)]
        model: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Chat { model } => {
            if !model.exists() {
                eprintln!("Error: Model file not found: {}", model.display());
                std::process::exit(1);
            }

            let model_path = model
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid model path"))?
                .to_string();

            println!("Loading model: {}", model_path);
            println!("Initializing RKLLM...");

            let session = chat::ChatSession::new(model_path)?;

            println!("Model loaded successfully!");
            println!();

            session.start()?;
        }
    }

    Ok(())
}
