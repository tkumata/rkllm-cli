# RKLLM CLI MCP対応 要件書

## 概要

RKLLM CLI に Model Context Protocol (MCP) クライアント機能を追加し、外部ツールやデータソースとシームレスに統合できるようにする。

## 目標

対話中に MCP サーバと通信し、外部ツールを呼び出して結果をコンテキストに統合することで、RKLLM の機能を大幅に拡張する。

---

## MCP (Model Context Protocol) とは

### 概要

Model Context Protocol (MCP) は、LLM アプリケーションと外部システム間の接続を標準化するオープンプロトコル。Anthropic により開発され、2024年11月に発表、2025年12月には Linux Foundation 傘下の Agentic AI Foundation (AAIF) に寄贈された。

### 主要な特徴

- **プロトコル**: JSON-RPC 2.0 ベース
- **最新仕様**: 2025-11-25 版
- **通信方式**: stdio、SSE (Server-Sent Events)、WebSocket
- **公式 SDK**: Python、TypeScript、Rust

### MCP の主要コンポーネント

#### 1. Tools（ツール）

外部の機能を呼び出すための仕組み。

```json
{
  "name": "get_weather",
  "description": "Get current weather for a location",
  "input_schema": {
    "type": "object",
    "properties": {
      "location": {
        "type": "string",
        "description": "City name"
      }
    },
    "required": ["location"]
  }
}
```

#### 2. Resources（リソース）

MCP サーバが提供するデータソース。

```json
{
  "uri": "file:///path/to/document.txt",
  "name": "Project Documentation",
  "mimeType": "text/plain"
}
```

#### 3. Prompts（プロンプトテンプレート）

再利用可能なプロンプトテンプレート。

```json
{
  "name": "code_review",
  "description": "Review code for best practices",
  "arguments": [
    {
      "name": "language",
      "description": "Programming language",
      "required": true
    }
  ]
}
```

#### 4. Sampling（サーバーサイド LLM 呼び出し）

MCP サーバが LLM を呼び出して処理を実行。

#### 5. Tasks（作業追跡）

2025年11月版で追加された新機能。MCP サーバが実行する作業を追跡。

---

## 機能要件

### 1. MCP サーバとの接続

#### 1.1 基本動作

- 設定ファイル（TOML/JSON）から MCP サーバの情報を読み込み
- stdio または SSE 経由で MCP サーバに接続
- 複数の MCP サーバを同時に利用可能

#### 1.2 対応通信方式

**優先順位**:

1. **stdio** (Phase 1): 標準入出力を使った通信
   - ローカルプロセスとして MCP サーバを起動
   - 最も実装が簡単で安定している

2. **SSE** (Phase 2): HTTP Server-Sent Events
   - リモート MCP サーバとの通信
   - 長時間接続が可能

3. **WebSocket** (Phase 3): 双方向通信
   - より高度なリアルタイム通信

#### 1.3 サーバ設定例

```toml
# mcp_config.toml

[[servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/home/user/Documents"]

[[servers]]
name = "weather"
command = "/usr/local/bin/weather-mcp-server"
args = []

[[servers]]
name = "database"
url = "http://localhost:8080/mcp"
transport = "sse"
```

### 2. ツール検出と呼び出し

#### 2.1 基本動作

1. MCP サーバに接続後、利用可能なツールをリスト取得
2. LLM がツール呼び出しを要求
3. CLI が MCP サーバにツール実行リクエストを送信
4. 結果を LLM のコンテキストに統合

#### 2.2 ツール呼び出しフロー

```
ユーザー入力
  ↓
LLM が応答生成（ツール呼び出しを含む）
  ↓
CLI がツール呼び出しを検出
  ↓
MCP サーバにリクエスト送信
  ↓
結果を取得
  ↓
結果を LLM に送信
  ↓
LLM が最終応答を生成
```

#### 2.3 ツール呼び出しの検出

LLM の出力から以下のようなマーカーでツール呼び出しを検出：

**JSON スタイル**:

```json
[TOOL_CALL]
{
  "name": "get_weather",
  "arguments": {
    "location": "Tokyo"
  }
}
[END_TOOL_CALL]
```

**XML スタイル**:

```xml
<tool_call name="get_weather">
  <argument name="location">Tokyo</argument>
</tool_call>
```

### 3. リソース統合

#### 3.1 基本動作

