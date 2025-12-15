# MCP (Model Context Protocol) 機能

## 概要

RKLLM CLI は Model Context Protocol (MCP) をサポートしており、外部ツールと連携できます。

## 使い方

### 1. MCP 設定ファイルを作成

`mcp_config.toml` を作成して、使用する MCP サーバを定義します：

```toml
[[servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/home/user/project"]
```

### 2. RKLLM CLI を起動

MCP 設定ファイルを指定して起動します：

```bash
./target/release/rkllm-cli chat --model /path/to/model.rkllm --mcp-config mcp_config.toml
```

### 3. ツールを使う

LLM にツールの使用を指示します。ツール呼び出しは以下の形式で検出されます：

#### JSON スタイル

```
[TOOL_CALL]
{
  "name": "list_directory",
  "arguments": {
    "path": "/home/user"
  }
}
[END_TOOL_CALL]
```

#### XML スタイル

```xml
<tool_call name="read_file">
  <argument name="path">/home/user/file.txt</argument>
</tool_call>
```

## 利用可能な MCP サーバ

### 公式サーバ

- **filesystem**: ファイルシステム操作
  ```toml
  [[servers]]
  name = "filesystem"
  command = "npx"
  args = ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/directory"]
  ```

- **github**: GitHub リポジトリ操作
  ```toml
  [[servers]]
  name = "github"
  command = "npx"
  args = ["-y", "@modelcontextprotocol/server-github"]

  [servers.env]
  GITHUB_TOKEN = "your_github_token"
  ```

- **sqlite**: SQLite データベース操作
  ```toml
  [[servers]]
  name = "sqlite"
  command = "npx"
  args = ["-y", "@modelcontextprotocol/server-sqlite", "/path/to/database.db"]
  ```

### カスタムサーバ

独自の MCP サーバを作成して使用することもできます。

## トラブルシューティング

### Node.js が必要

多くの MCP サーバは Node.js で実装されているため、`npx` を使うには Node.js のインストールが必要です：

```bash
# Ubuntu/Debian
sudo apt install nodejs npm

# または nvm を使用
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.39.0/install.sh | bash
nvm install node
```

### サーバが起動しない

エラーメッセージを確認し、以下を確認してください：

- `command` と `args` が正しいか
- サーバの実行ファイルが存在するか
- 必要な環境変数が設定されているか

## 参考資料

- [Model Context Protocol 公式サイト](https://modelcontextprotocol.io/)
- [MCP Specification](https://modelcontextprotocol.io/specification/2025-11-25)
- [公式 MCP サーバ一覧](https://github.com/modelcontextprotocol)
