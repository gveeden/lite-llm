# lite-llm

A Rust HTTP server that runs LLMs locally via [LiteRT-LM](https://github.com/google-ai-edge/LiteRT-LM) and exposes a streaming chat API with a data-driven tool execution engine.

Primary use case: natural language tool calling (smart home, weather, etc.) over a local HTTP/SSE API — no cloud required.

---

## Requirements

- Linux, aarch64 or x86_64
- Rust 1.78+
- LiteRT-LM native libraries (`libengine.so` and friends)

---

## Getting the native libraries

### Download from releases (recommended)

```bash
mkdir -p ~/litert-lm-libs

# Replace v0.1.0 and the filename with the current release for your platform
gh release download v0.1.0 \
  --repo gveeden/lite-llm \
  --pattern '*.so' \
  --dir ~/litert-lm-libs
```

Or download manually from the [Releases page](https://github.com/gveeden/lite-llm/releases) and place the `.so` files in a directory of your choice.

| Release asset | Platform |
|---|---|
| `litert-lm-libs-linux-aarch64-asahi.tar.gz` | Asahi Linux (Apple M-series) |
| `litert-lm-libs-linux-aarch64-pi.tar.gz` | Raspberry Pi 4/5, 64-bit OS |

### Build from source

See [building-litert-lm.md](building-litert-lm.md) for full build instructions.

---

## Building lite-llm

```bash
git clone https://github.com/gveeden/lite-llm.git
cd lite-llm

LITERT_LM_LIB_PATH=~/litert-lm-libs cargo build --release
```

`LITERT_LM_LIB_PATH` is only needed at build time. The path is baked into the binary's RPATH so you do not need to set `LD_LIBRARY_PATH` at runtime.

---

## Running

```bash
./target/release/lite-llm
```

The server starts on `http://localhost:8080` by default. Copy `config.toml` to configure the port, database path, and an optional startup model.

### Download and load a model

```bash
# Download the default model (gemma-4-E2B-it) — streams progress
curl -s -N -X POST http://localhost:8080/models/download

# Or specify a model
curl -s -N -X POST http://localhost:8080/models/download \
  -H 'Content-Type: application/json' \
  -d '{"repo_id": "litert-community/gemma-4-E2B-it-litert-lm"}'
```

---

## Usage

See [examples.md](examples.md) for full curl examples covering:

- Model download and loading
- Registering HTTP and MQTT tools
- `/chat` — streaming chat with optional tool use
- `/command` — full NL → tool execution → answer loop
- Session history

### Quick example

```bash
# Register a weather tool
curl -s -X POST http://localhost:8080/tools \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "get_weather",
    "description": "Get current weather for a location",
    "parameters": {
      "type": "object",
      "properties": {
        "location": {"type": "string"}
      },
      "required": ["location"]
    },
    "handler": {
      "type": "http",
      "method": "GET",
      "url": "https://wttr.in/{location}?format=4",
      "headers": {}
    }
  }'

# Ask a question that uses the tool
curl -s -N -X POST http://localhost:8080/command \
  -H 'Content-Type: application/json' \
  -d '{"text": "What is the weather in London?"}'
```

---

## Configuration

`config.toml`:

```toml
[server]
port = 8080
host = "127.0.0.1"

[model]
# Optional: load this model at startup
repo_id = "litert-community/gemma-4-E2B-it-litert-lm"
model_id = "gemma-4-E2B-it"

[db]
path = "~/.local/share/lite-llm/lite-llm.db"
```

For gated HuggingFace models set `HF_TOKEN` before starting:

```bash
HF_TOKEN=hf_xxx ./lite-llm
```