- MCP サーバが提供するリソース（ファイル、データベースレコードなど）を取得
- リソースをプロンプトに組み込んで LLM に送信

#### 3.2 使用例

```
> データベースから最新のユーザー情報を取得して要約して

[MCP: Connecting to database server...]
[MCP: Fetched resource: database://users/latest]
[MCP: Loaded 150 records]

◆ 最新のユーザー情報を要約します...
```

### 4. プロンプトテンプレート

#### 4.1 基本動作

- MCP サーバが提供するプロンプトテンプレートを利用
- テンプレートに引数を渡して実際のプロンプトを生成

#### 4.2 使用例

```
> /prompt code_review language=rust

[MCP: Using prompt template 'code_review' from server 'development']

◆ Rust コードのレビューを開始します...
```

---

## 実装詳細

### 5. アーキテクチャ

#### 5.1 モジュール構成

```
src/
├── mcp/
│   ├── mod.rs               # MCP モジュールのルート
│   ├── client.rs            # MCP クライアント実装
│   ├── transport/
│   │   ├── mod.rs
│   │   ├── stdio.rs         # stdio トランスポート
│   │   ├── sse.rs           # SSE トランスポート
│   │   └── websocket.rs     # WebSocket トランスポート (Phase 3)
│   ├── types.rs             # MCP の型定義
│   ├── tool_executor.rs     # ツール実行ロジック
│   ├── resource_loader.rs   # リソース読み込み
│   └── config.rs            # MCP 設定読み込み
├── tool_detector.rs         # LLM 出力からツール呼び出しを検出
├── prompt_builder.rs        # プロンプト構築（MCP 統合版）
└── chat.rs                  # チャットロジック（MCP 統合）
```

#### 5.2 依存クレート

```toml
[dependencies]
# 既存の依存関係
libc = "0.2"
clap = { version = "4.5", features = ["derive"] }
crossterm = "0.29"
anyhow = "1.0"
thiserror = "2.0"
regex = "1.10"
shellexpand = "3.1"
path-absolutize = "3.1"
mime_guess = "2.0"

# MCP 関連の新規依存関係
rust-mcp-sdk = "0.1"          # 公式 Rust MCP SDK
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
tokio = { version = "1.40", features = ["full"] }
async-trait = "0.1"
toml = "0.8"                  # 設定ファイル読み込み
```

### 6. MCP クライアントの実装

#### 6.1 MCP クライアントの初期化

```rust
use rust_mcp_sdk::{ClientHandler, StdioTransport};

pub struct McpClient {
    servers: Vec<ServerConnection>,
}

struct ServerConnection {
    name: String,
    handler: ClientHandler,
    available_tools: Vec<Tool>,
}

impl McpClient {
    pub async fn new(config_path: &str) -> Result<Self> {
        let config = McpConfig::load(config_path)?;
        let mut servers = Vec::new();

        for server_config in config.servers {
            let transport = match server_config.transport {
                Transport::Stdio => {
                    StdioTransport::new(&server_config.command, &server_config.args)?
                }
                Transport::Sse => {
                    // SSE トランスポートの実装
                    todo!("SSE transport")
                }
            };

            let handler = ClientHandler::new(transport).await?;
            let tools = handler.list_tools().await?;

            servers.push(ServerConnection {
                name: server_config.name,
                handler,
                available_tools: tools,
            });
        }

        Ok(McpClient { servers })
    }

    pub fn list_all_tools(&self) -> Vec<&Tool> {
        self.servers.iter()
            .flat_map(|s| s.available_tools.iter())
            .collect()
    }

    pub async fn call_tool(&mut self, name: &str, args: serde_json::Value) -> Result<String> {
        for server in &mut self.servers {
            if let Some(tool) = server.available_tools.iter().find(|t| t.name == name) {
                let result = server.handler.call_tool(name, args).await?;
                return Ok(serde_json::to_string_pretty(&result)?);
            }
        }
        Err(anyhow::anyhow!("Tool '{}' not found", name))
    }
}
```

#### 6.2 設定ファイルの読み込み

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct McpConfig {
    pub servers: Vec<ServerConfig>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ServerConfig {
    pub name: String,
    #[serde(default = "default_transport")]
    pub transport: Transport,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Stdio,
    Sse,
    WebSocket,
}

fn default_transport() -> Transport {
    Transport::Stdio
}

impl McpConfig {
    pub fn load(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&content)?)
    }
}
```

### 7. ツール呼び出しの検出と実行

#### 7.1 LLM 出力からツール呼び出しを検出

```rust
use regex::Regex;

