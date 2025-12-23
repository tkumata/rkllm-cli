use crate::file_ops::FileContent;
use crate::mcp::types::ToolResult;

/// システム向けの基本方針
const SYSTEM_INSTRUCTIONS: &str = r#"
You are a helpful coding assistant running on a local CLI. Always reply in Japanese.
The <files> section is read-only context. Do NOT echo it back. Only create or modify files the user explicitly asked for.
When the user asks for translation/summarization/rewriting, transform the content accordingly. Do NOT copy the input verbatim unless explicitly instructed.
If <files> is provided, you MUST base your answer on it and MUST NOT ignore it.
For translation requests, output only the translated text in the target language and never include the original text.
If output targets are provided, write results to those paths and do not overwrite the source file unless the user says so.
Use available MCP tools for environment actions (e.g., listing files) instead of fabricating content when tools are provided.
If the user does NOT explicitly ask to create/modify/save files, respond normally and NEVER use <file>...</file> blocks.
If <tool_results> is provided, use it as authoritative context and do not request the same tool again.
Never call a tool more than once for the same request; when tool results are available, answer directly.
For local file access, use tool calls in this format:
<tool_call name="read_file">{"path":"path/to/file.txt"}</tool_call>
<tool_call name="write_file">{"path":"path/to/file.txt","content":"..."}</tool_call>
"#;

/// ファイル操作の指示（システムプロンプトの補足）
const FILE_OPERATION_INSTRUCTIONS: &str = r#"
## File Operation Instructions

IMPORTANT: Only use this file creation feature when the user EXPLICITLY requests to create, write, or save files.
Do NOT create example files unless specifically asked.
When creating <file> outputs for translation/summarization/proofreading, the content MUST be the transformed result, not the original input.

When the user explicitly asks you to create or modify files, use the following format:

<file path="path/to/file.ext">
file content here
</file>

Example:
<file path="src/example.rs">
fn main() {
    println!("Hello");
}
</file>

You can create multiple files in a single response.
Preferred format is <file path="..."> ... </file>. Bracket format [CREATE_FILE: ...] ... [END_FILE] is allowed for compatibility only.
"#;

/// tool-only モード時の指示
const TOOL_ONLY_INSTRUCTIONS: &str = r#"
## Tool-only Mode

Local file creation or modification is disabled in this session.
Do not emit <file>...</file> or [CREATE_FILE: ...] blocks. Provide the result directly, and use MCP tools for environment actions instead of local writes.
"#;

#[cfg(test)]
/// ファイル内容を含むプロンプトを構築する（テスト用）
pub fn build_prompt(user_input: &str, files: &[FileContent], errors: &[(String, String)]) -> String {
    use crate::intent::has_file_operation_intent;
    build_chat_prompt(
        user_input,
        files,
        errors,
        None,
        &[],
        has_file_operation_intent(user_input),
        true,
        &[],
    )
}

#[cfg(test)]
/// シンプルなプロンプトを構築（テスト用、ファイルなし）
pub fn build_simple_prompt(user_input: &str) -> String {
    use crate::intent::has_file_operation_intent;
    build_chat_prompt(
        user_input,
        &[],
        &[],
        None,
        &[],
        has_file_operation_intent(user_input),
        true,
        &[],
    )
}

