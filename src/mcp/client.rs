// MCP Client implementation
// Based on Model Context Protocol Specification 2025-06-18

use anyhow::{Context, Result};
use std::collections::HashMap;

use super::config::{McpConfig, ServerConfig, Transport};
use super::transport::StdioTransport;
use super::types::*;

/// Connection to a single MCP server
pub struct ServerConnection {
    pub name: String,
    transport: StdioTransport,
    _server_info: Implementation,
    capabilities: ServerCapabilities,
    available_tools: Vec<Tool>,
}

impl ServerConnection {
    /// Create a new server connection and initialize it
    ///
    /// This performs the MCP initialization handshake:
    /// 1. Send 'initialize' request
    /// 2. Receive server capabilities and info
    /// 3. Send 'initialized' notification
    /// 4. List available tools (if server supports tools)
    pub async fn new(config: ServerConfig) -> Result<Self> {
        // Only stdio is supported currently
        if config.transport != Transport::Stdio {
            anyhow::bail!(
                "Only stdio transport is supported (server: '{}')",
                config.name
            );
        }

        // Create transport
        let transport = StdioTransport::new(&config.command, &config.args, config.env.as_ref())
            .await
            .with_context(|| format!("Failed to create transport for server '{}'", config.name))?;

        // Perform initialization handshake
        let init_params = InitializeParams::default();

        let response = transport
            .request("initialize", Some(serde_json::to_value(&init_params)?))
            .await
            .context("Failed to send initialize request")?;

        let init_result: InitializeResult = serde_json::from_value(
            response
                .result
                .context("Initialize response missing result field")?,
        )
        .context("Failed to parse initialize response")?;

        // Validate protocol version
        if init_result.protocol_version != PROTOCOL_VERSION {
            eprintln!(
                "[MCP: {}] Warning: Server uses protocol version {}, we use {}",
                config.name, init_result.protocol_version, PROTOCOL_VERSION
            );
        }

        // Send initialized notification
        transport
            .notify("notifications/initialized", None)
            .await
            .context("Failed to send initialized notification")?;

        println!(
            "[MCP: Connected to '{}' ({} v{})]",
            config.name, init_result.server_info.name, init_result.server_info.version
        );

        let mut connection = Self {
            name: config.name.clone(),
            transport,
            _server_info: init_result.server_info,
            capabilities: init_result.capabilities,
            available_tools: Vec::new(),
        };

        // List tools if server supports them
        if connection.capabilities.tools.is_some() {
            connection
                .refresh_tools()
                .await
                .context("Failed to list tools")?;
        }

        Ok(connection)
    }

    /// Refresh the list of available tools from the server
    async fn refresh_tools(&mut self) -> Result<()> {
        let params = ListToolsParams::default();

        let response = self
            .transport
            .request("tools/list", Some(serde_json::to_value(&params)?))
            .await
            .context("Failed to send tools/list request")?;

        let list_result: ListToolsResult = serde_json::from_value(
            response
                .result
                .context("tools/list response missing result field")?,
        )
        .context("Failed to parse tools/list response")?;

        self.available_tools = list_result.tools;

        if !self.available_tools.is_empty() {
            println!(
                "[MCP: Server '{}' provides {} tool(s)]",
                self.name,
                self.available_tools.len()
            );
            for tool in &self.available_tools {
                let description = tool
                    .description
                    .as_deref()
                    .unwrap_or("(no description)");
                println!("  - {}: {}", tool.name, description);
            }
        }

        // Handle pagination if needed (nextCursor)
        if let Some(cursor) = list_result.next_cursor {
            eprintln!(
                "[MCP: {}] Warning: Server returned pagination cursor, but pagination is not yet implemented. Cursor: {}",
                self.name, cursor
            );
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
            .request("tools/call", Some(serde_json::to_value(&params)?))
            .await
            .with_context(|| format!("Failed to call tool '{}' on server '{}'", name, self.name))?;

        let call_result: CallToolResult = serde_json::from_value(
            response
                .result
                .context("tools/call response missing result field")?,
        )
        .context("Failed to parse tools/call response")?;

        Ok(call_result)
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
    ///
    /// This will attempt to connect to all configured servers.
    /// Servers that fail to connect will be logged and skipped.
    pub async fn new(config: McpConfig) -> Result<Self> {
        let mut servers = HashMap::new();

        for server_config in config.servers {
            let name = server_config.name.clone();
            match ServerConnection::new(server_config).await {
                Ok(connection) => {
                    servers.insert(name, connection);
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
    ///
    /// Returns a list of (server_name, tool) pairs
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

    /// Find which server provides a tool with the given name
    fn find_server_for_tool(&self, tool_name: &str) -> Option<&ServerConnection> {
        for connection in self.servers.values() {
            if connection.tools().iter().any(|t| t.name == tool_name) {
                return Some(connection);
            }
        }
        None
    }

    /// Call a tool by name (searches across all servers)
    ///
    /// This will find the first server that provides a tool with the given name
    /// and execute it on that server.
    pub async fn call_tool(&self, name: &str, arguments: serde_json::Value) -> Result<ToolResult> {
        // Find which server has this tool
        let connection = self
            .find_server_for_tool(name)
            .ok_or_else(|| anyhow::anyhow!("Tool '{}' not found on any connected server", name))?;

        println!("[MCP: Calling tool '{}' on server '{}']", name, connection.name);

        // Call the tool
        let result = connection
            .call_tool(name, arguments)
            .await
            .with_context(|| format!("Failed to execute tool '{}'", name))?;

        // Convert to ToolResult
        let mut tool_result = ToolResult::from(result);
        tool_result.name = name.to_string();

        // Log result
        if tool_result.success {
            println!("[MCP: Tool '{}' completed successfully]", name);
        } else {
            println!("[MCP: Tool '{}' returned an error]", name);
        }

        Ok(tool_result)
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initialize_params_default() {
        let params = InitializeParams::default();
        assert_eq!(params.protocol_version, PROTOCOL_VERSION);
        assert_eq!(params.client_info.name, "rkllm-cli");
    }

    #[test]
    fn test_tool_result_conversion() {
        let call_result = CallToolResult {
            content: vec![
                Content::Text {
                    text: "Hello".to_string(),
                },
                Content::Text {
                    text: "World".to_string(),
                },
            ],
            is_error: Some(false),
        };

        let tool_result = ToolResult::from(call_result);
        assert!(tool_result.success);
        assert!(tool_result.output.contains("Hello"));
        assert!(tool_result.output.contains("World"));
    }

    #[test]
    fn test_tool_result_error() {
        let call_result = CallToolResult {
            content: vec![Content::Text {
                text: "Error message".to_string(),
            }],
            is_error: Some(true),
        };

        let tool_result = ToolResult::from(call_result);
        assert!(!tool_result.success);
        assert!(tool_result.output.contains("Error message"));
    }
}