pub struct ToolCallDetector {
    json_pattern: Regex,
    xml_pattern: Regex,
}

#[derive(Debug)]
pub struct ToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

impl ToolCallDetector {
    pub fn new() -> Self {
        Self {
            json_pattern: Regex::new(
                r"\[TOOL_CALL\]\s*(\{[\s\S]*?\})\s*\[END_TOOL_CALL\]"
            ).unwrap(),
            xml_pattern: Regex::new(
                r#"<tool_call\s+name="([^"]+)"\s*>([\s\S]*?)</tool_call>"#
            ).unwrap(),
        }
    }

    pub fn detect(&self, text: &str) -> Vec<ToolCall> {
        let mut calls = Vec::new();

        // JSON スタイルの検出
        for cap in self.json_pattern.captures_iter(text) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&cap[1]) {
                if let Some(obj) = value.as_object() {
                    if let (Some(name), Some(args)) = (
                        obj.get("name").and_then(|v| v.as_str()),
                        obj.get("arguments")
                    ) {
                        calls.push(ToolCall {
                            name: name.to_string(),
                            arguments: args.clone(),
                        });
                    }
                }
            }
        }

        // XML スタイルの検出
        for cap in self.xml_pattern.captures_iter(text) {
            let name = cap[1].to_string();
            let args_str = &cap[2];

            // 簡易的な XML パース（引数抽出）
            let mut args = serde_json::Map::new();
            let arg_regex = Regex::new(r#"<argument\s+name="([^"]+)"\s*>([^<]*)</argument>"#).unwrap();

            for arg_cap in arg_regex.captures_iter(args_str) {
                args.insert(
                    arg_cap[1].to_string(),
                    serde_json::Value::String(arg_cap[2].to_string())
                );
            }

            calls.push(ToolCall {
                name,
                arguments: serde_json::Value::Object(args),
            });
        }

        calls
    }
}
```

#### 7.2 ツール実行エンジン

```rust
pub struct ToolExecutor {
    mcp_client: McpClient,
    detector: ToolCallDetector,
}

impl ToolExecutor {
    pub fn new(mcp_client: McpClient) -> Self {
        Self {
            mcp_client,
            detector: ToolCallDetector::new(),
        }
    }

    pub async fn execute_from_text(&mut self, text: &str) -> Result<Vec<ToolResult>> {
        let calls = self.detector.detect(text);
        let mut results = Vec::new();

        for call in calls {
            println!("[MCP: Calling tool '{}']", call.name);

            match self.mcp_client.call_tool(&call.name, call.arguments).await {
                Ok(result) => {
                    println!("[MCP: Tool '{}' completed]", call.name);
                    results.push(ToolResult {
                        name: call.name,
                        success: true,
                        output: result,
                    });
                }
                Err(e) => {
                    eprintln!("[MCP: Tool '{}' failed: {}]", call.name, e);
                    results.push(ToolResult {
                        name: call.name,
                        success: false,
                        output: format!("Error: {}", e),
                    });
                }
            }
        }

        Ok(results)
    }
}

#[derive(Debug)]
pub struct ToolResult {
    pub name: String,
    pub success: bool,
    pub output: String,
}
```

### 8. チャットループへの統合

#### 8.1 マルチターン会話でのツール統合

```rust
pub async fn chat_with_mcp(
    llm: &mut RKLLM,
    mcp_client: McpClient,
    initial_prompt: &str,
) -> Result<()> {
    let mut tool_executor = ToolExecutor::new(mcp_client);
    let mut conversation_context = String::new();

    // 初回のプロンプト
    let mut current_prompt = initial_prompt.to_string();

    loop {
        // LLM に問い合わせ
        let response = llm.run(&current_prompt)?;

        println!("{}", response);

        // ツール呼び出しを検出
        let tool_results = tool_executor.execute_from_text(&response).await?;

        if tool_results.is_empty() {
            // ツール呼び出しなし、通常の応答として終了
            break;
        }

        // ツール実行結果をコンテキストに追加
        conversation_context.push_str(&format!("\n\n## Previous Response:\n{}", response));

        for result in tool_results {
            conversation_context.push_str(&format!(
                "\n\n## Tool Result: {}\n```\n{}\n```",
                result.name,
                result.output
            ));
        }

        // 次のプロンプトを構築
        current_prompt = format!(
            "{}\n\n上記のツール実行結果を踏まえて、ユーザーの質問に回答してください。",
            conversation_context
        );
    }

    Ok(())
}
```

### 9. プロンプトへのツール情報の統合

```rust
pub struct PromptBuilder {
    mcp_client: Option<McpClient>,
}

