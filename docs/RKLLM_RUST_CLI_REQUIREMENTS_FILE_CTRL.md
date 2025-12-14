# RKLLM CLI ファイル操作機能 要件書

## 概要

RKLLM CLI に、Claude Code や Gemini CLI のような対話中のファイル読み書き機能を追加する。

## 目標

対話中にユーザーが自然言語で指示することで、ファイルの読み書きができる CLI を実現する。

---

## 機能要件

### 1. ファイル読み込み機能

#### 1.1 基本動作

- ユーザーが対話中にファイルパスを含む質問をする
- CLI 側でファイルパスを検出し、ファイルを読み込む
- ファイル内容をプロンプトに組み込んで LLM に送信
- LLM が内容を理解して応答する

#### 1.2 検出パターン

以下のような入力からファイルパスを検出：

```
例1: "src/main.rs を読んでコメントを追加して"
例2: "README.md の内容を要約して"
例3: "このファイル ./config.toml を確認して"
例4: "Cargo.toml と src/lib.rs を見て依存関係を説明して"
```

#### 1.3 対応ファイル形式

- テキストファイル全般（.txt, .md, .rs, .py, .toml, .json, etc.）
- UTF-8 エンコーディング
- バイナリファイルは除外

#### 1.4 パス解決

- 相対パス：実行ディレクトリからの相対パス
- 絶対パス：そのまま使用
- `~`（ホームディレクトリ）の展開をサポート

#### 1.5 エラーハンドリング

- ファイルが存在しない場合：ユーザーに通知して LLM にも伝える
- 読み込み権限がない場合：エラーメッセージを表示
- ファイルサイズが大きすぎる場合：警告または制限（例：1MB 以上）

### 2. ファイル書き込み機能

#### 2.1 基本動作

LLM の応答から「ファイル作成・編集が必要」という指示を検出し、実際にファイルを書き込む。

#### 2.2 検出方法（案）

LLM の出力に特定のマーカーを含めることで検出：

```
[CREATE_FILE: src/example.rs]
\`\`\`rust
fn main() {
    println!("Hello");
}
\`\`\`
[END_FILE]
```

または

```xml
<file path="src/example.rs">
fn main() {
    println!("Hello");
}
</file>
```

#### 2.3 安全性

- 既存ファイルの上書き前に確認プロンプトを表示
- デフォルトでは実行ディレクトリ配下のみ書き込み可能
- システムディレクトリへの書き込みを制限

#### 2.4 動作例

```
> src/utils.rs にヘルパー関数を作って

◆ ヘルパー関数を作成します。

<file path="src/utils.rs">
pub fn format_message(msg: &str) -> String {
    format!("[INFO] {}", msg)
}
</file>

[Detected 1 file operation(s)]
[Created/Updated: src/utils.rs]
```

---

## 実装詳細

### 3. アーキテクチャ

以下のモジュール構成で実装：

```
src/
├── file_detector.rs       # ユーザー入力からファイルパス検出
├── file_ops.rs            # ファイル読み書き操作
├── file_output_parser.rs  # LLM出力からファイル操作を抽出
├── prompt_builder.rs      # プロンプト構築（ファイル内容を含む）
└── chat.rs                # メインのチャットロジック（統合）
```

### 4. ファイル検出の実装

#### 4.1 正規表現パターン

ユーザー入力から以下のパターンでファイルパスを検出：

```regex
(?:~/|/|\./)?[a-zA-Z0-9_\-.]+(?:/[a-zA-Z0-9_\-.]+)*\.[a-zA-Z0-9]+
```

対応するパターン：
- `src/main.rs`（相対パス）
- `./config.toml`（明示的な相対パス）
- `/home/user/file.txt`（絶対パス）
- `~/Documents/notes.txt`（ホームディレクトリ）
- `README.md`（ファイル名のみ）

#### 4.2 重複排除

同一ファイルパスが複数検出された場合は1つにまとめる。

