# RKLLM CLI

Rust implementation of a CLI tool for chatting with LLM models on Rockchip NPU using the `librkllmrt.so` library.

## Project Overview

This project provides a command-line interface to interact with Large Language Models (LLMs) running on Rockchip NPU hardware (rk3588, rk3576). It uses Rust FFI bindings to communicate with the native `librkllmrt.so` library.

I couldn't understand why Rockchip's samples use the `adb` command. That's why I created this program. I just want to use the NPU on the Rock5B by itself.

| Model                  | Token Per Second | Notes                                                |
| ---------------------- | ---------------- | ---------------------------------------------------- |
| Qwen3 1.7B             | 11.9             | Fast but accuracy is poor                            |
| Qwen3 4B               | 2.9              | The accuracy improves slightly, but it is very slow. |
| Gemma3 1B              | 12               | Fast but MCP client does not work                    |
| Gemma3 4B              | 2.9              | The accuracy improves slightly, but it is very slow. |
| DeepSeek R1 Distill 7B | ---              | Not work. LLM responces "Alright. ppppppppppppp...." |

DeepSeek R1 Distill 7B does not run because of this program.

## Architecture

This repository contains the "Agentic CLI & MCP Client (Rust)" based on the architecture described below. Other repositories (written in Rust and C++) will be released in the future.

The Rock5B's NPU delivers only 6 TOPS, so please don't expect too much.

```text
      ┌──────┐
      │ User │
      └──┬───┘
┌────────│────────────────────────── Rock5B ─┐
│        ▼                                   │
│ ┌──────────────┐    ┌───────────────┐      │
│ │ Agentic CLI  │◀︎──▶︎│ librkllmrt.so │      │
│ │ & MCP Client │    └┬──────────────┘      │
│ │ (Rust)       │     │  ┌───────────┐      │
│ └──────────────┘     └─▶︎│ LLM Model │      │
│        │                └───────────┘      │
│        ├─────────────┬─────────────┐       │
│        ▼             ▼             ▼       │
│ ┌────────────┐┌────────────┐┌────────────┐ │
│ │ MCP Server ││ MCP Server ││ MCP Server │ │
│ ├────────────┤├────────────┤├────────────┤ │
│ │ DB Adapter ││ filesystem ││ ripgrep    │ │
│ │ (Rust)     ││ (npx)      ││ (npx)      │ │
│ └──────┬─────┘└────────────┘└────────────┘ │
│        ▼                                   │
│ ┌────────────────────────────────────────┐ │
│ │ SQLite3                                │ │
│ ├────────────────────┬───────────────────┤ │
│ │ Sensor1 Data       │ Sensor2 Data      │ │
│ └────────────────────┴───────────────────┘ │
│        ▲                       ▲           │
│ ┌──────┴───────────────────────┴─────────┐ │
│ │ REST API as a Edge Server (Rust)       │ │
│ │   /api/climate/save                    │ │
│ │   /api/hmmd/save                       │ │
│ └────────────────────────────────────────┘ │
│        ▲                       ▲           │
└────────│───────────────────────│───────────┘
┌────────┴────────────┐┌─────────┴───────────┐
│ MCU + Sensor1 (C++) ││ MCU + Sensor2 (C++) │
└─────────────────────┘└─────────────────────┘
```

## Features

- **Interactive Chat**: Command-line chat interface with streaming output and multiline editing (arrow keys for cursor movement, insert at cursor, Shift+Enter for newline)
- **Safe Rust Wrapper**: Type-safe Rust bindings for the C library
- **UTF-8 Handling**: Proper handling of incomplete multi-byte UTF-8 sequences during streaming
- **Error Handling**: Comprehensive error handling with `anyhow`
- **File in/out pipeline**: Read specified files → transform (translate/summarize/append) → write to specified output paths. Source files are not overwritten unless explicitly instructed.
- **Writing files**: Write local files via `<file path="..."> ... </file>` format (bracket format is also accepted)
- **Prompt preview & write confirmation**: `--preview-prompt` (or `RKLLM_DEBUG_PROMPT=1`) to print the composed prompt, `--confirm-writes` to ask before every write.
- **Tool-only mode**: `--tool-only` uses MCP tools only (requires `--mcp-config`); local writes are disabled and file outputs are sent to the MCP write tool when available.
- **MCP client**: Connect to MCP server; tool list (short form) is always included in the system prompt with per-tool JSON samples for `[TOOL_CALL]` usage.
- **Chat templates & timeouts**: Switch template via `RKLLM_TEMPLATE=qwen|gemma`; adjust generation timeout via `RKLLM_INFER_TIMEOUT_SECS` and file load size via `RKLLM_MAX_FILE_SIZE`.

## Prerequisites

### Hardware