impl PromptBuilder {
    pub fn build_prompt_with_tools(&self, user_input: &str) -> String {
        let mut prompt = String::new();

        // ツール一覧を追加
        if let Some(client) = &self.mcp_client {
            let tools = client.list_all_tools();

            if !tools.is_empty() {
                prompt.push_str("あなたは以下のツールを利用できます:\n\n");

                for tool in tools {
                    prompt.push_str(&format!(
                        "- **{}**: {}\n",
                        tool.name,
                        tool.description.as_deref().unwrap_or("説明なし")
                    ));
                }

                prompt.push_str("\nツールを使用する場合は、以下の形式で指示してください:\n");
                prompt.push_str("[TOOL_CALL]\n");
                prompt.push_str("{\n");
                prompt.push_str("  \"name\": \"tool_name\",\n");
                prompt.push_str("  \"arguments\": {\n");
                prompt.push_str("    \"arg1\": \"value1\"\n");
                prompt.push_str("  }\n");
                prompt.push_str("}\n");
                prompt.push_str("[END_TOOL_CALL]\n\n");
            }
        }

        // ユーザー入力を追加
        prompt.push_str("ユーザーの質問:\n");
        prompt.push_str(user_input);

        prompt
    }
}
```

---

## 使用例

### 例1: 天気情報の取得

```
> 東京の天気を教えて

[MCP: Detected 1 tool call]
[MCP: Calling tool 'get_weather']
[MCP: Tool 'get_weather' completed]

