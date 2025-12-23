/// ファイル操作意図と出力優先度の判定をまとめたモジュール
use once_cell::sync::Lazy;
use std::collections::HashSet;

static STRONG_KEYWORDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        // 日本語
        "作成", "作って", "作る", "つくって", "つくる",
        "書き込", "書いて", "書く", "かいて",
        "保存", "ほぞん",
        "生成", "せいせい",
        "出力し", "出力ファイル",
        // 英語
        "create", "write", "save", "generate",
        "make a file", "make file",
    ]
    .into_iter()
    .collect()
});

static FILE_OPERATION_PHRASES: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "ファイルに", "ファイルを作", "ファイルを書", "ファイルを生成", "ファイルを出力",
        "file to", "file and", "to file", "in file", "into file",
        "create file", "write file", "save file", "output file", "generate file",
    ]
    .into_iter()
    .collect()
});

static FILE_READ_KEYWORDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        // 日本語
        "読む", "読み込", "読んで", "読み込んで",
        "翻訳", "要約", "校正", "修正", "整える",
        // 英語
        "read", "load", "summarize", "summary", "translate", "proofread", "polish",
    ]
    .into_iter()
    .collect()
});

/// ファイル操作の意図が含まれているかを判定
pub fn has_file_operation_intent(input: &str) -> bool {
    let input_lower = input.to_lowercase();

    STRONG_KEYWORDS
        .iter()
        .any(|&kw| input_lower.contains(kw))
        || FILE_OPERATION_PHRASES
            .iter()
            .any(|&phrase| input_lower.contains(phrase))
}

/// ファイル読み込みの意図が含まれているかを判定
pub fn has_file_read_intent(input: &str) -> bool {
    let input_lower = input.to_lowercase();
    FILE_READ_KEYWORDS
        .iter()
        .any(|&kw| input_lower.contains(kw))
}

/// 出力専用と推定できるキーワードを含むか判定
pub fn prefers_output_only(input: &str) -> bool {
    // has_file_operation_intent が真なら、強いキーワードはすでに検出済み。
    // ここでは「保存/書き込み/生成」系の語と file operation phrases を再利用して出力優先を判定する。
    has_file_operation_intent(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_file_read_intent_japanese() {
        assert!(has_file_read_intent("このファイルを読んで"));
        assert!(has_file_read_intent("翻訳して"));
        assert!(has_file_read_intent("要約して"));
        assert!(has_file_read_intent("校正して"));
        assert!(has_file_read_intent("修正して"));
        assert!(!has_file_read_intent("保存して"));
    }

    #[test]
    fn test_has_file_read_intent_english() {
        assert!(has_file_read_intent("read the file"));
        assert!(has_file_read_intent("summarize the file"));
        assert!(has_file_read_intent("translate this"));
        assert!(has_file_read_intent("proofread the text"));
        assert!(!has_file_read_intent("save the output"));
    }
}
