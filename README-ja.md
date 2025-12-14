
# RKLLM CLI

(このファイルは、Qwen3 1B を用いて、本プログラムで「英語のREADME.mdを読み込んで、日本語に翻訳し結果をREADME-ja.mdに保存してください。」と指示した結果です。)

Rust実装のCLIツールで、Rockchip NPU上で動作するLarge Language Models (LLMs) と通信するためのツールです。`librkllmrt.so`ライブラリを使用して、ローカルファイルを読み込むことも可能で、Rust FFIバインディングを使用してCライブラリとのインターフェースを提供しています。

## プロジェクト概要

このプロジェクトは、Rockchip NPU (rk3588, rk3576) に搭載されたLLMモデルと通信するためのコマンドラインインターフェースを提供します。`librkllmrt.so`ライブラリを使用して、ローカルファイルを読み込むことも可能です。

## フェアリティ

- **インタラクティブなチャット**: ターミナルでメッセージを送信し、ストリーミング出力を受け付けるインターフェース
- **セーフなRustラッパー**: Cライブラリとの型安全なインターフェース
- **UTF-8処理**: ストリーミング中に不完全なマルチバイトUTF-8セキュリティを処理
- **エラー処理**: `anyhow`を使用したフルエラーハンドリング
- **ファイル読み込み**: ローカルファイルを読み込む機能

## 前提条件

### ハードウェア

- Rockchipボード (rk3588またはrk3576) にNPUサポートが含まれる
- 例: Rock5BでArmbian (aarch64) を稼働中

### ソフトウェア

- Rustツールチェーン (aarch64-unknown-linux-gnuのためのクロスコンパイル)
- `librkllmrt.so`ライブラリ (Rockchip提供)
- RKLLMモデルファイル (`.rkllm`フォーマット)

## システムセット

### 1. ライブラリの配置

ローカルディレクトリにコピー:

```bash
cp /path/to/librkllmrt.so src/lib/
```

または、ターゲットデバイスに配置:

```bash
sudo cp librkllmrt.so /usr/local/lib/
sudo ldconfig
```

### 2. プロジェクトのビルド

ターゲットデバイス上でビルド:

```bash
cargo build --release
```

クロスコンパイル (MacまたはLinuxから):

```bash
# Rustupのインストール
rustup target add aarch64-unknown-linux-gnu

# ビルド
cargo build --release --target aarch64-unknown-linux-gnu
```

ビルド後のバイナリが以下にあります:

- ナイティブ: `target/release/rkllm-cli`
- クロスコンパイル: `target/aarch64-unknown-linux-gnu/release/rkllm-cli`

## 使用方法

### チャットセッションの開始

```bash
./target/release/rkllm-cli chat --model /path/to/your/model.rkllm
```

### サンプル

![screenshot](./docs/screenshot.png)

### コマンド

- メッセージを入力してEnterを押すと送信
- `exit`または`quit`でセッションを終了
- `Ctrl+C`で中断して終了

## プロジェクト構造

```
rkllm-cli/
├── Cargo.toml           # Rustパッケージ構成
├── build.rs             # ビルドスクリプト
├── src/
│   ├── main.rs          # CLIエントリポイント
│   ├── ffi.rs           # FFIバインディング
│   ├── llm.rs           # セーフなRustラッパー
│   ├── chat.rs          # チャットセッションロジック
│   └── lib/
│       └── librkllmrt.so  # Rockchip RKLLMランタイムライブラリ (場所に置いてください)
├── sample/
│   └── gradio_server.py   # Python参照実装
└── docs/
    └── RKLLM_RUST_CLI_REQUIREMENTS.md  # インターフェース要件
```

## メッセージの構造

- `#[repr(C)]`を使用してCコンパクトな構造体を定義
- 実装の詳細は`ffi.rs`および`llm.rs`で確認

## チャットロジック

- ライナルベースのリーディングインターフェースを使用
- ストリーミング出力サポート
- コマンド履歴

## 配置情報

モデルは以下のデフォルトパラメータで初期化されます (`llm.rs`で変更可能です):

- `max_context_len`: 4096
- `max_new_tokens`: -1 (無制限)
- `top_k`: 20
- `top_p`: 0.8
- `temperature`: 0.7
- `repeat_penalty`: 1.0
- `skip_special_token`: true

## ツールチケット

- ライブラリが見つからない場合:
  1. ライブラリが`src/lib/`に配置されているか確認
  2. `LD_LIBRARY_PATH`に設定:
     ```bash
     export LD_LIBRARY_PATH=/path/to/lib:$LD_LIBRARY_PATH
     ./rkllm-cli chat --model model.rkllm
     ```

## モデルロードエラー

- モデルファイルのパスが正しくないか確認
- モデルが`.rkllm`フォーマットであるか確認
- 足りないメモリがあるか確認

## 今後のアップデート

- **Phase 2**: ファイルアップロードとコンテキスト管理 (Claude CLIスタイル)
- **Phase 3**: MCP (Model Context Protocol) クライアントサポート

## ライセンス

このプロジェクトは、Rockchip NPUハードウェアで提供されているものです。

## 参考

- Python実装: `sample/gradio_server.py`
- 要件ドキュメント: `docs/RKLLM_RUST_CLI_REQUIREMENTS.md`
