# モデルテンプレートと推奨設定 (RK3588 / rkllm-cli)

## Qwen3 1.7B (推奨デフォルト)

- テンプレ: `RKLLM_TEMPLATE=qwen`
- 推奨パラメータ例: `max_new_tokens=1024~2048`, `top_k=40~64`, `top_p=0.9~0.95`, `temperature=0.7~0.9`, `repeat_penalty=1.05~1.1`
- 備考: RK3588 では int4/int8 量子化モデルを優先。`max_new_tokens` を抑えると TPS 向上。

## Gemma3 4B (従来テンプレ)

- テンプレ: デフォルト（`RKLLM_TEMPLATE` 未設定時）
- 推奨パラメータ例: `max_new_tokens=1024~1536`, `top_k=32~64`, `top_p=0.9`, `temperature=0.7~0.9`
- 備考: Qwen より負荷が高め。必要に応じて `repeat_penalty` を少し上げてループを抑制。

## 追加モデルを使う場合

- テンプレ切替: `RKLLM_TEMPLATE=<name>` を環境変数で指定し、テンプレ文字列を `src/llm.rs` に追加する。
- 推奨: テンプレは system/user/assistant のスタイルをモデルに合わせて定義し、`build_chat_prompt` で system/user/context/tools の 4 段構成に整合するようにする。
- パラメータ調整: RK3588 の場合、メモリと TPS のバランスを見ながら `max_new_tokens` を下げ、`top_k/top_p/temperature` を保守的に設定する。
- 運用: 小型モデルではプロンプトを短く保つため、ツール一覧は短縮版のみ表示する（`RKLLM_SHOW_TOOL_LIST=1`）か非表示を推奨。
