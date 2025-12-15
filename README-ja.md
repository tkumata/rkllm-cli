
# RKLLM CLI

Rust implementation of a CLI tool for chatting with LLM models on Rockchip NPU using the `librkllmrt.so` library.

## プロジェクト概要

このプロジェクトは、Rockchip NPU上で動作するLarge Language Models (LLMs)と通信するためのコマンドラインインターフェースを提供します。`librkllmrt.so`ライブラリを使用して、ローカルファイルをLLMモデルに渡してチャットする機能を実装しています。

## ファイナル機能

- **インタラクティブなチャット**: サイレット出力のサポートあり
- **セーフなRustワッパー**: Cライブラリとの型安全なインターフェース
- **UTF-8の処理**: サイレット出力において不完全なマルチバイトUTF-8シーケンスの処理
- **エラーハンドリング**: `anyhow`による充実したエラーハンドリング
- **ファイルの読み込み**: ローカルファイルの読み込み
- **ファイルの書き込み**: ローカルファイルの書き込み

## プライマリ要件

### ハードウェア

- Rockchipボード (rk3588またはrk3576) にNPUサポート
- 例: Rock5B上でArmbian (aarch64) を動作

### ソフトウェア

- Rustツールチェーン (aarch64-unknown-linux-gnuのためのクロスコンパイル)
- `librkllmrt.so`ライブラリ (Rockchip提供)
- RKLLMモデルファイル (`.rkllm`フォーマット)

## システム設定

### 1. ライブラリの配置

`src/lib/`ディレクトリに`librkllmrt.so`をコピーします:

```bash
cp /path/to/librkllmrt.so src/lib/
```

または、ターゲットデバイス上のシステムライブラリに配置:

```bash
sudo cp librkllmrt.so /usr/local/lib/
sudo ldconfig
```

### 2. プロジェクトのビルド

ターゲットデバイス上でネイティブビルド:

```bash
cargo build --release
```

クロスコンパイル (Mac/Linuxから):

```bash
# カスケードコンパイルツールチェーンのインストール
rustup target add aarch64-unknown-linux-gnu

# ビルド
cargo build --release --target aarch64-unknown-linux-gnu
```

ビルド出力は以下の場所にあります:

- ネイティブ: `target/release/rkllm-cli`
- クロスコンパイル: `target/aarch64-unknown-linux-gnu/release/rkllm-cli`

## 使用法

### チャットセッションの開始

```bash
./target/release/rkllm-cli chat --model /path/to/your/model.rkllm
```

### サンプル

![スクリーンショット](./docs/screenshot.png)

### コマンド

- メッセージを入力しEnterを押して送信
- `exit`または`quit`でセッションを終了
- `Ctrl+CとCtrl+C`で中断して終了

## プロジェクト構造

```
rkllm-cli/
├── Cargo.toml           # Rustパッケージ構成
├── build.rs             # ライブラリリンクのビルドスクリプト
├── src/
│   ├── main.rs          # CLIエントリポイント
│   ├── ffi.rs           # CライブラリとのFFIバインディング
│   ├── llm.rs           # セーフなRustワッパー
│   ├── chat.rs          # チャットセッションロジック
│   └── lib/
│       └── librkllmrt.so  # Rockchip RKLLMランタイムライブラリ (ここに配置)
├── sample/
│   └── gradio_server.py   # Python参考実装
└── docs/
    └── RKLLM_RUST_CLI_REQUIREMENTS.md  # リファレンス
```

## インターフェースの詳細

### FFIバインディング (ffi.rs)

- `#[repr(C)]`でCに適合した構造体を使用
- RKLLM API関数とデータ型の定義
- 型安全なエントリポイントと構造体

### RKLLMワッパー (llm.rs)

- CライブラリのセーフなRustワッパー
- キャーチングとUTF-8デコードの管理
- サイレット出力のサポート
- `Drop`トレイトによる自動リソース解放

### チャットロジック (chat.rs)

- `rustyline`ベースのインタラクティブなリーディングインターフェース
- サイレット出力のサポート
- コマンド履歴

## 配置

モデルは、以下のデフォルトパラメータで初期化されます (`llm.rs`で調整可能です):

- `max_context_len`: 4096
- `max_new_tokens`: -1 (無制限)
- `top_k`: 20
- `top_p`: 0.8
- `temperature`: 0.7
- `repeat_penalty`: 1.0
- `skip_special_token`: true

## トラブルシューティング

### ライブラリが見つからない

- ライブラリが`src/lib/`に配置されているか確認
- `LD_LIBRARY_PATH`環境変数を設定:
  ```bash
  export LD_LIBRARY_PATH=/path/to/lib:$LD_LIBRARY_PATH
  ./rkllm-cli chat --model model.rkllm
  ```

### モデルロードが失敗

- モデルファイルのパスが正しいか確認
- モデルが`.rkllm`フォーマットで存在しているか確認
- デバイス上のメモリが十分か確認

## 今後のアップデート

- **Phase 2**: ファイルのアップロードとコンテキスト管理 (Claude CLIスタイル)
- **Phase 3**: MCP (Model Context Protocol)クライアントサポート

## ライセンス

このプロジェクトは、Rockchip NPUハードウェアとの使用を目的としたものです。

## 参考

- Python実装: `sample/gradio_server.py`
- 要件ドキュメント: `docs/RKLLM_RUST_CLI_REQUIREMENTS.md`