### 5. ファイル読み込みの実装

#### 5.1 パス解決

1. `~`をホームディレクトリに展開（`shellexpand::tilde`）
2. 相対パスを絶対パスに変換（`path_absolutize`）

#### 5.2 ファイル種別判定

以下の方法でテキストファイルか判定：

1. **MIMEタイプ判定**（`mime_guess`）
   - `text/*`
   - `application/json`, `application/xml`, `application/yaml`

2. **拡張子による判定**
   - サポート対象拡張子リスト:
     - プログラミング言語: `.rs`, `.py`, `.js`, `.ts`, `.go`, `.c`, `.cpp`, `.java`, 他
     - 設定ファイル: `.toml`, `.yaml`, `.yml`, `.json`, `.env`, `.conf`
     - ドキュメント: `.md`, `.txt`, `.html`, `.css`
     - その他: `.sh`, `.bash`, `.sql`, `.dockerfile`, `.gitignore`, `.log`

#### 5.3 サイズ制限

```rust
const MAX_FILE_SIZE: u64 = 1_048_576;  // 1MB
```

1MB を超えるファイルは読み込みを拒否。

#### 5.4 エンコーディング

UTF-8デコード必須。デコードに失敗した場合はバイナリファイルとみなしてエラー。

#### 5.5 プロンプトへの組み込み

読み込んだファイルは以下の形式でプロンプトに追加：

```
以下のファイルが提供されています：

## ファイル: src/main.rs

\`\`\`
<ファイル内容>
\`\`\`

ユーザーの質問: src/main.rsを読んでコメントを追加して
```

### 6. ファイル書き込みの実装

#### 6.1 マーカー形式

LLMの出力から以下の2種類のマーカーを検出：

**形式1: XMLスタイル**

```xml
<file path="src/example.rs">
fn main() {
    println!("Hello, World!");
}
</file>
```

正規表現:
```regex
<file\s+path="([^"]+)"\s*>([\s\S]*?)</file>
```

**形式2: ブラケットスタイル**

```
[CREATE_FILE: src/example.rs]
```rust
fn main() {
    println!("Hello, World!");
}
```
[END_FILE]
```

正規表現:
```regex
\[CREATE_FILE:\s*([^\]]+)\]\s*```[a-z]*\n([\s\S]*?)\n```\s*\[END_FILE\]
```

#### 6.2 複数ファイル対応

1つの応答に複数のファイル作成指示が含まれている場合、すべて検出して処理。

#### 6.3 システムディレクトリ保護

以下のディレクトリへの書き込みを拒否：

```rust
["/etc", "/usr", "/bin", "/sbin", "/sys", "/proc",
 "/boot", "/dev", "/lib", "/lib64", "/opt", "/var"]
```

#### 6.4 既存ファイルの上書き確認

既存ファイルが存在する場合：

```
[File 'src/example.rs' already exists. Overwrite? (y/N):
```

ユーザーが`y`を入力した場合のみ上書き。

#### 6.5 ディレクトリ自動作成

ファイルパスに含まれるディレクトリが存在しない場合、`fs::create_dir_all`で自動作成。

### 7. エラーハンドリング

#### 7.1 ファイル読み込み時

| エラー種別 | メッセージ例 | 動作 |
|----------|------------|------|
| ファイル不存在 | `File not found: src/missing.rs` | LLMにエラー情報を送信 |
| 権限エラー | `Permission denied: /root/file.txt` | ユーザーに通知 |
| サイズ超過 | `File too large: 2.5MB (max: 1MB)` | 読み込み拒否 |
| バイナリファイル | `Not a text file: image.png` | 読み込み拒否 |
| UTF-8エラー | `Invalid UTF-8 encoding` | バイナリとみなす |

#### 7.2 ファイル書き込み時