◆ 東京の現在の天気は晴れ、気温は18度です。
```

### 例2: ファイルシステムツールとの連携

```toml
# mcp_config.toml
[[servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/home/user/project"]
```

```
> プロジェクト内の .rs ファイルを全部リストアップして

[MCP: Calling tool 'list_files']
[MCP: Tool 'list_files' completed]

◆ プロジェクト内の Rust ファイルは以下の通りです:
- src/main.rs
- src/ffi.rs
- src/llm.rs
- src/chat.rs
...
```

### 例3: データベースツールとの連携

```
> データベースから最新の10件のユーザーを取得して表形式で表示して

[MCP: Calling tool 'query_database']
[MCP: Tool 'query_database' completed]

◆ 最新の10件のユーザー情報:

| ID | Name       | Email                | Created At          |
|----|------------|----------------------|---------------------|
| 1  | Alice      | alice@example.com    | 2025-01-01 10:00:00 |
| 2  | Bob        | bob@example.com      | 2025-01-02 11:30:00 |
...
```

### 例4: 複数ツールの連携

```
> 東京の天気を取得して、その結果をファイル weather_report.txt に保存して

[MCP: Calling tool 'get_weather']
[MCP: Tool 'get_weather' completed]
[MCP: Calling tool 'write_file']
[MCP: Tool 'write_file' completed]

◆ 天気情報を取得し、weather_report.txt に保存しました。
```

---

## 実装フェーズ

### Phase 1: 基本的な MCP クライアント機能（stdio のみ）

- [x] 要件定義書作成
- [ ] `rust-mcp-sdk` の統合
- [ ] MCP 設定ファイル読み込み
- [ ] stdio トランスポートによるサーバ接続
- [ ] ツール一覧の取得
- [ ] ツール呼び出しの実装
- [ ] チャットループへの統合

### Phase 2: リソースとプロンプトテンプレート対応

- [ ] リソース一覧の取得
- [ ] リソース読み込み機能
- [ ] プロンプトテンプレート一覧の取得
- [ ] プロンプトテンプレート利用機能

### Phase 3: SSE / WebSocket 対応

- [ ] SSE トランスポートの実装
- [ ] WebSocket トランスポートの実装
- [ ] リモート MCP サーバとの通信

### Phase 4: 高度な機能

- [ ] Tasks（作業追跡）機能の実装
- [ ] Sampling（サーバーサイド LLM 呼び出し）
- [ ] 並列ツール呼び出し
- [ ] ツール実行のキャンセル機能

---

## テストケース

### 1. MCP クライアント

- [ ] 設定ファイルの読み込み
- [ ] stdio サーバへの接続
- [ ] ツール一覧の取得
- [ ] ツール呼び出しの成功
- [ ] ツール呼び出しの失敗（存在しないツール）
- [ ] 複数サーバとの接続

### 2. ツール検出

- [ ] JSON スタイルのツール呼び出し検出
- [ ] XML スタイルのツール呼び出し検出
- [ ] 複数ツール呼び出しの検出
- [ ] ツール呼び出しがない場合

### 3. 統合テスト

- [ ] ファイルシステムサーバとの統合
- [ ] 天気サーバとの統合
- [ ] マルチターン会話でのツール利用
- [ ] エラーハンドリング

---

## セキュリティ考慮事項

### 1. サンドボックス実行

MCP サーバが任意のコマンドを実行できるため、以下の制限を設ける：

- ホワイトリスト方式でサーバを限定
- システムディレクトリへのアクセス制限
- ネットワークアクセスの制限（オプション）

### 2. 権限管理

- ツール呼び出し前にユーザー確認（オプション）
- 危険な操作（削除、実行）には警告表示

### 3. データ保護

- MCP サーバとの通信内容のログ記録
- 機密情報の扱いに注意（API キーなど）

---

## 制限事項

### Phase 1 での制限

1. **通信方式**: stdio のみ対応（SSE/WebSocket は Phase 3）
2. **ツール呼び出し**: 同期的な実行のみ（並列実行は Phase 4）
3. **リソース**: 未対応（Phase 2）
4. **プロンプトテンプレート**: 未対応（Phase 2）

### 一般的な制限

1. **LLM の制約**
   - RKLLM 自体が関数呼び出し（Function Calling）をネイティブサポートしていないため、マーカーベースの検出に依存

2. **パフォーマンス**
   - ツール呼び出しごとに追加のラウンドトリップが発生
   - 大量のツール呼び出しは時間がかかる

3. **エラーリカバリ**
   - ツール実行失敗時の自動リトライは未実装

---

## 今後の拡張案

### 1. ツール自動選択

- LLM がツールを自動選択するための強化学習
- ツール利用履歴の学習

### 2. カスタムツールの作成

- Rust でカスタム MCP サーバを作成するテンプレート
- RKLLM 固有の機能を MCP ツールとして提供

### 3. GUI ツール管理

- ツールの有効/無効切り替え
- ツール実行履歴の可視化

### 4. クラウド統合

- 外部 API（OpenAI、Google など）を MCP ツールとして統合
- クラウドベースの MCP サーバ

---

## 参考資料

### 公式ドキュメント

- [MCP Specification (2025-11-25)](https://modelcontextprotocol.io/specification/2025-11-25)
- [Anthropic MCP Documentation](https://docs.anthropic.com/en/docs/mcp)
- [MCP GitHub Repository](https://github.com/modelcontextprotocol)

### Rust 実装

- [Official Rust SDK](https://github.com/modelcontextprotocol/rust-sdk)
- [rust-mcp-sdk (crates.io)](https://crates.io/crates/rust-mcp-sdk)
- [Build MCP Servers in Rust - Complete Guide](https://mcpcat.io/guides/building-mcp-server-rust/)
- [How to Build a stdio MCP Server in Rust](https://www.shuttle.dev/blog/2025/07/18/how-to-build-a-stdio-mcp-server-in-rust)

### チュートリアル

- [Introduction to Model Context Protocol (Anthropic Courses)](https://anthropic.skilljar.com/introduction-to-model-context-protocol)
- [Build a Weather MCP Server with Rust](https://paulyu.dev/article/rust-mcp-server-weather-tutorial/)

### その他

- [Anthropic Model Context Protocol Announcement](https://www.anthropic.com/news/model-context-protocol)
- [One Year of MCP: November 2025 Spec Release](https://blog.modelcontextprotocol.io/posts/2025-11-25-first-mcp-anniversary/)

---

## 完成予定

**Phase 1 目標**: 2025年12月末

stdio ベースの基本的な MCP クライアント機能を完成させ、ファイルシステムサーバなどの標準的な MCP サーバと連携できるようにする。