- Rockchip board (rk3588 or rk3576) with NPU support
- Example: Rock5B running Armbian (aarch64)

### Software

- Rust toolchain (for aarch64-unknown-linux-gnu if cross-compiling)
- `librkllmrt.so` library (provided by Rockchip)
- RKLLM model file (`.rkllm` format)

## Setup

### 1. Place the Shared Library

Copy `librkllmrt.so` to the `src/lib` directory:

```bash
cp /path/to/librkllmrt.so src/lib/
```

Alternatively, you can place it in a system library path on your target device:

```bash
sudo cp librkllmrt.so /usr/local/lib/
sudo ldconfig
```

### 2. Build the Project

For native build on the target device:

```bash
cargo build --release
```

For cross-compilation from Mac/Linux:

```bash
# Install cross-compilation toolchain
rustup target add aarch64-unknown-linux-gnu

# Build
cargo build --release --target aarch64-unknown-linux-gnu
```

The binary will be located at:

- Native: `target/release/rkllm-cli`
- Cross-compiled: `target/aarch64-unknown-linux-gnu/release/rkllm-cli`

## Usage

### Start a Chat Session

```bash
./target/release/rkllm-cli chat --model /path/to/your/model.rkllm

# Common flags
--mcp-config mcp_config.toml    # enable MCP tools
--preview-prompt                # print the composed prompt before sending
--confirm-writes[=true|false]   # ask before every file write (default: true)
--tool-only                     # MCP tools only; disable local file writes and forward outputs to MCP (requires --mcp-config)
```

#### MCP tools and samples

- If `--mcp-config` connects successfully, the system prompt automatically lists available tools (short form) and adds a `[TOOL_CALL]` sample per tool, e.g.:
  ```
  [TOOL_CALL]
  {
    "name": "list_directory",
    "arguments": {
      "path": "/tmp"
    }
  }
  [END_TOOL_CALL]
  ```
- Use `RKLLM_DEBUG_PROMPT=1` with `--preview-prompt` to inspect the exact `<tools>` section if needed.

### Example

![screenshot](./docs/screenshot.png)

### Commands

- Type your message and press Enter to send
- Use arrow keys to move the cursor across lines; text is inserted at the cursor. Shift+Enter (or Ctrl+J) inserts a newline.
- Type `exit` or `quit` to end the session
- Press `Ctrl+C and Ctrl+C` to interrupt and exit

## Project Structure

```
rkllm-cli/
├── Cargo.toml           # Rust package configuration
├── build.rs             # Build script for linking librkllmrt.so
├── src/
│   ├── main.rs          # CLI entry point
│   ├── ffi.rs           # FFI bindings for librkllmrt.so
│   ├── llm.rs           # Safe Rust wrapper for RKLLM
│   ├── chat.rs          # Chat session logic
│   └── lib/
│       └── librkllmrt.so  # Rockchip RKLLM runtime library (place here)
├── sample/
│   └── gradio_server.py   # Python reference implementation
└── docs/
    └── RKLLM_RUST_CLI_REQUIREMENTS.md  # Implementation requirements
```

## Implementation Details

### FFI Bindings (ffi.rs)

- Uses `#[repr(C)]` for C-compatible structures
- Defines all RKLLM API functions and data types
- Provides type-safe enums and structures

### RKLLM Wrapper (llm.rs)

- Safe Rust wrapper around the C library
- Handles callback registration and UTF-8 decoding
- Manages incomplete multi-byte sequences during streaming
- Automatic resource cleanup with Drop trait

### Chat Logic (chat.rs)

- Interactive multiline interface using `crossterm` (arrow-key cursor 移動・挿入、Shift+Enter で改行)
- Streaming output support
- File operations: detects referenced paths, loads existing files as context, treats missing paths as output targets, and remaps single-target outputs when the model writes to the input path.

## Configuration

The model is initialized with the following default parameters (can be modified in `llm.rs`):

- `max_context_len`: 4096
- `max_new_tokens`: -1 (unlimited)
- `top_k`: 20
- `top_p`: 0.8
- `temperature`: 0.7
- `repeat_penalty`: 1.0
- `skip_special_token`: true

## Troubleshooting

### Library Not Found

If you get an error about `librkllmrt.so` not being found:

1. Make sure the library is in `src/lib/` directory
2. Or set `LD_LIBRARY_PATH`:
   ```bash
   export LD_LIBRARY_PATH=/path/to/lib:$LD_LIBRARY_PATH
   ./rkllm-cli chat --model model.rkllm
   ```

### Model Loading Fails

- Verify the model file path is correct
- Ensure the model is in RKLLM format (`.rkllm`)
- Check that you have sufficient memory on the device

## License

This project is provided as-is for use with Rockchip NPU hardware.

## Reference

- Python implementation: `sample/gradio_server.py`
