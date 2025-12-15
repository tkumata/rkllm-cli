// MCP Client implementation

use anyhow::{Context, Result};
use std::collections::HashMap;

use super::config::{McpConfig, ServerConfig, Transport};
use super::transport::StdioTransport;
use super::types::*;

/// MCP Server connection
pub struct ServerConnection {
    pub name: String,
    transport: StdioTransport,
    available_tools: Vec<Tool>,
    server_info: Option<ServerInfo>,
}

impl ServerConnection {
    /// Create a new server connection and initialize it
    pub async fn new(config: ServerConfig) -> Result<Self> {
        // Only stdio is supported in Phase 1
        if config.transport != Transport::Stdio {
            anyhow::bail!("Only stdio transport is supported in Phase 1");
        }

        // Create transport
        let transport = StdioTransport::new(&config.command, &config.args, config.env.as_ref())
            .await
            .with_context(|| format!("Failed to create transport for server '{}'", config.name))?;

        let mut connection = Self {
            name: config.name.clone(),
            transport,
            available_tools: Vec::new(),
            server_info: None,
        };

        // Initialize the MCP connection
        connection.initialize().await?;

        // List available tools
        connection.refresh_tools().await?;

        Ok(connection)
    }

    /// Initialize the MCP connection
    async fn initialize(&mut self) -> Result<()> {
        let params = InitializeParams {
            protocol_version: "2025-03-26".to_string(),
            capabilities: ClientCapabilities {
                roots: Some(RootsCapability {
                    list_changed: false,
                }),
                sampling: None,
            },
            client_info: ClientInfo {
                name: "rkllm-cli".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };

        let response = self
            .transport
            .request("initialize".to_string(), Some(serde_json::to_value(params)?))
            .await
            .context("Failed to send initialize request")?;

        if let Some(result) = response.result {
            let init_result: InitializeResult = serde_json::from_value(result)
                .context("Failed to parse initialize response")?;

            self.server_info = Some(init_result.server_info);

            println!(
                "[MCP: Connected to server '{}' ({})]",
                self.name,
                self.server_info
                    .as_ref()
                    .map(|info| info.name.as_str())
                    .unwrap_or("unknown")
            );
        }

        // Send initialized notification
        self.transport
            .notify("notifications/initialized".to_string(), None)
            .await?;

        Ok(())
    }

    /// Refresh the list of available tools
    async fn refresh_tools(&mut self) -> Result<()> {
        // Send empty object instead of null for params
        let response = self
            .transport
            .request("tools/list".to_string(), Some(serde_json::json!({})))
            .await
            .context("Failed to list tools")?;

        if let Some(result) = response.result {
            let list_result: ListToolsResult =
                serde_json::from_value(result).context("Failed to parse tools/list response")?;

            self.available_tools = list_result.tools;

            if !self.available_tools.is_empty() {
                println!(
                    "[MCP: Server '{}' provides {} tool(s)]",
                    self.name,
                    self.available_tools.len()
                );
                for tool in &self.available_tools {
                    println!(
                        "  - {} : {}",
                        tool.name,
                        tool.description.as_deref().unwrap_or("(no description)")
                    );
                }
            }
        }

        Ok(())
    }

    /// Call a tool on this server
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<CallToolResult> {
        let params = CallToolParams {
            name: name.to_string(),
            arguments: Some(arguments),
        };

        let response = self
            .transport
            .request("tools/call".to_string(), Some(serde_json::to_value(params)?))
            .await
            .with_context(|| format!("Failed to call tool '{}'", name))?;

        if let Some(result) = response.result {
            let call_result: CallToolResult =
                serde_json::from_value(result).context("Failed to parse tools/call response")?;
            Ok(call_result)
        } else {
            anyhow::bail!("Tool call returned no result");
        }
    }

    /// Get available tools
    pub fn tools(&self) -> &[Tool] {
        &self.available_tools
    }
}

/// MCP Client managing multiple server connections
pub struct McpClient {
    servers: HashMap<String, ServerConnection>,
}

impl McpClient {
    /// Create a new MCP client from configuration
    pub async fn new(config: McpConfig) -> Result<Self> {
        let mut servers = HashMap::new();

        for server_config in config.servers {
            let name = server_config.name.clone();
            match ServerConnection::new(server_config).await {
                Ok(connection) => {
                    servers.insert(name.clone(), connection);
                }
                Err(e) => {
                    eprintln!("[MCP: Failed to connect to server '{}': {}]", name, e);
                    // Continue with other servers
                }
            }
        }

        if servers.is_empty() {
            println!("[MCP: No servers connected]");
        } else {
            println!("[MCP: Successfully connected to {} server(s)]", servers.len());
        }

        Ok(Self { servers })
    }

    /// Get all available tools from all servers
    pub fn list_all_tools(&self) -> Vec<(&str, &Tool)> {
        self.servers
            .iter()
            .flat_map(|(server_name, conn)| {
                conn.tools()
                    .iter()
                    .map(move |tool| (server_name.as_str(), tool))
            })
            .collect()
    }

    /// Call a tool by name (searches across all servers)
    pub async fn call_tool(&self, name: &str, arguments: serde_json::Value) -> Result<ToolResult> {
        // Find which server has this tool
        for (server_name, connection) in &self.servers {
            if connection.tools().iter().any(|t| t.name == name) {
                println!("[MCP: Calling tool '{}' on server '{}']", name, server_name);

                match connection.call_tool(name, arguments).await {
                    Ok(result) => {
                        // Extract text from content
                        let mut output = String::new();
                        for content in result.content {
                            match content {
                                ToolContent::Text { text } => {
                                    output.push_str(&text);
                                    output.push('\n');
                                }
                                ToolContent::Image { .. } => {
                                    output.push_str("[Image content]\n");
                                }
                                ToolContent::Resource { .. } => {
                                    output.push_str("[Resource content]\n");
                                }
                            }
                        }

                        let success = !result.is_error.unwrap_or(false);

                        if success {
                            println!("[MCP: Tool '{}' completed successfully]", name);
                        } else {
                            println!("[MCP: Tool '{}' returned an error]", name);
                        }

                        return Ok(ToolResult {
                            name: name.to_string(),
                            success,
                            output,
                        });
                    }
                    Err(e) => {
                        return Ok(ToolResult {
                            name: name.to_string(),
                            success: false,
                            output: format!("Error: {}", e),
                        });
                    }
                }
            }
        }

        anyhow::bail!("Tool '{}' not found on any connected server", name)
    }

    /// Check if client has any servers connected
    pub fn has_servers(&self) -> bool {
        !self.servers.is_empty()
    }

    /// Get number of connected servers
    pub fn server_count(&self) -> usize {
        self.servers.len()
    }
}
