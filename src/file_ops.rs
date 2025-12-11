use anyhow::{anyhow, Context, Result};
use path_absolutize::*;
use std::fs;
use std::path::{Path, PathBuf};

/// ファイル読み込みの最大サイズ（1MB）
const MAX_FILE_SIZE: u64 = 1_048_576;

/// ファイル読み込みの結果
#[derive(Debug, Clone)]
pub struct FileContent {
    /// ファイルパス（解決後の絶対パス）
    #[allow(dead_code)]
    pub path: PathBuf,
    /// ファイルの内容
    pub content: String,
    /// 元のパス（ユーザーが指定したパス）
    pub original_path: String,
}

/// ファイルパスを解決する
///
/// # 引数
/// * `path` - 元のファイルパス（相対パス、絶対パス、~を含むパス）
///
/// # 戻り値
/// 解決された絶対パス
///
/// # エラー
/// パスの解決に失敗した場合
fn resolve_path(path: &str) -> Result<PathBuf> {
    // ~ を展開
    let expanded = shellexpand::tilde(path);
    let path = Path::new(expanded.as_ref());

    // 絶対パスに変換
    let absolute = path
        .absolutize()
        .context("Failed to resolve absolute path")?;

    Ok(absolute.to_path_buf())
}

/// ファイルがテキストファイルかどうかを判定
///
/// # 引数
/// * `path` - ファイルパス
///
/// # 戻り値
/// テキストファイルの場合true
fn is_text_file(path: &Path) -> bool {
    // MIMEタイプで判定
    match mime_guess::from_path(path).first() {
        Some(mime) => {
            // text/* または特定の開発関連ファイル
            mime.type_() == "text"
                || mime.subtype() == "json"
                || mime.subtype() == "xml"
                || mime.subtype() == "yaml"
                || mime.subtype() == "toml"
        }
        None => {
            // 拡張子がない場合は、よくあるテキストファイルの拡張子をチェック
            if let Some(ext) = path.extension() {
                matches!(
                    ext.to_str().unwrap_or(""),
                    "txt" | "md" | "rs" | "py" | "js" | "ts" | "json" | "toml"
                    | "yaml" | "yml" | "xml" | "html" | "css" | "sh" | "bash"
                    | "c" | "cpp" | "h" | "hpp" | "java" | "go" | "rb" | "php"
                    | "swift" | "kt" | "cs" | "scala" | "r" | "sql" | "dockerfile"
                    | "gitignore" | "env" | "config" | "conf" | "log"
                )
            } else {
                false
            }
        }
    }
}

/// ファイルを読み込む
///
/// # 引数
/// * `path` - ファイルパス（相対パス、絶対パス、~を含むパス）
///
/// # 戻り値
/// ファイルの内容
///
/// # エラー
/// - ファイルが存在しない
/// - 読み込み権限がない
/// - ファイルサイズが大きすぎる（1MB以上）
/// - UTF-8でデコードできない（バイナリファイル）
pub fn read_file(path: &str) -> Result<FileContent> {
    // パスを解決
    let resolved_path = resolve_path(path)
        .with_context(|| format!("Failed to resolve path: {}", path))?;

    // ファイルの存在確認
    if !resolved_path.exists() {
        return Err(anyhow!("File not found: {}", path));
    }

    // ディレクトリでないことを確認
    if resolved_path.is_dir() {
        return Err(anyhow!("Path is a directory, not a file: {}", path));
    }

    // テキストファイルかどうかを判定
    if !is_text_file(&resolved_path) {
        return Err(anyhow!(
            "File is not a text file (binary files are not supported): {}",
            path
        ));
    }

    // ファイルサイズの確認
    let metadata = fs::metadata(&resolved_path)
        .with_context(|| format!("Failed to read file metadata: {}", path))?;

    if metadata.len() > MAX_FILE_SIZE {
        return Err(anyhow!(
            "File is too large (max {} bytes): {} bytes",
            MAX_FILE_SIZE,
            metadata.len()
        ));
    }

    // ファイルを読み込む
    let content = fs::read_to_string(&resolved_path)
        .with_context(|| format!("Failed to read file (not UTF-8 encoded?): {}", path))?;

    Ok(FileContent {
        path: resolved_path,
        content,
        original_path: path.to_string(),
    })
}

/// 複数のファイルを読み込む
///
/// # 引数
/// * `paths` - ファイルパスのリスト
///
/// # 戻り値
/// 成功したファイルの内容と、失敗したファイルのエラーメッセージのタプル
pub fn read_files(paths: &[String]) -> (Vec<FileContent>, Vec<(String, String)>) {
    let mut successes = Vec::new();
    let mut errors = Vec::new();

    for path in paths {
        match read_file(path) {
            Ok(content) => successes.push(content),
            Err(e) => errors.push((path.clone(), e.to_string())),
        }
    }

    (successes, errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_read_file_success() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        let mut file = File::create(&file_path).unwrap();
        writeln!(file, "Hello, World!").unwrap();

        let result = read_file(file_path.to_str().unwrap());
        assert!(result.is_ok());
        let content = result.unwrap();
        assert_eq!(content.content.trim(), "Hello, World!");
    }

    #[test]
    fn test_read_file_not_found() {
        let result = read_file("/nonexistent/file.txt");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("File not found"));
    }

    #[test]
    fn test_is_text_file() {
        assert!(is_text_file(Path::new("test.txt")));
        assert!(is_text_file(Path::new("test.rs")));
        assert!(is_text_file(Path::new("test.json")));
        assert!(is_text_file(Path::new("Cargo.toml")));
        assert!(!is_text_file(Path::new("test.exe")));
        assert!(!is_text_file(Path::new("test.bin")));
    }
}
