// stdio transport for MCP
// Based on Model Context Protocol Specification 2025-06-18
// https://modelcontextprotocol.io/docs/concepts/transports

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use crate::mcp::types::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, RequestId};

/// Default timeout for requests (30 seconds)
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// stdio transport for MCP server communication
///
/// This transport implements the MCP stdio transport specification:
/// - Messages are UTF-8 encoded JSON-RPC 2.0 messages
/// - Messages are delimited by newlines and MUST NOT contain embedded newlines
/// - Server reads from stdin, writes to stdout
/// - Server MAY write logs to stderr
pub struct StdioTransport {
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<ChildStdin>>,
    stdout: Arc<Mutex<BufReader<ChildStdout>>>,
    next_id: Arc<Mutex<i64>>,
    server_name: String,
    request_timeout: Duration,
}

impl StdioTransport {
    /// Create a new stdio transport by spawning an MCP server process
    pub async fn new(
        command: &str,
        args: &[String],
        env: Option<&HashMap<String, String>>,
    ) -> Result<Self> {
        Self::with_timeout(command, args, env, DEFAULT_REQUEST_TIMEOUT).await
    }

    /// Create a new stdio transport with custom timeout
    pub async fn with_timeout(
        command: &str,
        args: &[String],
        env: Option<&HashMap<String, String>>,
        timeout: Duration,
    ) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

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

        // Spawn task to handle stderr logging
        Self::spawn_stderr_logger(stderr, command.to_string());

