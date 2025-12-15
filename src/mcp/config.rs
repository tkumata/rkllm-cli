// MCP configuration file handling

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// MCP configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    pub servers: Vec<ServerConfig>,
}

/// Individual MCP server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub name: String,
    #[serde(default = "default_transport")]
    pub transport: Transport,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<std::collections::HashMap<String, String>>,
}

/// Transport type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Stdio,
    #[allow(dead_code)]
    Sse,
    #[allow(dead_code)]
    WebSocket,
}

fn default_transport() -> Transport {
    Transport::Stdio
}

impl McpConfig {
    /// Load MCP configuration from a TOML file
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: McpConfig = toml::from_str(&content)?;
        Ok(config)
    }

    /// Create a default configuration
    pub fn default() -> Self {
        Self {
            servers: Vec::new(),
        }
    }

    /// Check if configuration is empty
    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_load_config() {
        let config_toml = r#"
[[servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]

[[servers]]
name = "weather"
transport = "stdio"
command = "/usr/local/bin/weather-server"
args = []
"#;

        let mut file = NamedTempFile::new().unwrap();
        file.write_all(config_toml.as_bytes()).unwrap();
        file.flush().unwrap();

        let config = McpConfig::load(file.path()).unwrap();
        assert_eq!(config.servers.len(), 2);
        assert_eq!(config.servers[0].name, "filesystem");
        assert_eq!(config.servers[0].command, "npx");
        assert_eq!(config.servers[0].args.len(), 3);
        assert_eq!(config.servers[1].name, "weather");
    }

    #[test]
    fn test_default_config() {
        let config = McpConfig::default();
        assert!(config.is_empty());
    }
}
