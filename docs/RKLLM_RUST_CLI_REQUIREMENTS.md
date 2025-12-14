# RKLLM Rust CLI 実装要件書

## プロジェクト概要

Rockchip が提供する C ライブラリ `librkllmrt.so` (aarch64) を Rust でラップし、CLI から LLM とチャットできるツールを実装する。

## 開発環境

- **開発マシン**: Rock5B (aarch64 Armbian)
- **実行環境**: Rock5B (aarch64 Armbian)
- **ライブラリ**: `librkllmrt.so` (ヘッダファイルなし)
- **参考実装**: Python による実装あり (gradio_server.py)

## 実装フェーズ

### 第一弾: 基本的な CLI Chat 機能 (本要件書の対象)

- コマンドラインから対話的にチャットできる
- ストリーミング出力対応

### 第二弾: Claude CLI 風のファイル操作機能

- コンテキストにファイルを含める

### 第三弾: MCP クライアント機能

- MCP サーバと通信して外部ツールを利用

---

## librkllmrt.so の API 構造

### 主要な関数

Python の ctypes による実装から抽出した API:

1. **rkllm_init**
   - シグネチャ: `int rkllm_init(RKLLM_Handle_t*, RKLLMParam*, callback_fn)`
   - 用途: モデルの初期化
   - 戻り値: 成功時 0

2. **rkllm_run**
   - シグネチャ: `int rkllm_run(RKLLM_Handle_t, RKLLMInput*, RKLLMInferParam*, void*)`
   - 用途: 推論実行
   - 戻り値: 成功時 0

3. **rkllm_destroy**
   - シグネチャ: `int rkllm_destroy(RKLLM_Handle_t)`
   - 用途: リソース解放
   - 戻り値: 成功時 0

4. **rkllm_load_lora** (オプション)
   - シグネチャ: `int rkllm_load_lora(RKLLM_Handle_t, RKLLMLoraAdapter*)`
   - 用途: LoRA アダプタのロード

5. **rkllm_load_prompt_cache** (オプション)
   - シグネチャ: `int rkllm_load_prompt_cache(RKLLM_Handle_t, const char*)`
   - 用途: プロンプトキャッシュのロード

### データ構造

#### RKLLMParam (初期化パラメータ)

```c
struct RKLLMParam {
    char* model_path;              // モデルファイルのパス
    int32_t max_context_len;       // 最大コンテキスト長 (例: 2048)
    int32_t max_new_tokens;        // 最大生成トークン数 (-1 で無制限)
    int32_t top_k;                 // Top-K サンプリング (例: 1)
    float top_p;                   // Top-P サンプリング (例: 0.9)
    float temperature;             // 温度パラメータ (例: 0.5)
    float repeat_penalty;          // 繰り返しペナルティ (例: 1.2)
    float frequency_penalty;       // 頻度ペナルティ (例: 0.0)
    float presence_penalty;        // 存在ペナルティ (例: 0.0)
    int32_t mirostat;              // Mirostat アルゴリズム (例: 0)
    float mirostat_tau;            // Mirostat tau (例: 5.0)
    float mirostat_eta;            // Mirostat eta (例: 0.1)
    bool skip_special_token;       // 特殊トークンをスキップ (例: true)
    bool is_async;                 // 非同期モード (例: false)
    char* img_start;               // 画像開始タグ
    char* img_end;                 // 画像終了タグ
    char* img_content;             // 画像コンテンツ
    RKLLMExtendParam extend_param; // 拡張パラメータ
};
```

#### RKLLMExtendParam

```c
struct RKLLMExtendParam {
    int32_t base_domain_id;  // 例: 0
    uint8_t reserved[112];   // 予約領域
};
```

#### RKLLMInput (推論入力)

