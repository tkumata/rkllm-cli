
# RKLLM CLI

(これは Gemma3:4B IT をモデルで利用し、本プログラムで「README.mdを読み込んで、翻訳結果をREADME-ja.mdに書いて」で出力されてものです。

Rust実装による、Rockchip NPU上でLLMモデルとチャットするためのCLIツール。 `librkllmrt.so`ライブラリを使用。

## プロジェクト概要

このプロジェクトは、大規模言語モデル (LLM) をRockchip NPUハードウェア (rk3588, rk3576) で実行するためのコマンドラインインターフェースを提供します。Rust FFIバインディングを使用して、ネイティブの `librkllmrt.so` ライブラリと通信します。

## 機能

- **インタラクティブチャット**: ストリーミング出力を含むコマンドラインチャットインターフェース
- **安全なRustラッパー**: Cライブラリ向けの型安全なRustバインディング
- **UTF-8処理**: ストリーミング中に不完全なマルチバイトUTF-8シーケンスの適切な処理
- **エラー処理**: `anyhow`を使用した包括的なエラー処理
- **ファイル読み込み**: ローカルファイルを読み込み、LLMモデルに渡す

## 前提条件

### ハードウェア

- Rockchipボード (rk3588またはrk3576) とNPUサポート
- 例：Rock5B での Armbian (aarch64)

### ソフトウェア

- Rustツールチェーン (aarch64-unknown-linux-gnu などのクロスコンパイルの場合)
- `librkllmrt.so` ライブラリ (Rockchipから提供)
- RKLLMモデルファイル (`.rkllm`形式)

## セットアップ

### 1. 共有ライブラリの配置

`librkllmrt.so` を `src/lib` ディレクトリにコピーします。

```bash
cp /path/to/librkllmrt.so src/lib/
```

または、ターゲットデバイスのシステムライブラリパスに配置します。

```bash
sudo cp librkllmrt.so /usr/local/lib/
sudo ldconfig
```

### 2. プロジェクトのビルド

ネイティブビルドの場合:

```bash
cargo build --release
```

クロスコンパイルの場合:

```bash
# クロスコンパイルツールチェーンをインストール
rustup target add aarch64-unknown-linux-gnu

# ビルド
cargo build --release --target aarch64-unknown-linux-gnu
```

実行可能ファイルは次の場所にあります。

- ネイティブ: `target/release/rkllm-cli`
- クロスコンパイル: `target/aarch64-unknown-linux-gnu/release/rkllm-cli`

## 使用方法

### チャットセッションの開始

```bash
./target/release/rkllm-cli chat --model /path/to/your/model.rkllm
```

### 例

![screenshot](./docs/screenshot.png)

### コマンド

- メッセージを入力してEnterキーを押して送信
- セッションを終了するには`exit`または`quit`を入力
- セッションを中断して終了するには`Ctrl+C`を入力

## プロジェクト構造

```
rkllm-cli/
├── Cargo.toml           # Rust パッケージ構成
├── build.rs             # librkllmrt.so のリンキング用ビルドスクリプト
├── src/
│   ├── main.rs          # CLI エントリポイント
│   ├── ffi.rs           # librkllmrt.so の FFI バインディング
│   ├── llm.rs           # RKLLM の安全な Rust ラッパー
│   ├── chat.rs          # チャットセッションロジック
│   └── lib/
│       └── librkllmrt.so  # Rockchip RKLLM ランタイムライブラリ (ここに配置)
├── sample/
│   └── gradio_server.py   # Python 参照実装
└── docs/
    └── RKLLM_RUST_CLI_REQUIREMENTS.md  # 実装要件
```

## 実装の詳細

### FFI バインディング (ffi.rs)

- `#[repr(C)]` を使用して C-互換構造を定義
- 全ての RKLLM API 関数とデータ型を定義
- ストリーミング中に不完全なマルチバイト UTF-8 シーケンスの適切な処理

### RKLLM ラッパー (llm.rs)

- RKLLM ライブラリの安全な Rust ラッパー
- コールバックの登録と UTF-8 デコードの管理
- 不完全なマルチバイトシーケンスの自動リソースクリーンアップ (`Drop` トレイトを使用)

### チャットロジック (chat.rs)

- `rustyline` を使用したインタラクティブ readline インターフェース
- ストリーミング出力のサポート
- コマンド履歴

## 設定

モデルは、デフォルトのパラメータで初期化されます ( `llm.rs` で変更可能):

- `max_context_len`: 2048
- `max_new_tokens`: -1 (無制限)
- `top_k`: 1
- `top_p`: 0.9
- `temperature`: 0.5
- `repeat_penalty`: 1.2
- `skip_special_token`: true

## トラブルシューティング

### ライブラリが見つからない

`librkllmrt.so` が見つからないエラーが発生した場合:

1. ライブラリが `src/lib/` ディレクトリに存在することを確認
2. または、`LD_LIBRARY_PATH` を設定:

   ```bash
   export LD_LIBRARY_PATH=/path/to/lib:$LD_LIBRARY_PATH
   ./rkllm-cli chat --model model.rkllm
   ```

### モデルのロード失敗

- モデルファイルのパスが正しいことを確認
- モデルが RKLLM 形式 (`.rkllm`) であることを確認
- ターゲットデバイスのメモリが十分であることを確認

## 今後の改善点

- **フェーズ 2**: ファイルアップロードとコンテキスト管理 (Claude CLI スタイルの)
- **フェーズ 3**: MCP (Model Context Protocol) クライアントサポート

## ライセンス

このプロジェクトは、Rockchip NPU ハードウェアで使用することを目的として提供されています。

## 参照

- Python 実装: `sample/gradio_server.py`
- 要件ドキュメント: `docs/RKLLM_RUST_CLI_REQUIREMENTS.md`
</file_error>
