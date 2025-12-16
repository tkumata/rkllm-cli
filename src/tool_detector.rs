// Tool call detection from LLM output

use regex::Regex;
use serde_json::Value;

use crate::mcp::types::ToolCall;

/// Tool call detector that extracts tool calls from LLM output
pub struct ToolCallDetector {
    json_pattern: Regex,
    xml_pattern: Regex,
}

impl ToolCallDetector {
    /// Create a new tool call detector
    pub fn new() -> Self {
        Self {
            // JSON style: [TOOL_CALL] {...} [END_TOOL_CALL]
            json_pattern: Regex::new(
                r"(?s)\[TOOL_CALL\]\s*(\{.*?\})\s*\[END_TOOL_CALL\]"
            ).unwrap(),
            // XML style: <tool_call name="...">...</tool_call>
            xml_pattern: Regex::new(
                r#"<tool_call\s+name="([^"]+)"\s*>([\s\S]*?)</tool_call>"#
            ).unwrap(),
        }
    }

    /// Detect tool calls from text
    pub fn detect(&self, text: &str) -> Vec<ToolCall> {
        let mut calls = Vec::new();

        // Detect JSON style
        calls.extend(self.detect_json_style(text));

        // Detect XML style
        calls.extend(self.detect_xml_style(text));

        calls
    }

    /// Detect JSON-style tool calls
    fn detect_json_style(&self, text: &str) -> Vec<ToolCall> {
        let mut calls = Vec::new();

        for cap in self.json_pattern.captures_iter(text) {
            if let Ok(value) = serde_json::from_str::<Value>(&cap[1]) {
                if let Some(obj) = value.as_object() {
                    if let (Some(name), Some(args)) = (
                        obj.get("name").and_then(|v| v.as_str()),
                        obj.get("arguments"),
                    ) {
                        calls.push(ToolCall {
                            name: name.to_string(),
                            arguments: args.clone(),
                        });
                    }
                }
            }
        }

        calls
    }

    /// Detect XML-style tool calls
    fn detect_xml_style(&self, text: &str) -> Vec<ToolCall> {
        let mut calls = Vec::new();

        for cap in self.xml_pattern.captures_iter(text) {
            let name = cap[1].to_string();
            let args_str = &cap[2];

            // Simple XML argument parsing
            let mut args = serde_json::Map::new();
            let arg_regex =
                Regex::new(r#"<argument\s+name="([^"]+)"\s*>([^<]*)</argument>"#).unwrap();

            for arg_cap in arg_regex.captures_iter(args_str) {
                let arg_name = arg_cap[1].to_string();
                let arg_value = arg_cap[2].to_string();

                // Try to parse as JSON value, otherwise use as string
                let value = match serde_json::from_str::<Value>(&arg_value) {
                    Ok(v) => v,
                    Err(_) => Value::String(arg_value),
                };

                args.insert(arg_name, value);
            }

            calls.push(ToolCall {
                name,
                arguments: Value::Object(args),
            });
        }

        calls
    }
}

impl Default for ToolCallDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_json_style() {
        let detector = ToolCallDetector::new();

        let text = r#"
Let me check the weather for you.

[TOOL_CALL]
{
  "name": "get_weather",
  "arguments": {
    "location": "Tokyo"
  }
}
[END_TOOL_CALL]

I'll get that information for you.
"#;

        let calls = detector.detect(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(
            calls[0].arguments.get("location").and_then(|v| v.as_str()),
            Some("Tokyo")
        );
    }

    #[test]
    fn test_detect_xml_style() {
        let detector = ToolCallDetector::new();

        let text = r#"
I'll search for that file.

<tool_call name="list_files">
  <argument name="directory">/home/user</argument>
  <argument name="pattern">*.rs</argument>
</tool_call>

Here are the results.
"#;

        let calls = detector.detect(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "list_files");
        assert_eq!(
            calls[0].arguments.get("directory").and_then(|v| v.as_str()),
            Some("/home/user")
        );
        assert_eq!(
            calls[0].arguments.get("pattern").and_then(|v| v.as_str()),
            Some("*.rs")
        );
    }

    #[test]
    fn test_detect_multiple_calls() {
        let detector = ToolCallDetector::new();

        let text = r#"
First, I'll check the weather:

[TOOL_CALL]
{"name": "get_weather", "arguments": {"location": "Tokyo"}}
[END_TOOL_CALL]

Then I'll save it to a file:

[TOOL_CALL]
{"name": "write_file", "arguments": {"path": "weather.txt", "content": "sunny"}}
[END_TOOL_CALL]
"#;

        let calls = detector.detect(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[1].name, "write_file");
    }

    #[test]
    fn test_no_tool_calls() {
        let detector = ToolCallDetector::new();

        let text = "This is just regular text without any tool calls.";

        let calls = detector.detect(text);
        assert_eq!(calls.len(), 0);
    }
}