```c
enum RKLLMInputMode {
    RKLLM_INPUT_PROMPT = 0,      // プロンプト文字列
    RKLLM_INPUT_TOKEN = 1,       // トークン配列
    RKLLM_INPUT_EMBED = 2,       // エンベディング
    RKLLM_INPUT_MULTIMODAL = 3   // マルチモーダル
};

union RKLLMInputUnion {
    char* prompt_input;                    // プロンプト文字列
    RKLLMEmbedInput embed_input;           // エンベディング
    RKLLMTokenInput token_input;           // トークン
    RKLLMMultiModelInput multimodal_input; // マルチモーダル
};

struct RKLLMInput {
    int input_mode;              // 入力モード
    RKLLMInputUnion input_data;  // 入力データ
};
```

#### RKLLMInferParam (推論パラメータ)

```c
enum RKLLMInferMode {
    RKLLM_INFER_GENERATE = 0,                // 通常の生成
    RKLLM_INFER_GET_LAST_HIDDEN_LAYER = 1    // 隠れ層取得
};

struct RKLLMInferParam {
    RKLLMInferMode mode;                  // 推論モード
    RKLLMLoraParam* lora_params;          // LoRA パラメータ (NULL 可)
    RKLLMPromptCacheParam* prompt_cache_params; // キャッシュパラメータ (NULL 可)
};
```

#### コールバック関数

```c
enum LLMCallState {
    RKLLM_RUN_NORMAL = 0,                    // 通常実行中
    RKLLM_RUN_WAITING = 1,                   // 待機中
    RKLLM_RUN_FINISH = 2,                    // 完了
    RKLLM_RUN_ERROR = 3,                     // エラー
    RKLLM_RUN_GET_LAST_HIDDEN_LAYER = 4      // 隠れ層取得
};

struct RKLLMResult {
    char* text;                             // 生成されたテキスト
    int size;                               // テキストのサイズ
    RKLLMResultLastHiddenLayer last_hidden_layer; // 隠れ層データ
};

// コールバック関数の型
typedef void (*callback_fn)(RKLLMResult*, void* userdata, LLMCallState state);
```

---

## Python 実装からの重要な知見

### プロンプトテンプレート

Python 実装では以下のテンプレートを使用:

```
<|im_start|>system
あなたは高い知識と多様なスキルを持ったAIアシスタントです。質問者からのすべての問い合わせに対して、わかりやすく丁寧な日本語で回答してください。技術的な話題から日常的な話題まで、幅広い分野に対応し、ユーザーが納得できる情報を提供することを心がけてください。具体例や参考情報が必要な場合は、日本語で適切な例を挙げ、できる限り明確に解説してください。誤解を避けるため、正確で簡潔な説明を心がけ、曖昧な表現を避けてください。以下のルールに従ってください。
1. 日本語で回答してください。
2. 思考プロセスを明確に示しながら回答してください。
<|im_end|> <|im_start|>user
{ユーザー入力}
<|im_end|><|im_start|>assistant
```

### コールバックでのストリーミング処理

- コールバックは推論中に複数回呼ばれる
- `state == RKLLM_RUN_NORMAL` の時に部分的なテキストが返される
- `state == RKLLM_RUN_FINISH` で完了
- UTF-8 の不完全なバイト列が来る可能性があるため、Python では分割されたバイトを保持して次回と結合している

```python
# Python での処理例
try:
    decoded = (split_byte_data + result.contents.text).decode('utf-8')
    print(decoded, end='')
    split_byte_data = b""
except UnicodeDecodeError as e:
    split_byte_data = split_byte_data + result.contents.text
```

### 初期化時のパラメータ例

```python
max_context_len = 4096
max_new_tokens = -1          # -1 で無制限
skip_special_token = True
top_k = 20
top_p = 0.9
temperature = 0.7
repeat_penalty = 1.0
frequency_penalty = 0.0
presence_penalty = 0.0
mirostat = 0
mirostat_tau = 5.0
mirostat_eta = 0.1
is_async = False
```

---

## プロジェクト構造

