// MCP (Model Context Protocol) module
//
// This module provides MCP client functionality for RKLLM CLI.

pub mod config;
pub mod types;
pub mod transport;
pub mod client;

pub use config::McpConfig;
pub use client::McpClient;
