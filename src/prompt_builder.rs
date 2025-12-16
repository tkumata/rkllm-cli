use crate::file_ops::FileContent;

/// システム向けの基本方針
const SYSTEM_INSTRUCTIONS: &str = r#"
You are a helpful coding assistant running on a local CLI. Always reply in Japanese.
The <files> section is read-only context. Do NOT echo it back. Only create or modify files the user explicitly asked for.
When the user asks for translation/summarization/rewriting, transform the content accordingly. Do NOT copy the input verbatim unless explicitly instructed.
If output targets are provided, write results to those paths and do not overwrite the source file unless the user says so.
Use available MCP tools for environment actions (e.g., listing files) instead of fabricating content when tools are provided.
Use the output format <file path="..."> ... </file> for file writes. Do not overwrite input files unless explicitly permitted.
"#;

/// ファイル操作の指示（システムプロンプトの補足）
const FILE_OPERATION_INSTRUCTIONS: &str = r#"
## File Operation Instructions

IMPORTANT: Only use this file creation feature when the user EXPLICITLY requests to create, write, or save files.
Do NOT create example files unless specifically asked.

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

/// ユーザー入力にファイル操作の意図が含まれているかを判定
///
/// # 引数
/// * `input` - ユーザーの入力テキスト
///
/// # 戻り値
/// ファイル操作の意図が含まれている場合はtrue
pub fn has_file_operation_intent(input: &str) -> bool {
    let input_lower = input.to_lowercase();

    // ファイル作成・書き込みを示す強いキーワード（これらは単独で判定）
    let strong_keywords = [
        // 日本語
        "作成", "作って", "作る", "つくって", "つくる",
        "書き込", "書いて", "書く", "かいて",
        "保存", "ほぞん",
        "生成", "せいせい",
        "出力し", "出力ファイル",
        // 英語
        "create", "write", "save", "generate",
        "make a file", "make file",
    ];

    // ファイル操作のフレーズ（組み合わせ）
    let file_operation_phrases = [
        "ファイルに", "ファイルを作", "ファイルを書", "ファイルを生成", "ファイルを出力",
        "file to", "file and", "to file", "in file", "into file",
        "create file", "write file", "save file", "output file", "generate file",
    ];

    // 強いキーワードまたはファイル操作のフレーズが含まれているかチェック
    strong_keywords.iter().any(|&keyword| input_lower.contains(keyword))
        || file_operation_phrases.iter().any(|&phrase| input_lower.contains(phrase))
}

/// ファイル内容を含むプロンプトを構築する
///
/// # 引数
/// * `user_input` - ユーザーの入力
/// * `files` - 読み込まれたファイルの内容
/// * `errors` - ファイル読み込みエラーのリスト
///
/// # 戻り値
/// 構築されたプロンプト
///
/// # フォーマット
/// ```text
/// <files>
/// <file path="src/main.rs">
/// ファイルの内容...
/// </file>
/// </files>
///
/// <user_input>
/// ユーザーの入力
/// </user_input>
/// ```
pub fn build_prompt(user_input: &str, files: &[FileContent], errors: &[(String, String)]) -> String {
    build_chat_prompt(user_input, files, errors, None, &[])
}

/// シンプルなプロンプトを構築（ファイルなし）
///
/// # 引数
/// * `user_input` - ユーザーの入力
///
/// # 戻り値
/// ユーザー入力を含むプロンプト（ファイル操作の意図がある場合のみインストラクション付き）
pub fn build_simple_prompt(user_input: &str) -> String {
    build_chat_prompt(user_input, &[], &[], None, &[])
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
) -> String {
    let mut prompt = String::new();

    // system
    prompt.push_str("<system>\n");
    prompt.push_str(SYSTEM_INSTRUCTIONS);
    prompt.push_str("\n");
    if has_file_operation_intent(user_input) {
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

    if !output_targets.is_empty() {
        prompt.push_str("<output_targets>\n");
        for target in output_targets {
            prompt.push_str(&format!("<target>{}</target>\n", target));
        }
        prompt.push_str("</output_targets>\n\n");
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
        );
        assert!(prompt.contains("<output_targets>"));
        assert!(prompt.contains("<target>b.txt</target>"));
    }
}
