# lite-llm API Examples

## Models

### Download a model (recommended first step)

```bash
# All defaults — downloads gemma-4-E2B-it from litert-community
curl -s -N -X POST http://localhost:8080/models/download

# Specify repo only — auto-detects the .litertlm filename
curl -s -N -X POST http://localhost:8080/models/download \
  -H 'Content-Type: application/json' \
  -d '{"repo_id": "google/gemma-3n-E2B-it-litert-lm"}'

# Fully explicit
curl -s -N -X POST http://localhost:8080/models/download \
  -H 'Content-Type: application/json' \
  -d '{
    "repo_id": "litert-community/gemma-4-E2B-it-litert-lm",
    "filename": "gemma-4-E2B-it.litertlm",
    "model_id": "gemma-4"
  }'
```

For gated models, set `HF_TOKEN` before starting the server:
```bash
HF_TOKEN=hf_xxx cargo run
```

Download SSE stream:
```
data: {"status":"resolving","message":"Listing files in litert-community/gemma-4-E2B-it-litert-lm..."}
data: {"status":"downloading","filename":"gemma-4-E2B-it.litertlm","downloaded":5242880,"total":1900000000}
data: {"status":"loading","message":"Loading gemma-4-E2B-it..."}
data: {"status":"complete","model_id":"gemma-4-E2B-it","path":"/home/gerald/.local/share/lite-llm/models/gemma-4-E2B-it.litertlm"}
```

### Load a model from a local path

```bash
curl -s -X POST http://localhost:8080/models/load \
  -H 'Content-Type: application/json' \
  -d '{
    "model_id": "gemma",
    "model_path": "/path/to/gemma-4-E2B-it.litertlm"
  }'
```

### List loaded models

```bash
curl -s http://localhost:8080/models | jq
```

---

## Tools

### Register an HTTP tool

```bash
curl -s -X POST http://localhost:8080/tools \
    -H 'Content-Type: application/json' \
    -d '{
      "name": "get_weather",
      "description": "Get detailed weather: current, min/max temps, precipitation, wind, and feel",
      "parameters": {
        "type": "object",
        "properties": {
          "location": {"type": "string", "description": "City name"}
        },
        "required": ["location"]
      },
      "handler": {
        "type": "http",
        "method": "GET",
        "url": "https://wttr.in/{location}?format=%l:+%C+%t+(Feels:%20%f)+Min:%20%L+Max:%20%H+Prec:%20%p+Wind:%20%w",
        "headers": {
          "User-Agent": "curl"
        }
      }
    }'
```

### Register an MQTT smart home tool

Topic structure: `felix/homekit/{device name}/set/{CharacteristicName}`
Payload for the `On` characteristic: `true`, `false`, `1`, `0`, `on`, or `off`

The payload field supports Jinja2 templating (`{% if %}`, `{{ var }}`) as well as
plain `{param}` substitution.

```bash
# Using Jinja conditional — model passes "turn_on" or "turn_off", template maps to true/false
curl -s -X POST http://localhost:8080/tools \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "control_light",
    "description": "Turn a smart light on or off",
    "parameters": {
      "type": "object",
      "properties": {
        "room": {"type": "string"},
        "action": {"type": "string", "enum": ["turn_on", "turn_off"]}
      },
      "required": ["room", "action"]
    },
    "handler": {
      "type": "mqtt",
      "broker": "raspberrypi.local:1883",
      "command_topic": "felix/homekit/{room} light/set/On",
      "payload": "{% if action == '\''turn_on'\'' %}true{% else %}false{% endif %}",
      "timeout_ms": 3000
    }
  }' | jq

# Simpler alternative — model outputs "true"/"false" directly, no template needed
curl -s -X POST http://localhost:8080/tools \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "control_light",
    "description": "Turn a smart light on or off",
    "parameters": {
      "type": "object",
      "properties": {
        "room": {"type": "string"},
        "power_state": {"type": "string", "enum": ["true", "false"], "description": "true to turn on, false to turn off"}
      },
      "required": ["room", "power_state"]
    },
    "handler": {
      "type": "mqtt",
      "broker": "raspberrypi.local:1883",
      "command_topic": "felix/homekit/{room} light/set/On",
      "payload": "{power_state}",
      "timeout_ms": 3000
    }
  }' | jq
```

