mod chat;
mod config;
mod ffi;
mod file_detector;
mod file_ops;
mod file_output_parser;
mod intent;
mod llm;
mod mcp;
mod prompt_builder;
mod tool_detector;

use anyhow::Result;
use clap::{ArgAction, Parser, Subcommand};
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

        /// Path to MCP configuration file (optional)
        #[arg(long)]
        mcp_config: Option<PathBuf>,

        /// Print the composed prompt before sending it to the model
        #[arg(long)]
        preview_prompt: bool,

        /// Ask confirmation before every file write
        #[arg(long, default_value_t = true, action = ArgAction::Set)]
        confirm_writes: bool,

        /// Disable local file writes and ignore file output markers (MCP tools only)
        #[arg(long)]
        tool_only: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Chat {
            model,
            mcp_config,
            preview_prompt,
            confirm_writes,
            tool_only,
        } => {
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

            let session = chat::ChatSession::new(
                model_path,
                mcp_config,
                preview_prompt,
                confirm_writes,
                tool_only,
            )
            .await?;

            println!("Model loaded successfully!");
            println!();

            session.start().await?;
        }
    }

    Ok(())
}
