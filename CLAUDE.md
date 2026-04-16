# lite-llm

A Rust HTTP server that runs LLMs locally via LiteRT-LM and exposes a streaming chat API with a generic tool execution engine. Primary use case: tool calling (smart home control, weather, etc.) via natural language over HTTP.

## Context

This design was worked out over a full planning session. Do not re-derive architecture from scratch — implement what is described here.

The project is on Linux at `~/dev/lite-llm`.

---

## LiteRT-LM

- Repo: https://github.com/google-ai-edge/LiteRT-LM
- C++ library from Google for on-device LLM inference
- Has a **C API** (`extern "C"`) with opaque pointer types: `LiteRtLmEngine`, `LiteRtLmSession`, `LiteRtLmResponses`
- Pre-built as `libengine.so` (Linux) / `libengine.dylib` (macOS) via Bazel upstream — we just link against it
- Models are pulled from HuggingFace by repo ID (built into the library)
- Supports streaming via callback: `LiteRtLmStreamCallback(userdata, chunk, is_final, error_msg)`
- Supports tool/function calling

### FFI Approach

Follow https://github.com/maceip/litert-lm-rs exactly:
- Use `bindgen` (not `cxx`) to generate bindings from the C header
- Allowlist: `litert_lm_.*` functions, `LiteRtLm.*` types, `InputData.*`, `kInput.*` vars
- Link `dylib=engine`; respect `LITERT_LM_LIB_PATH` env var for path override
- Platform-specific C++ stdlib: `dylib=c++` on macOS, `dylib=stdc++` on Linux

```rust
// build.rs pattern (from litert-lm-rs)
let bindings = bindgen::Builder::default()
    .header("src/ffi/engine.h")
    .allowlist_function("litert_lm_.*")
    .allowlist_type("LiteRtLm.*")
    .allowlist_type("InputData.*")
    .allowlist_var("kInput.*")
    .generate_comments(true)
    .generate()?;

println!("cargo:rustc-link-lib=dylib=engine");
if let Ok(p) = env::var("LITERT_LM_LIB_PATH") {
    println!("cargo:rustc-link-search=native={p}");
}
```

Safe wrappers in `src/ffi/mod.rs`:
- `Engine`: `*mut LiteRtLmEngine` + settings ptr, `Send + Sync`, RAII Drop
- `Session`: `*mut LiteRtLmSession`, `Send` only, RAII Drop
- Streaming: box a `tokio::sync::mpsc::UnboundedSender<StreamChunk>` as `*mut c_void` userdata, cast back in the C callback and call `.send()`

---

## Project Layout

```
lite-llm/
├── CLAUDE.md
├── build.rs
├── Cargo.toml
├── config.toml                   # default config (gitignored for local overrides)
├── src/
│   ├── main.rs                   # CLI: parse args, init DB, start server
│   ├── config.rs                 # TOML config struct
│   ├── db.rs                     # sqlx pool init + migrations
│   ├── migrations/
│   │   └── 001_init.sql
│   ├── ffi/
│   │   ├── engine.h              # LiteRT-LM C header (copy from LiteRT-LM repo)
│   │   └── mod.rs                # Engine + Session safe wrappers
│   ├── engine/
│   │   ├── mod.rs
│   │   ├── model_manager.rs      # Load/unload models by HF repo+model ID
│   │   └── session.rs            # Chat sessions, history, tool loop; injects datetime into system prompt
│   ├── tools/
│   │   ├── mod.rs                # ToolDefinition + ToolHandler structs
│   │   ├── registry.rs           # ToolRegistry: load/store tools from SQLite
│   │   └── executor.rs           # Generic dispatcher: HTTP and MQTT handlers
│   └── api/
│       ├── router.rs             # Axum router wiring
│       ├── chat.rs               # POST /chat → SSE stream
│       ├── command.rs            # POST /command (NL → tool loop → SSE answer)
│       ├── models.rs             # POST /models/load, GET /models
│       ├── tools.rs              # GET/POST/DELETE /tools
│       └── sessions.rs           # GET /sessions, GET /sessions/:id/messages
```

---

## Tools