/// 役割分離版のチャットプロンプトを構築
///
/// - system: 基本方針 + （必要なら）ファイル操作指示
/// - tools: MCPツール情報（任意）
/// - context: <files> ブロック（参照専用）
/// - user_input: ユーザー入力
pub fn build_chat_prompt(
    user_input: &str,
    files: &[FileContent],
    errors: &[(String, String)],
    tool_info: Option<&str>,
    output_targets: &[String],
    has_file_op_intent: bool,
    file_writes_enabled: bool,
    tool_results: &[ToolResult],
) -> String {
    let mut prompt = String::new();

    // system
    prompt.push_str("<system>\n");
    prompt.push_str(SYSTEM_INSTRUCTIONS);
    prompt.push_str("\n");
    if !file_writes_enabled {
        prompt.push_str(TOOL_ONLY_INSTRUCTIONS);
        prompt.push_str("\n");
    } else if has_file_op_intent {
        prompt.push_str(FILE_OPERATION_INSTRUCTIONS);
        prompt.push_str("\n");
    }
    prompt.push_str("</system>\n\n");

    // tools
    if let Some(info) = tool_info {
        if !info.trim().is_empty() {
            prompt.push_str("<tools>\n");
            prompt.push_str(info.trim());
            prompt.push_str("\n</tools>\n\n");
        }
    }

    // context files
    if !files.is_empty() || !errors.is_empty() {
        prompt.push_str("<files>\n");

        for file in files {
            prompt.push_str(&format!(
                "<file path=\"{}\">\n{}\n</file>\n\n",
                file.original_path, file.content
            ));
        }

        for (path, error) in errors {
            prompt.push_str(&format!(
                "<file_error path=\"{}\">\n{}\n</file_error>\n\n",
                path, error
            ));
        }

        prompt.push_str("</files>\n\n");
    }

    if file_writes_enabled && !output_targets.is_empty() {
        prompt.push_str("<output_targets>\n");
        for target in output_targets {
            prompt.push_str(&format!("<target>{}</target>\n", target));
        }
        prompt.push_str("</output_targets>\n\n");
    }

    if !tool_results.is_empty() {
        prompt.push_str("<tool_results>\n");
        for result in tool_results {
            let success = if result.success { "true" } else { "false" };
            prompt.push_str(&format!(
                "<tool_result name=\"{}\" success=\"{}\">\n{}\n</tool_result>\n\n",
                result.name, success, result.output
            ));
        }
        prompt.push_str("</tool_results>\n\n");
    }

    // user input
    prompt.push_str("<user_input>\n");
    prompt.push_str(user_input);
    prompt.push_str("\n</user_input>");

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent::has_file_operation_intent;

    #[test]
    fn test_has_file_operation_intent_japanese() {
        // ファイル操作の意図がある
        assert!(has_file_operation_intent("test.txtを作成して"));
        assert!(has_file_operation_intent("ファイルを書いて"));
        assert!(has_file_operation_intent("ファイルを作成"));
        assert!(has_file_operation_intent("コードを生成してください"));
        assert!(has_file_operation_intent("結果を保存して"));

        // ファイル操作の意図がない
        assert!(!has_file_operation_intent("こんにちは"));
        assert!(!has_file_operation_intent("日本の首都は？"));
        assert!(!has_file_operation_intent("これは何ですか？"));
        assert!(!has_file_operation_intent("ファイルを要約して"));
        assert!(!has_file_operation_intent("ファイルを読んで"));
        assert!(!has_file_operation_intent("このファイルは何？"));
    }

    #[test]
    fn test_has_file_operation_intent_english() {
        // ファイル操作の意図がある
        assert!(has_file_operation_intent("create a file"));
        assert!(has_file_operation_intent("write to example.txt"));
        assert!(has_file_operation_intent("generate code"));
        assert!(has_file_operation_intent("save the output"));
        assert!(has_file_operation_intent("create file test.txt"));

        // ファイル操作の意図がない
        assert!(!has_file_operation_intent("hello"));
        assert!(!has_file_operation_intent("what is this?"));
        assert!(!has_file_operation_intent("summarize the file"));
        assert!(!has_file_operation_intent("read the file"));
    }

    #[test]
    fn test_build_prompt_with_files() {
        let files = vec![FileContent {
            content: "Hello, World!".to_string(),
            original_path: "test.txt".to_string(),
        }];

        let errors = vec![];
        let prompt = build_prompt("ファイルを要約して", &files, &errors);

        assert!(prompt.contains("<system>"));
        assert!(prompt.contains("<files>"));
        assert!(prompt.contains("<file path=\"test.txt\">"));
        assert!(prompt.contains("Hello, World!"));
        assert!(prompt.contains("</file>"));
        assert!(prompt.contains("</files>"));
        assert!(prompt.contains("<user_input>"));
        assert!(prompt.contains("ファイルを要約して"));
        assert!(prompt.contains("</user_input>"));
        // ファイル操作の意図がないので、インストラクションは含まれない
        assert!(!prompt.contains("File Operation Instructions"));
    }

    #[test]
    fn test_build_prompt_with_errors() {
        let files = vec![];
        let errors = vec![("test.txt".to_string(), "File not found".to_string())];
        let prompt = build_prompt("ファイルを読んで", &files, &errors);

        assert!(prompt.contains("<files>"));
        assert!(prompt.contains("<file_error path=\"test.txt\">"));
        assert!(prompt.contains("File not found"));
        assert!(prompt.contains("</file_error>"));
        assert!(prompt.contains("</files>"));
    }

    #[test]
    fn test_build_prompt_no_files() {
        let files = vec![];
        let errors = vec![];
        let prompt = build_prompt("日本の首都は？", &files, &errors);

        assert!(prompt.contains("<system>"));
        assert!(prompt.contains("<user_input>"));
        assert!(prompt.contains("日本の首都は？"));
        assert!(prompt.contains("</user_input>"));
        // ファイル操作の意図がないので、インストラクションは含まれない
        assert!(!prompt.contains("File Operation Instructions"));
    }

    #[test]
    fn test_build_simple_prompt_without_file_intent() {
        let prompt = build_simple_prompt("こんにちは");
        assert!(prompt.contains("<system>"));
        assert!(prompt.contains("<user_input>"));
        assert!(prompt.contains("こんにちは"));
        // ファイル操作の意図がないので、インストラクションは含まれない
        assert!(!prompt.contains("File Operation Instructions"));
    }

    #[test]
    fn test_build_simple_prompt_with_file_intent() {
        let prompt = build_simple_prompt("test.txtを作成して");
        assert!(prompt.contains("<system>"));
        assert!(prompt.contains("test.txtを作成して"));
        // ファイル操作の意図があるので、インストラクションが含まれる
        assert!(prompt.contains("File Operation Instructions"));
    }

    #[test]
    fn test_output_targets_included() {
        let files = vec![FileContent {
            content: "Hello".to_string(),
            original_path: "a.txt".to_string(),
        }];
        let prompt = build_chat_prompt(
            "翻訳して b.txt に保存",
            &files,
            &[],
            None,
            &["b.txt".to_string()],
            true,
            true,
            &[],
        );
        assert!(prompt.contains("<output_targets>"));
        assert!(prompt.contains("<target>b.txt</target>"));
    }

    #[test]
    fn test_tool_only_instructions_and_no_output_targets() {
        let prompt = build_chat_prompt(
            "test.txtを作成して",
            &[],
            &[],
            None,
            &["test.txt".to_string()],
            true,
            false,
            &[],
        );

        assert!(prompt.contains("Tool-only Mode"));
        assert!(!prompt.contains("<output_targets>"));
        assert!(!prompt.contains("File Operation Instructions"));
    }
}