```
rkllm-cli/
├── Cargo.toml
├── build.rs                 # librkllmrt.so のリンク設定
├── .cargo/
│   └── config.toml         # クロスコンパイル設定 (必要に応じて)
├── src/
│   ├── lib/
│   │   └── librkllmrt.so   # aarch64 用共有ライブラリ
│   ├── main.rs             # CLI エントリポイント
│   ├── ffi.rs              # Rust FFI バインディング
│   ├── llm.rs              # RKLLM のラッパー
│   └── chat.rs             # Chat ロジック
└── README.md
```

---

## 第一弾の実装要件

### 機能要件

1. **基本的な対話機能**
   - プロンプトを入力して応答を得る
   - ストリーミング出力 (トークンごとに表示)
   - 連続した会話が可能

2. **CLI インターフェース**
   - `rkllm-cli chat --model <モデルパス>` のような起動方法
   - 対話モードに入る
   - `Ctrl+C` または `exit` で終了

3. **エラーハンドリング**
   - ライブラリの初期化失敗
   - モデルファイルが見つからない
   - 推論中のエラー

### 非機能要件

1. **パフォーマンス**
   - Python 実装と同等以上の速度
   - メモリリークなし

2. **メンテナンス性**
   - FFI バインディングを `ffi.rs` に分離
   - ビジネスロジックを `llm.rs` と `chat.rs` に分離

3. **安全性**
   - unsafe コードは必要最小限に
   - ライフタイムとドロップ処理を適切に管理

---

## 実装のポイント

### 1. FFI バインディング (ffi.rs)

- `#[repr(C)]` を使って C 互換の構造体を定義
- `extern "C"` で関数を宣言
- `libc` クレートを使用

### 2. コールバック処理

- Rust のクロージャを C のコールバックに変換
- グローバルな状態管理が必要な場合は `Arc<Mutex<T>>` を使用
- コールバック内での panic を避ける

### 3. UTF-8 処理

- C から受け取ったバイト列を安全に UTF-8 に変換
- 不完全なマルチバイト文字を次回まで保持

### 4. ライブラリのリンク (build.rs)

```rust
fn main() {
    println!("cargo:rustc-link-search=native=src/lib");
    println!("cargo:rustc-link-lib=dylib=rkllmrt");
    println!("cargo:rerun-if-changed=src/lib/librkllmrt.so");
}
```

### 5. 依存クレート候補

- `libc`: C FFI
- `clap`: CLI 引数パース
- `rustyline`: 対話的な入力 (readline 風)
- `anyhow`: エラーハンドリング
- `tokio` or `async-std`: 非同期処理 (必要に応じて)

---

## テスト方法

1. **ビルド**
   ```bash
   cargo build
   ```

2. **実行**
   ```bash
   ./target/debug/rkllm-cli chat --model /path/to/model.rkllm
   ```

3. **動作確認**
   - 簡単な質問を入力して応答を確認
   - ストリーミング出力が正しく動作するか確認
   - 連続した会話が可能か確認
   - 終了処理が正常に動作するか確認

---

## 参考情報

- Python 実装ファイル: `gradio_server.py`
- 対象プラットフォーム: rk3588 または rk3576
- ライブラリの配置: プロジェクトルートの `src/lib/librkllmrt.so`

---

## 注意事項

- ヘッダファイルが提供されていないため、Python 実装から API を逆引きしている
- 中華企業製のライブラリのため、ドキュメントが不十分な可能性がある
- 実装中に予期しない動作がある場合は、Python 実装の動作を参考にする

---

## 次フェーズへの展望

第一弾が完成したら、以下の機能を追加予定:

- **第二弾**: Claude CLI 風のファイル操作機能
  - コンテキストにファイル内容を含める
  - マルチターン会話でのファイル参照

- **第三弾**: MCP クライアント機能
  - MCP サーバとの通信
  - 外部ツールの実行
  - 結果のコンテキスト統合
