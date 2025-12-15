// stdio transport for MCP
//
// This module implements the stdio (standard input/output) transport
// for Model Context Protocol communication.

use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::mcp::types::{JsonRpcRequest, JsonRpcResponse};

/// stdio transport for MCP server communication
pub struct StdioTransport {
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<ChildStdin>>,
    stdout: Arc<Mutex<BufReader<ChildStdout>>>,
    stderr: Arc<Mutex<BufReader<ChildStderr>>>,
    next_id: Arc<Mutex<i64>>,
}

impl StdioTransport {
    /// Create a new stdio transport by spawning an MCP server process
    pub async fn new(
        command: &str,
        args: &[String],
        env: Option<&std::collections::HashMap<String, String>>,
    ) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()); // Capture server stderr for debugging

        // Add environment variables if provided
        if let Some(env_vars) = env {
            for (key, value) in env_vars {
                cmd.env(key, value);
            }
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn MCP server: {}", command))?;

        let stdin = child
            .stdin
            .take()
            .context("Failed to capture stdin of MCP server")?;
        let stdout = child
            .stdout
            .take()
            .context("Failed to capture stdout of MCP server")?;
        let stderr = child
            .stderr
            .take()
            .context("Failed to capture stderr of MCP server")?;

        // Spawn a task to read and log stderr
        let stderr_reader = BufReader::new(stderr);
        let stderr_arc = Arc::new(Mutex::new(stderr_reader));
        let stderr_clone = stderr_arc.clone();

        tokio::spawn(async move {
            let mut stderr = stderr_clone.lock().await;
            let mut line = String::new();
            loop {
                line.clear();
                match stderr.read_line(&mut line) {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        eprint!("[MCP Server stderr]: {}", line);
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            child: Arc::new(Mutex::new(child)),
            stdin: Arc::new(Mutex::new(stdin)),
            stdout: Arc::new(Mutex::new(BufReader::new(stdout))),
            stderr: stderr_arc,
            next_id: Arc::new(Mutex::new(1)),
        })
    }

    /// Send a JSON-RPC request and read the response
    pub async fn request(
        &self,
        method: String,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse> {
        // Generate request ID
        let id = {
            let mut next_id = self.next_id.lock().await;
            let id = *next_id;
            *next_id += 1;
            id
        };

        // Create request
        let request = JsonRpcRequest::new(method.clone(), params, id);
        let request_json = serde_json::to_string(&request)?;

        // Send request
        {
            let mut stdin = self.stdin.lock().await;
            writeln!(stdin, "{}", request_json)
                .context("Failed to write request to MCP server stdin")?;
            stdin.flush().context("Failed to flush MCP server stdin")?;
        }

        // Read responses until we get the one with matching ID
        // (server may send notifications in between)
        let response = loop {
            let mut line = String::new();
            {
                let mut stdout = self.stdout.lock().await;

                // Add timeout using tokio
                let read_future = async {
                    stdout
                        .read_line(&mut line)
                        .context("Failed to read response from MCP server stdout")
                };

                match tokio::time::timeout(std::time::Duration::from_secs(10), read_future).await {
                    Ok(Ok(bytes_read)) => {
                        if bytes_read == 0 {
                            anyhow::bail!("MCP server closed stdout before sending response");
                        }
                    }
                    Ok(Err(e)) => {
                        return Err(e);
                    }
                    Err(_) => {
                        anyhow::bail!("Timeout waiting for response from MCP server (10 seconds). Method: {}", method);
                    }
                }
            }

            // Try to parse as JSON-RPC message
            let value: serde_json::Value = serde_json::from_str(&line)
                .with_context(|| format!("Failed to parse JSON-RPC message: {}", line))?;

            // Check if this is a notification (no id field) or a response
            if let Some(response_id) = value.get("id") {
                // This is a response
                if response_id == &serde_json::Value::Number(id.into()) {
                    // This is the response we're waiting for
                    let response: JsonRpcResponse = serde_json::from_value(value)
                        .context("Failed to parse JSON-RPC response")?;

                    // Check for error
                    if let Some(error) = &response.error {
                        anyhow::bail!("MCP server returned error: {}", error.message);
                    }

                    break response;
                } else {
                    // Received response with different ID, continue reading
                }
            } else {
                // This is a notification, ignore and continue reading
            }
        };

        Ok(response)
    }

    /// Send a notification (no response expected)
    pub async fn notify(&self, method: String, params: Option<serde_json::Value>) -> Result<()> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });

        let notification_json = serde_json::to_string(&notification)?;

        let mut stdin = self.stdin.lock().await;
        writeln!(stdin, "{}", notification_json)
            .context("Failed to write notification to MCP server stdin")?;
        stdin
            .flush()
            .context("Failed to flush MCP server stdin")?;

        Ok(())
    }

    /// Kill the MCP server process
    pub async fn kill(&self) -> Result<()> {
        let mut child = self.child.lock().await;
        child.kill().context("Failed to kill MCP server process")?;
        Ok(())
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // Try to kill the child process when dropped
        if let Ok(mut child) = self.child.try_lock() {
            let _ = child.kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_stdio_transport_echo() {
        // This test requires a simple echo MCP server
        // Skip if not available
        if std::env::var("TEST_MCP_ECHO_SERVER").is_err() {
            println!("Skipping test: TEST_MCP_ECHO_SERVER not set");
            return;
        }

        let transport = StdioTransport::new("echo-mcp-server", &[], None)
            .await
            .unwrap();

        let response = transport
            .request("ping".to_string(), None)
            .await
            .unwrap();

        assert!(response.result.is_some());
    }
}