### List registered tools

```bash
curl -s http://localhost:8080/tools | jq
```

### Delete a tool

```bash
curl -s -X DELETE http://localhost:8080/tools/get_weather
```

---

## Chat

The `-N` flag disables output buffering so SSE chunks appear as they arrive.

### Simple chat (no tools)

```bash
curl -s -N -X POST http://localhost:8080/chat \
  -H 'Content-Type: application/json' \
  -d '{
    "messages": [{"role": "user", "content": "What day is it today?"}]
  }'
```

### Chat with a specific tool enabled

```bash
curl -s -N -X POST http://localhost:8080/chat \
  -H 'Content-Type: application/json' \
  -d '{
    "messages": [{"role": "user", "content": "What is the weather in Tokyo?"}],
    "tools": ["get_weather"]
  }'
```

Chat SSE stream:
```
data: {"delta":"The weather in Tokyo is","done":false}
data: {"delta":" sunny and 24°C.","done":false}
data: {"delta":"","done":true,"tool_executions":[{"tool":"get_weather","args":{"location":"Tokyo"},"result":"Tokyo: ☀️  +24°C"}]}
```

---

## Command

Runs the full NL → tool loop → answer pipeline server-side. Preferred over `/chat` for tool use.

### Command with all registered tools

```bash
curl -s -N -X POST http://localhost:8080/command \
  -H 'Content-Type: application/json' \
  -d '{
    "text": "Turn on the living room light"
  }'
```

### Command with specific tools

```bash
curl -s -N -X POST http://localhost:8080/command \
  -H 'Content-Type: application/json' \
  -d '{
    "text": "What is the weather in London?",
    "tools": ["get_weather"]
  }'
```

---

## Memory

Memory must be enabled in `config.toml` before these endpoints are available:

```toml
[memory]
enabled = true
```

Or pass `--no-memory` at startup to disable it regardless of config.

### Teach it about your home

```bash
curl -s -X POST http://localhost:8080/remember \
  -H 'Content-Type: application/json' \
  -d '{"text": "The living room has a smart light called '\''Main'\''"}'

curl -s -X POST http://localhost:8080/remember \
  -H 'Content-Type: application/json' \
  -d '{"text": "The bedroom has a smart light called '\''Bedside'\''"}'

curl -s -X POST http://localhost:8080/remember \
  -H 'Content-Type: application/json' \
  -d '{"text": "The kitchen has a smart light called '\''Kitchen Ceiling'\''"}'
```

Response:
```json
{"result": "stored"}
```

Submitting a duplicate or near-duplicate fact returns `"already known"` instead.

### Ask something that requires multiple tool calls

With the memories above, "turn off all the lights" will retrieve all three room facts and the model will call `control_light` once per room:

```bash
curl -s -N -X POST http://localhost:8080/command \
  -H 'Content-Type: application/json' \
  -d '{"text": "Turn off all the lights"}'
```

### List stored memories

```bash
curl -s http://localhost:8080/memories | jq
```

```json
[
  {"content": "The kitchen has a smart light called 'Kitchen Ceiling'"},
  {"content": "The bedroom has a smart light called 'Bedside'"},
  {"content": "The living room has a smart light called 'Main'"}
]
```

### The model stores memories autonomously

When memory is enabled, the model has access to the `remember` tool and will call it during a conversation when it decides something is worth storing. For example:

```bash
curl -s -N -X POST http://localhost:8080/command \
  -H 'Content-Type: application/json' \
  -d '{"text": "By the way, the hallway also has a light called Hallway Lamp"}'
```

The model will call `remember` with the fact, then confirm to you that it has been saved.

---

## Sessions

```bash
# List all sessions
curl -s http://localhost:8080/sessions | jq

# Get messages for a session
curl -s http://localhost:8080/sessions/sess_abc123/messages | jq
```