There are no hardcoded tool implementations. All tools are data — registered via the API or seeded into SQLite, then executed by the generic dispatcher.

### ToolDefinition

```rust
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,  // JSON Schema object
    pub handler: ToolHandler,
    pub enabled: bool,
}

pub enum ToolHandler {
    Http {
        method: String,           // "GET", "POST", etc.
        url: String,              // may contain {param} placeholders
        headers: HashMap<String, String>,
        body: Option<String>,     // template string with {param} placeholders
    },
    Mqtt {
        broker: String,           // "host:port"
        command_topic: String,    // may contain {param} placeholders
        payload: String,          // template string with {param} placeholders
        response_topic: Option<String>,  // if None, fire-and-forget
        timeout_ms: u64,
    },
}
```

### Parameter substitution

`{param_name}` placeholders in URL, topic, body, and payload are replaced with the corresponding argument value from the model's tool call. Unknown placeholders are left as-is.

### executor.rs

Single entry point:
```rust
pub async fn execute(tool: &ToolDefinition, args: serde_json::Value) -> anyhow::Result<String>
```

- Substitutes args into the handler template
- HTTP: uses `reqwest` to make the request, returns the response body as a string
- MQTT: publishes to `command_topic`; if `response_topic` is set, subscribes and waits up to `timeout_ms` for one message, returns its payload; otherwise returns `"ok"`
- The returned string is fed back to the model verbatim

### Datetime injection

Current date and time are **not** a tool. `session.rs` prepends them to the system prompt on every turn:

```
Today is {weekday}, {date}. The current time is {HH:MM} {timezone}.
```

---

## SQLite Schema (`src/migrations/001_init.sql`)

```sql
CREATE TABLE sessions (
    id          TEXT PRIMARY KEY,
    model_id    TEXT NOT NULL,
    title       TEXT,
    created_at  INTEGER NOT NULL,
    last_used   INTEGER NOT NULL
);

CREATE TABLE messages (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role        TEXT NOT NULL,        -- 'user' | 'assistant' | 'tool'
    content     TEXT NOT NULL,
    tool_call   TEXT,                 -- JSON: {"tool": "...", "parameters": {...}}
    tool_result TEXT,                 -- JSON: {"tool": "...", "success": bool, "result": "..."}
    created_at  INTEGER NOT NULL
);

CREATE TABLE tools (
    name        TEXT PRIMARY KEY,
    description TEXT NOT NULL,
    parameters  TEXT NOT NULL,        -- JSON Schema
    handler     TEXT NOT NULL,        -- JSON: {"type": "http"|"mqtt", ...config}
    enabled     INTEGER NOT NULL DEFAULT 1,
    created_at  INTEGER NOT NULL
);
```

---

## HTTP API

All responses are JSON. Streaming uses **Server-Sent Events (SSE)**.

### `POST /models/load`
```json
// Request
{"repo_id": "google/gemma-3n-E2B-it-litert-lm", "model_id": "gemma-3n-E2B-it-int4"}
// Response
{"model_handle": "gemma-3n-E2B-it-int4", "status": "loaded"}
```

### `GET /models`
```json
{"loaded": ["gemma-3n-E2B-it-int4"], "active": "gemma-3n-E2B-it-int4"}
```

### `POST /chat` → SSE
```json
// Request
{
  "model": "gemma-3n-E2B-it-int4",   // optional if only one loaded
  "session_id": "sess_abc123",        // optional, creates new session if omitted
  "messages": [{"role": "user", "content": "What lights are on?"}],
  "tools": ["control_smart_home"],    // optional: tool names to enable
  "stream": true
}
// SSE stream
data: {"delta": "The living room light is on.", "done": false}
data: {"delta": "", "done": true, "tool_calls": []}
```

When the model emits a tool call, the server returns it and stops — the client is responsible for executing and continuing:
```
data: {"delta": "", "done": true, "tool_calls": [
  {"id": "tc_1", "name": "control_smart_home", "arguments": {"device": "living room light", "action": "turn_on"}}
]}
```