| エラー種別 | メッセージ例 | 動作 |
|----------|------------|------|
| システムディレクトリ | `Writing to system directory is not allowed: /etc/config` | 書き込み拒否 |
| ディレクトリ作成失敗 | `Failed to create directory: /root/new` | 書き込み失敗 |
| 書き込み権限エラー | `Permission denied: /protected/file.txt` | 書き込み失敗 |
| 上書き拒否 | `[Skipped: src/example.rs]` | ユーザーが拒否 |

### 8. テストケース

各モジュールにユニットテストを実装：

#### 8.1 `file_detector` テスト

- 単一ファイル検出
- 複数ファイル検出
- 絶対パス検出
- ホームディレクトリパス検出
- 相対パス（`./`付き）検出
- ファイルパスがない場合
- 重複ファイルパスの除外

#### 8.2 `file_output_parser` テスト

- XMLスタイルのマーカー検出
- ブラケットスタイルのマーカー検出
- 複数ファイルの検出
- マーカーがない場合
- 両方のスタイルが混在する場合

#### 8.3 `file_ops` テスト

- 正常なファイル読み込み
- 存在しないファイルのエラー処理
- サイズ超過ファイルの拒否
- バイナリファイルの拒否
- パス解決（~展開、相対パス）
- システムディレクトリへの書き込み拒否

---

## 使用例

### 例1: ファイルを読んで質問

```
> src/main.rs を読んでバグがないか確認して

[Detected files: src/main.rs]
[Successfully loaded 1 file(s)]
────────────────────────────────────────────────

🔹 コードを確認しました。以下の点に注意してください：

1. エラーハンドリングが不足しています...
```

### 例2: ファイルを作成

```
> test.txt に "Hello, World!" を書いて

────────────────────────────────────────────────

🔹 ファイルを作成します。

<file path="test.txt">
Hello, World!
</file>

[Detected 1 file operation(s)]
[Created/Updated: test.txt]
```

### 例3: 複数ファイルの読み書き

```
> Cargo.toml と src/lib.rs を見て、新しい依存関係を追加したコードを書いて

[Detected files: Cargo.toml, src/lib.rs]
[Successfully loaded 2 file(s)]
────────────────────────────────────────────────

🔹 以下の変更を提案します：

<file path="Cargo.toml">
[package]
name = "example"
...
[dependencies]
serde = { version = "1.0", features = ["derive"] }
</file>

<file path="src/lib.rs">
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
pub struct Config {
    ...
}
</file>

[Detected 2 file operation(s)]
[File 'Cargo.toml' already exists. Overwrite? (y/N): y
[Created/Updated: Cargo.toml]
[Created/Updated: src/lib.rs]
```

---

## 制限事項

1. **バイナリファイル非対応**
   - 画像、動画、実行ファイルなどは読み込み不可

2. **ファイルサイズ制限**
   - 1MB 以上のファイルは読み込み拒否
   - 大きなログファイルなどは分割が必要

3. **ファイル編集機能なし**
   - 現在はファイル全体の上書きのみ
   - 差分パッチ適用などは未実装

4. **マルチバイト文字のパス**
   - 日本語などを含むファイルパスは正規表現で検出されない可能性あり

5. **ディレクトリ操作**
   - ディレクトリの削除、移動などは未対応
   - ファイルの削除も未対応

---

## 今後の拡張案

1. **ファイル編集機能**
   - 差分パッチの適用
   - 特定の行の置換
   - 挿入・削除

2. **ディレクトリ操作**
   - ディレクトリツリーの表示
   - ファイル一覧の取得
   - 再帰的なファイル検索

3. **バイナリファイル対応**
   - 画像のBase64エンコーディング
   - PDFからのテキスト抽出

4. **Git統合**
   - 変更差分の自動コミット
   - ブランチ管理

5. **高度な安全性**
   - サンドボックス実行環境
   - ファイル操作のプレビュー
   - 変更のロールバック

---

## 完成日

**2025年12月13日**

ファイル読み込み・書き込み機能が完全に実装され、安全性チェックとエラーハンドリングも完備。
