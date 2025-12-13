use regex::Regex;
use std::sync::OnceLock;

/// LLMの出力からファイル操作を抽出する

#[derive(Debug, Clone, PartialEq)]
pub struct FileOperation {
    /// ファイルパス
    pub path: String,
    /// ファイル内容
    pub content: String,
    /// 操作の種類（現在は CREATE のみサポート）
    pub operation_type: FileOperationType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FileOperationType {
    Create,
}

/// XMLスタイルのファイルマーカーパターン: <file path="...">...</file>
fn xml_file_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r#"<file\s+path="([^"]+)"\s*>([\s\S]*?)</file>"#).unwrap()
    })
}

/// ブラケットスタイルのファイルマーカーパターン: [CREATE_FILE: ...]...[ END_FILE]
fn bracket_file_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r#"\[CREATE_FILE:\s*([^\]]+)\]\s*```[a-z]*\n([\s\S]*?)\n```\s*\[END_FILE\]"#)
            .unwrap()
    })
}

/// LLMの出力からファイル操作を抽出する
///
/// # 引数
/// * `output` - LLMの出力テキスト
///
/// # 戻り値
/// 検出されたファイル操作のベクトル
///
/// # サポートされる形式
///
/// 1. XMLスタイル:
/// ```text
/// <file path="src/example.rs">
/// fn main() {
///     println!("Hello");
/// }
/// </file>
/// ```
///
/// 2. ブラケットスタイル:
/// ```text
/// [CREATE_FILE: src/example.rs]
/// ```rust
/// fn main() {
///     println!("Hello");
/// }
/// ```
/// [END_FILE]
/// ```
pub fn parse_file_operations(output: &str) -> Vec<FileOperation> {
    let mut operations = Vec::new();

    // XMLスタイルのマーカーを検出
    let xml_pattern = xml_file_pattern();
    for cap in xml_pattern.captures_iter(output) {
        if let (Some(path), Some(content)) = (cap.get(1), cap.get(2)) {
            operations.push(FileOperation {
                path: path.as_str().trim().to_string(),
                content: content.as_str().to_string(),
                operation_type: FileOperationType::Create,
            });
        }
    }

    // ブラケットスタイルのマーカーを検出
    let bracket_pattern = bracket_file_pattern();
    for cap in bracket_pattern.captures_iter(output) {
        if let (Some(path), Some(content)) = (cap.get(1), cap.get(2)) {
            operations.push(FileOperation {
                path: path.as_str().trim().to_string(),
                content: content.as_str().to_string(),
                operation_type: FileOperationType::Create,
            });
        }
    }

    operations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_xml_style() {
        let output = r#"ファイルを作成します。

<file path="src/example.rs">
fn main() {
    println!("Hello, World!");
}
</file>

以上です。"#;

        let ops = parse_file_operations(output);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].path, "src/example.rs");
        assert!(ops[0].content.contains("fn main()"));
        assert_eq!(ops[0].operation_type, FileOperationType::Create);
    }

    #[test]
    fn test_parse_bracket_style() {
        let output = r#"ファイルを作成します。

[CREATE_FILE: src/helper.rs]
```rust
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}
```
[END_FILE]

以上です。"#;

        let ops = parse_file_operations(output);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].path, "src/helper.rs");
        assert!(ops[0].content.contains("pub fn add"));
        assert_eq!(ops[0].operation_type, FileOperationType::Create);
    }

    #[test]
    fn test_parse_multiple_files() {
        let output = r#"複数のファイルを作成します。

<file path="src/mod1.rs">
pub fn func1() {}
</file>

<file path="src/mod2.rs">
pub fn func2() {}
</file>

以上です。"#;

        let ops = parse_file_operations(output);
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0].path, "src/mod1.rs");
        assert_eq!(ops[1].path, "src/mod2.rs");
    }

    #[test]
    fn test_parse_no_files() {
        let output = "これは普通のテキストです。ファイル操作はありません。";
        let ops = parse_file_operations(output);
        assert_eq!(ops.len(), 0);
    }

    #[test]
    fn test_parse_mixed_styles() {
        let output = r#"両方のスタイルを使います。

<file path="src/xml_style.rs">
fn xml_func() {}
</file>

[CREATE_FILE: src/bracket_style.rs]
```rust
fn bracket_func() {}
```
[END_FILE]

以上です。"#;

        let ops = parse_file_operations(output);
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0].path, "src/xml_style.rs");
        assert_eq!(ops[1].path, "src/bracket_style.rs");
    }
}