### `POST /command` → SSE (preferred endpoint)
Runs the full NL → LLM → tool dispatch → LLM → answer loop server-side.
```json
// Request
{
  "model": "gemma-3n-E2B-it-int4",
  "text": "Turn on the living room light",
  "tools": ["control_smart_home"],
  "stream": true
}
// SSE stream
data: {"delta": "Done! The living room light is now on.", "done": false, "phase": "answer"}
data: {"delta": "", "done": true, "tool_executions": [
  {"tool": "control_smart_home", "args": {"device": "living room light", "action": "turn_on"}, "result": "ok"}
]}
```

### `GET /sessions`
### `GET /sessions/:id/messages`

### `GET /tools`
```json
[
  {
    "name": "control_smart_home",
    "description": "Turn a smart home device on or off",
    "parameters": {"type": "object", "properties": {"device": {"type": "string"}, "action": {"type": "string", "enum": ["turn_on", "turn_off"]}}},
    "handler": {"type": "mqtt", "broker": "raspberrypi.local:1883", "command_topic": "felix/homekit/command/{device}", "payload": "{\"action\": \"{action}\"}", "timeout_ms": 3000},
    "enabled": true
  }
]
```

### `POST /tools` — register a tool
```json
{
  "name": "get_weather",
  "description": "Get current weather for a location",
  "parameters": {"type": "object", "properties": {"location": {"type": "string"}}, "required": ["location"]},
  "handler": {"type": "http", "method": "GET", "url": "https://wttr.in/{location}?format=j1", "headers": {}}
}
```

### `DELETE /tools/:name`

---

## Config (`config.toml`)

```toml
[server]
port = 8080
host = "127.0.0.1"

[model]
# Optional: load this model at startup
repo_id = "google/gemma-3n-E2B-it-litert-lm"
model_id = "gemma-3n-E2B-it-int4"

[db]
path = "~/.local/share/lite-llm/lite-llm.db"
```

---

## Cargo.toml Dependencies

```toml
[build-dependencies]
bindgen = "0.70"

[dependencies]
axum = { version = "0.7", features = ["macros"] }
tokio = { version = "1", features = ["full"] }
tokio-stream = "0.1"
sqlx = { version = "0.7", features = ["sqlite", "runtime-tokio", "macros"] }
rumqttc = "0.24"
reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }
jsonschema = "0.26"
chrono = "0.4"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
anyhow = "1.0"
clap = { version = "4.5", features = ["derive"] }
tracing = "0.1"
tracing-subscriber = "0.3"
uuid = { version = "1", features = ["v4"] }
```

---

## Startup Sequence

1. Parse CLI args / load `config.toml`
2. Open SQLite pool at configured path, run migrations
3. Load all tools from `tools` table into `ToolRegistry`
4. Load LiteRT-LM model if `[model]` section is configured
5. Start Axum HTTP server

---

## Implementation Order

1. **`build.rs` + `src/ffi/mod.rs`** — FFI bridge. Smoke test: `Engine::new()` + `Session::generate("hello")`
2. **`db.rs` + `src/migrations/001_init.sql`** — sqlx pool, run migrations on startup
3. **`tools/mod.rs` + `tools/registry.rs` + `tools/executor.rs`** — ToolDefinition structs, SQLite CRUD, HTTP + MQTT dispatch
4. **`/chat` endpoint** — blocking first, swap in SSE streaming after; datetime injected in session.rs
5. **`/command` endpoint** — full NL → tool dispatch loop
6. **`/tools` + `/sessions` CRUD**

---

## Key Decisions (do not re-debate)

- FFI: `bindgen` not `cxx`
- Streaming: SSE not WebSockets
- Tools: generic data-driven executor only — no hardcoded tool implementations
- Tool handlers: `http` and `mqtt` in v1; no shell handler
- MQTT broker config is per-tool inside the handler JSON, not a global server config
- MQTT client for tool execution is lazy — instantiated on first MQTT tool call
- Datetime (date + time + timezone) is injected into the system prompt on every session turn, not a tool
- SQLite: via `sqlx` with compile-time query checking
- No device registry in this service — device management is a separate concern
- Do not use anything from `~/dev/vox`, `~/dev/vox-companion`, or `~/dev/felix`
