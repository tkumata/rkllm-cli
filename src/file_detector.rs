use regex::Regex;
use std::sync::OnceLock;

/// ユーザー入力からファイルパスを検出するモジュール

/// ファイルパスの正規表現パターンを取得
/// 以下のパターンに対応:
/// - 相対パス: src/main.rs, ./config.toml
/// - 絶対パス: /home/user/file.txt
/// - ホームディレクトリ: ~/file.txt
/// - ファイル名のみ: README.md, Cargo.toml
fn file_path_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        // ファイルパスのパターン:
        // - オプションで ~/ または / または ./ で開始
        // - または、相対パスとして英数字で開始
        // - その後、ASCII英数字、ハイフン、アンダースコア、ドット、スラッシュが続く
        // - 最後に拡張子（ドット + ASCII英数字）
        // - 日本語などの非ASCII文字は含まない
        Regex::new(r"(?:~/|/|\./)?[a-zA-Z0-9_\-.]+(?:/[a-zA-Z0-9_\-.]+)*\.[a-zA-Z0-9]+").unwrap()
    })
}

/// ユーザー入力からファイルパスを抽出する
///
/// # 引数
/// * `input` - ユーザーの入力文字列
///
/// # 戻り値
/// 検出されたファイルパスのベクトル（重複を除く）
///
/// # 例
/// ```
/// let paths = detect_file_paths("src/main.rsを読んでコメントを追加して");
/// assert_eq!(paths, vec!["src/main.rs"]);
/// ```
pub fn detect_file_paths(input: &str) -> Vec<String> {
    let pattern = file_path_pattern();
    let mut paths = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for cap in pattern.find_iter(input) {
        let path = cap.as_str();
        // 重複を除外
        if seen.insert(path.to_string()) {
            paths.push(path.to_string());
        }
    }

    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_file() {
        let input = "src/main.rsを読んでコメントを追加して";
        let paths = detect_file_paths(input);
        assert_eq!(paths, vec!["src/main.rs"]);
    }

    #[test]
    fn test_multiple_files() {
        let input = "Cargo.tomlとsrc/lib.rsを見て依存関係を説明して";
        let paths = detect_file_paths(input);
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&"Cargo.toml".to_string()));
        assert!(paths.contains(&"src/lib.rs".to_string()));
    }

    #[test]
    fn test_absolute_path() {
        let input = "/home/user/config.tomlを確認して";
        let paths = detect_file_paths(input);
        assert_eq!(paths, vec!["/home/user/config.toml"]);
    }

    #[test]
    fn test_home_directory() {
        let input = "~/Documents/notes.txtを読んで";
        let paths = detect_file_paths(input);
        assert_eq!(paths, vec!["~/Documents/notes.txt"]);
    }

    #[test]
    fn test_relative_path_with_dot() {
        let input = "このファイル ./config.toml を確認して";
        let paths = detect_file_paths(input);
        assert_eq!(paths, vec!["./config.toml"]);
    }

    #[test]
    fn test_no_files() {
        let input = "日本の首都は？";
        let paths = detect_file_paths(input);
        assert_eq!(paths.len(), 0);
    }

    #[test]
    fn test_duplicate_files() {
        let input = "main.rsとmain.rsを比較して";
        let paths = detect_file_paths(input);
        assert_eq!(paths, vec!["main.rs"]);
    }
}