        Ok(Self {
            child: Arc::new(Mutex::new(child)),
            stdin: Arc::new(Mutex::new(stdin)),
            stdout: Arc::new(Mutex::new(BufReader::new(stdout))),
            next_id: Arc::new(Mutex::new(1)),
            server_name: command.to_string(),
            request_timeout: timeout,
        })
    }

    /// Spawn a task to read and log stderr from the server
    fn spawn_stderr_logger(stderr: ChildStderr, server_name: String) {
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();

            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        // Only print non-empty lines
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            eprintln!("[MCP Server: {}]: {}", server_name, trimmed);
                        }
                    }
                    Err(e) => {
                        eprintln!("[MCP Server: {}] stderr read error: {}", server_name, e);
                        break;
                    }
                }
            }
        });
    }

    /// Generate next request ID
    async fn next_id(&self) -> i64 {
        let mut id = self.next_id.lock().await;
        let current = *id;
        *id += 1;
        current
    }

    /// Send a JSON-RPC request and wait for response
    pub async fn request(
        &self,
        method: impl Into<String>,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse> {
        let method = method.into();
        let id = self.next_id().await;

        let request = JsonRpcRequest::new(method.clone(), params, id);

        // Serialize to single-line JSON (no embedded newlines)
        let request_json = serde_json::to_string(&request)
            .context("Failed to serialize JSON-RPC request")?;

        // Verify no embedded newlines (should never happen with serde_json::to_string)
        debug_assert!(
            !request_json.contains('\n'),
            "JSON-RPC message contains embedded newline"
        );

        // Send request
        {
            let mut stdin = self.stdin.lock().await;
            writeln!(stdin, "{}", request_json)
                .context("Failed to write request to MCP server stdin")?;
            stdin.flush().context("Failed to flush MCP server stdin")?;
        }

        // Wait for response with timeout
        let response = tokio::time::timeout(self.request_timeout, self.read_response(id))
            .await
            .with_context(|| {
                format!(
                    "Timeout waiting for response to '{}' ({}s)",
                    method,
                    self.request_timeout.as_secs()
                )
            })??;

        // Check for JSON-RPC error
        if let Some(error) = &response.error {
            anyhow::bail!(
                "JSON-RPC error (code {}): {}",
                error.code,
                error.message
            );
        }

        Ok(response)
    }

    /// Send a JSON-RPC notification (no response expected)
    pub async fn notify(
        &self,
        method: impl Into<String>,
        params: Option<serde_json::Value>,
    ) -> Result<()> {
        let notification = JsonRpcNotification::new(method.into(), params);

        // Serialize to single-line JSON
        let notification_json = serde_json::to_string(&notification)
            .context("Failed to serialize JSON-RPC notification")?;

        debug_assert!(
            !notification_json.contains('\n'),
            "JSON-RPC notification contains embedded newline"
        );

        let mut stdin = self.stdin.lock().await;
        writeln!(stdin, "{}", notification_json)
            .context("Failed to write notification to MCP server stdin")?;
        stdin
            .flush()
            .context("Failed to flush MCP server stdin")?;

        Ok(())
    }

    /// Read responses from stdout until we get the one with matching ID
    async fn read_response(&self, expected_id: i64) -> Result<JsonRpcResponse> {
        let expected_id = RequestId::Number(expected_id);

        loop {
            let line = {
                let mut stdout = self.stdout.lock().await;
                let mut line = String::new();

                stdout
                    .read_line(&mut line)
                    .context("Failed to read from MCP server stdout")?;

                if line.is_empty() {
                    anyhow::bail!("MCP server closed stdout unexpectedly");
                }

                line
            };

            // Parse JSON-RPC message
            let value: serde_json::Value = serde_json::from_str(&line)
                .with_context(|| format!("Failed to parse JSON-RPC message: {}", line.trim()))?;

            // Check if this is a response (has 'id' field) or notification (no 'id')
            if value.get("id").is_some() {
                // This is a response - parse it
                let response: JsonRpcResponse = serde_json::from_value(value)
                    .context("Failed to parse JSON-RPC response")?;

                // Check if this is the response we're waiting for
                if response.id == expected_id {
                    return Ok(response);
                } else {
                    // Received response for different request - this shouldn't happen
                    // in our usage pattern, but we'll ignore it
                    eprintln!(
                        "[MCP: {}] Warning: Received response for unexpected request ID: {:?}",
                        self.server_name, response.id
                    );
                }
            } else {
                // This is a server-initiated notification - handle it
                self.handle_notification(&value).await;
            }
        }
    }

    /// Handle server-initiated notifications
    async fn handle_notification(&self, value: &serde_json::Value) {
        // Extract method name
        let method = value
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown");

        match method {
            "notifications/progress" => {
                // Progress notification - could display progress
                if let Some(params) = value.get("params") {
                    eprintln!("[MCP: {}] Progress: {:?}", self.server_name, params);
                }
            }
            "notifications/message" => {
                // Message notification - server wants to show something to user
                if let Some(params) = value.get("params") {
                    eprintln!("[MCP: {}] Message: {:?}", self.server_name, params);
                }
            }
            _ => {
                // Unknown notification - log it
                eprintln!(
                    "[MCP: {}] Unknown notification '{}': {:?}",
                    self.server_name, method, value
                );
            }
        }
    }

    /// Cancel a request (send $/cancelRequest notification)
    #[allow(dead_code)]
    pub async fn cancel_request(&self, request_id: RequestId, reason: Option<String>) -> Result<()> {
        let params = serde_json::json!({
            "requestId": request_id,
            "reason": reason,
        });

        self.notify("$/cancelRequest", Some(params)).await
    }

    /// Kill the MCP server process
    #[allow(dead_code)]
    pub async fn kill(&self) -> Result<()> {
        let mut child = self.child.lock().await;
        child.kill().context("Failed to kill MCP server process")?;
        Ok(())
    }

    /// Check if the server process is still running
    pub async fn is_alive(&self) -> bool {
        let mut child = self.child.lock().await;
        child.try_wait().ok().flatten().is_none()
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

    #[test]
    fn test_request_serialization() {
        let request = JsonRpcRequest::new("test", None, 1);
        let json = serde_json::to_string(&request).unwrap();
        // Ensure no embedded newlines
        assert!(!json.contains('\n'));
    }

    #[test]
    fn test_notification_serialization() {
        let notification = JsonRpcNotification::new("test", None);
        let json = serde_json::to_string(&notification).unwrap();
        // Ensure no embedded newlines
        assert!(!json.contains('\n'));
        // Ensure no id field
        assert!(!json.contains(r#""id""#));
    }

    #[tokio::test]
    async fn test_stdio_transport_lifecycle() {
        // Test that we can create and drop a transport
        // This test requires 'cat' command which just echoes
        if cfg!(unix) {
            let result = StdioTransport::new("cat", &[], None).await;
            // Cat will work but won't respond to JSON-RPC, so we just test creation
            assert!(result.is_ok());
            let transport = result.unwrap();
            assert!(transport.is_alive().await);
            // Drop transport
            drop(transport);
        }
    }
}
