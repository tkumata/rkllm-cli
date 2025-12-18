# tool-only モード運用ガイド

## 概要

`--tool-only` は MCP ツール経由で環境操作を行うためのモードです。ローカルファイルへの直接書き込みを禁止し、LLM が生成した `<file>` / `[CREATE_FILE: ...]` ブロックは MCP の書き込みツール（例: `write_file`）に転送されます。

## 前提条件

- `--mcp-config` で MCP サーバに接続すること（必須）。少なくともパスとコンテンツを受け取れる書き込みツールが必要です。
  - 例: `@modelcontextprotocol/server-filesystem`（npx 経由で `/path/to/workspace` を指定）
- LLM がツール呼び出しを出力できるテンプレを使用すること。

## 起動例

```bash
./target/release/rkllm-cli chat \
  --model /path/to/model.rkllm \
  --mcp-config mcp_config.toml \
  --tool-only
```

## 期待される挙動

- ローカルへの直接書き込みは行わず、検出したファイル出力は MCP 書き込みツールに転送される。
- `write_file` が存在すれば最優先で使用し、名前に `write`/`file` を含むツールや `path` と `content` を持つツールにフォールバックする。
- 書き込みツールが見つからない場合はスキップし、警告を表示する。

## LLM への指示テンプレ

ツール呼び出しは `[TOOL_CALL] ... [END_TOOL_CALL]` 形式で記述します。例:

```
[TOOL_CALL]
{
  "name": "write_file",
  "arguments": {
    "path": "README-ja.md",
    "content": "翻訳内容 ..."
  }
}
[END_TOOL_CALL]
```

## ベストプラクティス

- 書き込みが必要なときは、`path` と `content` を持つ書き込みツールの有無を MCP ログで確認する。
- 入力ファイルを読み込ませる場合でも `<file>` 出力は必ずツール経由になるため、書き込み先パスを明示する。
- 変換・翻訳などで長いコンテンツを書く場合は、1 回のツール呼び出しで完結するよう促す。
- ツールが失敗した場合の再実行手順を LLM に案内させるか、ユーザーが再送できるようにする。

## トラブルシューティング

- **起動時にエラー**: MCP クライアント未接続だと `--tool-only requires MCP tools...` で終了します。`--mcp-config` を指定してください。
- **書き込みツールが見つからない**: MCP サーバに `write_file` 相当のツールを追加するか、`path` と `content` を受け取れるツールを用意してください。
