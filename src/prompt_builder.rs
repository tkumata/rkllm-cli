use crate::file_ops::FileContent;

/// ファイル操作の指示（システムプロンプト）
const FILE_OPERATION_INSTRUCTIONS: &str = r#"
## File Operation Instructions

When you need to create or modify files, use the following format:

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
"#;

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
pub fn build_prompt(
    user_input: &str,
    files: &[FileContent],
    errors: &[(String, String)],
) -> String {
    let mut prompt = String::new();

    // ファイル操作の指示を追加
    prompt.push_str(FILE_OPERATION_INSTRUCTIONS);
    prompt.push_str("\n");

    // ファイル内容を追加
    if !files.is_empty() || !errors.is_empty() {
        prompt.push_str("<files>\n");

        // 成功したファイルを追加
        for file in files {
            prompt.push_str(&format!(
                "<file path=\"{}\">\n{}\n</file>\n\n",
                file.original_path, file.content
            ));
        }

        // エラーを追加
        for (path, error) in errors {
            prompt.push_str(&format!(
                "<file_error path=\"{}\">\n{}\n</file_error>\n\n",
                path, error
            ));
        }

        prompt.push_str("</files>\n\n");
    }

    // ユーザー入力を追加
    prompt.push_str(&format!("<user_input>\n{}\n</user_input>", user_input));

    prompt
}

/// シンプルなプロンプトを構築（ファイルなし）
///
/// # 引数
/// * `user_input` - ユーザーの入力
///
/// # 戻り値
/// ファイル操作指示とユーザー入力を含むプロンプト
pub fn build_simple_prompt(user_input: &str) -> String {
    let mut prompt = String::new();

    // ファイル操作の指示を追加
    prompt.push_str(FILE_OPERATION_INSTRUCTIONS);
    prompt.push_str("\n");

    // ユーザー入力を追加
    prompt.push_str(user_input);

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_build_prompt_with_files() {
        let files = vec![FileContent {
            path: PathBuf::from("/home/user/test.txt"),
            content: "Hello, World!".to_string(),
            original_path: "test.txt".to_string(),
        }];

        let errors = vec![];
        let prompt = build_prompt("ファイルを要約して", &files, &errors);

        assert!(prompt.contains("<files>"));
        assert!(prompt.contains("<file path=\"test.txt\">"));
        assert!(prompt.contains("Hello, World!"));
        assert!(prompt.contains("</file>"));
        assert!(prompt.contains("</files>"));
        assert!(prompt.contains("<user_input>"));
        assert!(prompt.contains("ファイルを要約して"));
        assert!(prompt.contains("</user_input>"));
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

        assert!(!prompt.contains("<files>"));
        assert!(prompt.contains("<user_input>"));
        assert!(prompt.contains("日本の首都は？"));
        assert!(prompt.contains("</user_input>"));
        assert!(prompt.contains("File Operation Instructions"));
    }

    #[test]
    fn test_build_simple_prompt() {
        let prompt = build_simple_prompt("こんにちは");
        assert!(prompt.contains("こんにちは"));
        assert!(prompt.contains("File Operation Instructions"));
    }
}
