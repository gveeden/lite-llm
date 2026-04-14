# lite-llm

A Rust HTTP server that runs LLMs locally via LiteRT-LM and exposes a streaming chat API with a built-in tool execution engine. Primary use case: tool calling (smart home control, weather, etc.) via natural language over HTTP.

## Context

This design was worked out over a full planning session. Do not re-derive architecture from scratch — implement what is described here.

The project is on Linux at `~/dev/lite-llm`. Felix (`~/dev/felix`) is a sibling Rust project that already has tool infrastructure we are porting directly. Check it first before writing anything tool-related.

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
│   │   └── session.rs            # Chat sessions, history, tool loop
│   ├── mqtt.rs                   # Lazy MQTT client (connects to external broker)
│   ├── mqtt_devices.rs           # DeviceRegistry + SmartDevice — port from Felix
│   ├── tools/
│   │   ├── mod.rs                # Tool trait + ToolDefinition — port from Felix
│   │   ├── registry.rs           # ToolRegistry — port from Felix, add SQLite persistence
│   │   ├── weather.rs            # GetWeather — port from Felix (async reqwest)
│   │   ├── current_time.rs       # GetCurrentTime + GetCurrentDate — port from Felix
│   │   └── smart_home.rs         # ControlSmartHome + ListSmartDevices — port from Felix
│   └── api/
│       ├── router.rs             # Axum router wiring
│       ├── chat.rs               # POST /chat → SSE stream
│       ├── command.rs            # POST /command (NL → tool loop → SSE answer)
│       ├── models.rs             # POST /models/load, GET /models
│       ├── tools.rs              # GET/POST/DELETE /tools
│       └── sessions.rs           # GET /sessions, GET /sessions/:id/messages
```

---

## Felix Tool Ports

Felix is at `~/dev/felix`. These files map directly:

| Felix file | Destination | Notes |
|---|---|---|
| `src/tools/mod.rs` | `src/tools/mod.rs` | Port as-is. `Tool` trait + `ToolDefinition` |
| `src/tools/registry.rs` | `src/tools/registry.rs` | Port + add SQLite persistence on top |
| `src/tools/weather.rs` | `src/tools/weather.rs` | Port, switch blocking reqwest → async |
| `src/tools/current_time.rs` | `src/tools/current_time.rs` | Port as-is |
| `src/tools/smart_home.rs` | `src/tools/smart_home.rs` | Port as-is |
| `src/mqtt_devices.rs` | `src/mqtt_devices.rs` | Port as-is. All payload generation logic is here |
| `src/mqtt.rs` | `src/mqtt.rs` | Port as-is |

Do NOT port: `felix-homekit-bridge/`, `src/audio*.rs`, `src/vad.rs`, `src/stt.rs`, `src/tts.rs`, `src/wakeword.rs`. Those are Felix-specific.

Do NOT use anything from `~/dev/vox` or `~/dev/vox-companion`.

### MQTT: Client Only

There is no embedded MQTT broker. The MQTT client (`rumqttc`) connects to an **external** broker (default: `raspberrypi.local:1883`). The client is lazy — initialized on first use by `ControlSmartHome` or `ListSmartDevices`. The server starts cleanly with no broker present.

At startup, attempt to sync devices from the HomeKit bridge via MQTT (`felix/homekit/command/list` → `felix/homekit/response/list`). If the broker is unreachable, load last-known devices from SQLite instead.

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

-- User-defined / HTTP tools (built-ins are registered in code, not here)
CREATE TABLE tools (
    name        TEXT PRIMARY KEY,
    group_name  TEXT NOT NULL,
    description TEXT NOT NULL,
    parameters  TEXT NOT NULL,        -- JSON Schema
    handler     TEXT NOT NULL,        -- JSON: {"type": "http"|"shell", ...config}
    enabled     INTEGER NOT NULL DEFAULT 1,
    created_at  INTEGER NOT NULL
);

-- Device registry persisted so server is usable if broker is temporarily down
CREATE TABLE devices (
    id                        TEXT PRIMARY KEY,
    name                      TEXT NOT NULL,
    device_type               TEXT NOT NULL,
    command_topic             TEXT NOT NULL,
    state_topic               TEXT,
    capabilities              TEXT NOT NULL,    -- JSON array
    homebridge_characteristic TEXT,
    last_seen                 INTEGER NOT NULL
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
  "tools": ["smart_home"],            // optional: tool groups to enable
  "stream": true
}
// SSE stream
data: {"delta": "The living room light is on.", "done": false}
data: {"delta": "", "done": true, "tool_calls": []}
```

When the model calls a tool, the server returns:
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
  "tool_groups": ["smart_home"],
  "stream": true
}
// SSE stream
data: {"delta": "Done! The living room light is now on.", "done": false, "phase": "answer"}
data: {"delta": "", "done": true, "tool_executions": [
  {"tool": "control_smart_home", "args": {"device": "living room light", "action": "turn_on"}, "result": "living room light: turn_on"}
]}
```

### `GET /sessions`
### `GET /sessions/:id/messages`
### `GET /tools`
### `POST /tools` — register a custom tool
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

[mqtt]
enabled = true
broker = "raspberrypi.local"
port = 1883
client_id = "lite-llm"

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
urlencoding = "2.1"
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
3. Init `ToolRegistry`: register all built-in tools (weather, time, smart_home)
4. Load any custom tools from `tools` table in SQLite
5. Load persisted devices from SQLite into `DeviceRegistry`
6. Spawn background task: connect MQTT, run `sync_homekit_devices`, persist new/updated devices to SQLite
7. Load LiteRT-LM model if `[model]` section is configured
8. Start Axum HTTP server

---

## Implementation Order

1. **`build.rs` + `src/ffi/mod.rs`** — FFI bridge. Smoke test: `Engine::new()` + `Session::generate("hello")`
2. **`db.rs` + `src/migrations/001_init.sql`** — sqlx pool, run migrations on startup
3. **Port Felix tools** — `Tool` trait, `ToolRegistry`, `weather`, `current_time`, `smart_home`, `mqtt_devices`, `mqtt`
4. **`/chat` endpoint** — blocking first, swap in SSE streaming after
5. **`/command` endpoint** — full NL → tool dispatch loop
6. **MQTT + device persistence** — lazy client init, homekit sync, SQLite device cache
7. **`/tools` + `/sessions` CRUD**

---

## Key Decisions (do not re-debate)

- FFI: `bindgen` not `cxx`
- Streaming: SSE not WebSockets
- MQTT: external broker only, client is lazy-init, not started at server boot
- SQLite: via `sqlx` with compile-time query checking
- Tool built-ins: registered in code; SQLite `tools` table is only for user-defined/HTTP tools
- DeviceRegistry: in-memory (fast) backed by SQLite (durability across restarts)
- No notes tool in v1
- No shell tool in v1
- Do not use anything from vox or vox-companion
