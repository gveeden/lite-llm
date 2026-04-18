use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatTemplate, LlamaModel};
use llama_cpp_2::openai::OpenAIChatTemplateParams;
use llama_cpp_2::sampling::LlamaSampler;
use once_cell::sync::OnceCell;
use serde_json::Value;

use super::{BackendResponse, ConversationHandle, IncomingMessage, Message, ModelBackend};
use crate::tools::ToolDefinition;

// ── Global backend (llama_backend_init can only be called once per process) ───

static LLAMA_BACKEND: OnceCell<LlamaBackend> = OnceCell::new();

fn get_or_init_backend() -> anyhow::Result<&'static LlamaBackend> {
    LLAMA_BACKEND
        .get_or_try_init(LlamaBackend::init)
        .map_err(|e| anyhow::anyhow!("Failed to initialise llama backend: {e}"))
}

// ── Shared context pool ───────────────────────────────────────────────────────
//
// LlamaContext wraps a raw C pointer that is not auto-Send/Sync.  We guarantee
// exclusive access via Mutex, so both impls are safe.
//
// `cached_tokens` tracks which tokens are currently valid in the KV cache so
// that successive requests sharing a common prompt prefix can skip re-evaluating
// those tokens and only decode the delta.
struct ContextState {
    ctx: llama_cpp_2::context::LlamaContext<'static>,
    cached_tokens: Vec<llama_cpp_2::token::LlamaToken>,
}

struct ContextPool(Mutex<ContextState>);
unsafe impl Send for ContextPool {}
unsafe impl Sync for ContextPool {}

// ── Backend ───────────────────────────────────────────────────────────────────

pub struct LlamaModelBackend {
    model: &'static LlamaModel,
    // The inference context is created once at load time and shared across
    // all conversations via a Mutex.  This matches how llama-server pre-allocates
    // slots: context creation (KV-cache allocation) is expensive and should not
    // happen per-request.
    pool: Arc<ContextPool>,
    config: crate::config::ModelConfig,
}

impl LlamaModelBackend {
    pub fn load(path: &str, cfg: &crate::config::ModelConfig) -> anyhow::Result<Self> {
        let backend = get_or_init_backend()?;

        // Number of transformer layers to offload to GPU/Vulkan.
        let n_gpu = cfg.gpu_layers;

        let model_params = LlamaModelParams::default().with_n_gpu_layers(n_gpu);
        let model = LlamaModel::load_from_file(backend, path, &model_params)
            .map_err(|e| anyhow::anyhow!("Failed to load llama model '{path}': {e}"))?;

        // Leak the model so we can hold &'static refs (needed for LlamaContext<'static>).
        // The model lives for the process lifetime — the leak is intentional.
        let model: &'static LlamaModel = Box::leak(Box::new(model));

        // Create the inference context once here.
        // n_batch = n_ctx so the full prompt always fits in a single decode call.
        // 1 = LLAMA_FLASH_ATTN_TYPE_ENABLED  (matches --flash-attn on)
        let n_ctx = cfg.context_size;
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(n_ctx))
            .with_n_batch(n_ctx)
            .with_flash_attention_policy(1);

        let context = model
            .new_context(backend, ctx_params)
            .map_err(|e| anyhow::anyhow!("new_context: {e}"))?;

        tracing::info!("Loaded llama model: {path} (n_gpu_layers={n_gpu}, n_ctx={n_ctx})");
        Ok(LlamaModelBackend {
            model,
            pool: Arc::new(ContextPool(Mutex::new(ContextState {
                ctx: context,
                cached_tokens: Vec::new(),
            }))),
            config: cfg.clone(),
        })
    }
}

impl ModelBackend for LlamaModelBackend {
    fn new_conversation(
        &self,
        system: Option<&str>,
        tools: &[ToolDefinition],
        history: &[Message],
    ) -> anyhow::Result<Box<dyn ConversationHandle>> {
        let tools_json = if tools.is_empty() {
            None
        } else {
            let arr: Vec<Value> = tools.iter().map(|t| t.to_function_declaration()).collect();
            Some(serde_json::to_string(&arr)?)
        };

        // Seed conversation with system prompt then chat history.
        let mut messages: Vec<Value> = Vec::new();
        if let Some(sys) = system {
            messages.push(serde_json::json!({"role": "system", "content": sys}));
        }
        for msg in history {
            messages.push(serde_json::json!({"role": msg.role, "content": msg.content}));
        }

        Ok(Box::new(LlamaConversation {
            model: self.model,
            pool: Arc::clone(&self.pool),
            messages,
            tools_json,
            config: self.config.clone(),
        }))
    }
}

// ── Conversation handle ───────────────────────────────────────────────────────

// ContextPool's Mutex guarantees exclusive access; safe to send across threads.
unsafe impl Send for LlamaConversation {}

struct LlamaConversation {
    model: &'static LlamaModel,
    /// Shared inference context (one per model, serialised via Mutex).
    pool: Arc<ContextPool>,
    /// Accumulated history as OAI-format message objects.
    messages: Vec<Value>,
    /// OpenAI-format tools JSON array, if tools are active.
    tools_json: Option<String>,
    config: crate::config::ModelConfig,
}

impl ConversationHandle for LlamaConversation {
    fn send(
        &mut self,
        msg: IncomingMessage,
        delta_tx: Option<&tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> anyhow::Result<BackendResponse> {
        let t_start = std::time::Instant::now();

        // 1. Append incoming message to history.
        match msg {
            IncomingMessage::User(text) => {
                self.messages
                    .push(serde_json::json!({"role": "user", "content": text}));
            }
            IncomingMessage::ToolResult { tool_name, result } => {
                // Include the tool name so the chat template can associate this
                // result with the preceding assistant tool_call turn.
                self.messages.push(serde_json::json!({
                    "role": "tool",
                    "name": tool_name,
                    "content": result,
                }));
            }
        }
        let t_after_msg = t_start.elapsed();

        // 2. Serialise accumulated history for the OAI-compat template API.
        let messages_json = serde_json::to_string(&self.messages)?;
        let t_after_serialize = t_start.elapsed();

        // 3. Format prompt via the model's embedded chat template.
        //    enable_thinking=false suppresses Gemma 4's chain-of-thought block.
        //    strip_thinking_block() below is kept as a safety net for models that
        //    still emit empty thought tags even when thinking is disabled.
        let template: LlamaChatTemplate = self
            .model
            .chat_template(None)
            .map_err(|e| anyhow::anyhow!("chat_template: {e}"))?;
        let t_after_get_template = t_start.elapsed();

        let params = OpenAIChatTemplateParams {
            messages_json: &messages_json,
            tools_json: self.tools_json.as_deref(),
            tool_choice: None,
            json_schema: None,
            grammar: None,
            reasoning_format: Some("none"),
            chat_template_kwargs: None,
            add_generation_prompt: true,
            use_jinja: true,
            parallel_tool_calls: false,
            enable_thinking: false,
            add_bos: false,
            add_eos: false,
            parse_tool_calls: false,
        };

        let template_result = self
            .model
            .apply_chat_template_oaicompat(&template, &params)
            .map_err(|e| anyhow::anyhow!("apply_chat_template: {e}"))?;
        let t_after_apply_template = t_start.elapsed();

        let prompt = &template_result.prompt;
        tracing::debug!(chars = prompt.len(), "→ llama prompt");

        // Build stop sequences: use what the template provides plus known Gemma 4 markers.
        // <eos> and <end_of_turn> are end-of-turn tokens that is_eog_token may not catch.
        // <tool_call|> ends a tool call — stop immediately so we can parse it.
        let mut stops: Vec<String> = template_result.additional_stops.clone();
        for s in &["<eos>", "<end_of_turn>", "<tool_call|>"] {
            if !stops.contains(&(*s).to_string()) {
                stops.push((*s).to_string());
            }
        }

        // 4. Tokenise.
        let tokens = self
            .model
            .str_to_token(prompt, AddBos::Always)
            .map_err(|e| anyhow::anyhow!("tokenise: {e}"))?;
        let t_after_tokenize = t_start.elapsed();

        let n_prompt = tokens.len();
        anyhow::ensure!(n_prompt > 0, "Empty prompt after tokenisation");

        // 5. Evaluate only the tokens that aren't already in the KV cache.
        //
        //    We track which tokens were last written into the context and find
        //    the longest common prefix with the current prompt.  Only the
        //    suffix beyond that prefix needs decoding.  This matches the
        //    llama-server "context checkpoint" strategy and eliminates the
        //    dominant cost on warm requests where the system-prompt + tool
        //    definitions haven't changed.
        let mut state = self.pool.0.lock().unwrap();
        let t_after_lock = t_start.elapsed();

        let n_common = tokens
            .iter()
            .zip(state.cached_tokens.iter())
            .take_while(|(a, b)| a == b)
            .count();

        // Determine the starting position for the decode batch.
        //
        // When every prompt token is already cached (n_common == n_prompt) the
        // logit buffer still holds output from the PREVIOUS request's last
        // generated token, which is wrong.  We evict just the last prompt
        // position and re-decode it (1 token) so the sampler sees the correct
        // next-token distribution.
        //
        // Generated tokens from the previous turn are always stripped after
        // generation (see below), so the KV is clean from n_cached_end onwards.
        let decode_from = if n_common == n_prompt && n_prompt > 0 {
            let last = n_prompt - 1;
            state
                .ctx
                .clear_kv_cache_seq(Some(0), Some(last as u32), None)
                .map_err(|e| anyhow::anyhow!("clear_kv_cache_seq(last): {e}"))?;
            last
        } else {
            // Clear everything from n_common onwards (overwrites old tail +
            // any generated tokens left from the previous turn).
            if n_common == 0 {
                state.ctx.clear_kv_cache();
            } else {
                state
                    .ctx
                    .clear_kv_cache_seq(Some(0), Some(n_common as u32), None)
                    .map_err(|e| anyhow::anyhow!("clear_kv_cache_seq: {e}"))?;
            }
            n_common
        };
        let t_after_kv_clear = t_start.elapsed();

        let n_new_prompt = n_prompt - n_common; // tokens charged to this turn (for stats)
        let n_to_decode = n_prompt - decode_from; // tokens actually sent to the GPU
        let mut batch = LlamaBatch::new(n_to_decode, 1);
        for (i, &tok) in tokens[decode_from..].iter().enumerate() {
            let pos = (decode_from + i) as i32;
            let is_last = i == n_to_decode - 1;
            batch
                .add(tok, pos, &[0], is_last)
                .map_err(|e| anyhow::anyhow!("batch.add: {e}"))?;
        }
        let t_after_batch_build = t_start.elapsed();

        state
            .ctx
            .decode(&mut batch)
            .map_err(|e| anyhow::anyhow!("decode(prompt): {e}"))?;
        let t_after_prompt_decode = t_start.elapsed();

        // Record the prompt tokens as the new cache state.
        state.cached_tokens = tokens.clone();

        // 6. Autoregressive token sampling.
        let mut sampler = LlamaSampler::chain(
            [
                LlamaSampler::top_k(self.config.top_k),
                LlamaSampler::top_p(self.config.top_p, 1),
                LlamaSampler::temp(self.config.temperature),
            ],
            false,
        );
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut output = String::new();
        let mut n_new: usize = 0;
        let mut t_first_token: Option<std::time::Duration> = None;
        const MAX_NEW_TOKENS: usize = 2048;

        loop {
            // -1 means "sample from the last position's logits".
            let token = sampler.sample(&state.ctx, -1);
            sampler.accept(token);

            if self.model.is_eog_token(token) {
                break;
            }

            let piece = self
                .model
                .token_to_piece(token, &mut decoder, true, None)
                .map_err(|e| anyhow::anyhow!("token_to_piece: {e}"))?;
            output.push_str(&piece);
            n_new += 1;

            if t_first_token.is_none() {
                t_first_token = Some(t_start.elapsed());
            }

            // Check explicit stop sequences (catches <eos> text tokens, <end_of_turn>,
            // and <tool_call|> which ends a Gemma 4 tool call).
            if let Some(stop) = stops.iter().find(|s| output.ends_with(s.as_str())) {
                let trim_len = output.len() - stop.len();
                output.truncate(trim_len);
                break;
            }

            // Stream piece to caller — but only for text responses, not tool calls.
            // A tool call response begins with `<|tool_call>` or `<tool_call>`;
            // once the accumulated output starts with that marker we know we are
            // in a tool call and suppress all deltas for this turn.
            if let Some(tx) = delta_tx {
                let trimmed = output.trim_start();
                let is_tool_call =
                    trimmed.starts_with("<|tool_call>") || trimmed.starts_with("<tool_call>");
                if !is_tool_call {
                    let _ = tx.send(piece.clone());
                }
            }

            if n_new >= MAX_NEW_TOKENS {
                tracing::warn!("llama: hit MAX_NEW_TOKENS limit ({MAX_NEW_TOKENS})");
                break;
            }

            // Decode the newly sampled token.
            batch.clear();
            batch
                .add(token, (n_prompt + n_new - 1) as i32, &[0], true)
                .map_err(|e| anyhow::anyhow!("batch.add: {e}"))?;
            state
                .ctx
                .decode(&mut batch)
                .map_err(|e| anyhow::anyhow!("decode(token): {e}"))?;
        }

        // Strip generated tokens from the KV cache so subsequent send() calls
        // (tool-result turns or next-request prefix matching) always start from
        // a clean n_prompt boundary.  Generated positions don't survive across
        // turns because the chat template re-encodes them differently in history.
        if n_new > 0 {
            let _ = state
                .ctx
                .clear_kv_cache_seq(Some(0), Some(n_prompt as u32), None);
        }

        // ── Stats ──────────────────────────────────────────────────────────────
        let t_total = t_start.elapsed();
        let ttft = t_first_token.unwrap_or(t_total);
        let gen_secs = (t_total - ttft).as_secs_f64();
        let tps = if gen_secs > 0.0 {
            n_new as f64 / gen_secs
        } else {
            0.0
        };
        tracing::info!(
            n_prompt,
            n_common,
            n_new_prompt,
            n_new,
            // Phase timings (ms since t_start, cumulative)
            msg_ms        = t_after_msg.as_millis(),
            serialize_ms  = t_after_serialize.as_millis(),
            get_tmpl_ms   = t_after_get_template.as_millis(),
            apply_tmpl_ms = t_after_apply_template.as_millis(),
            tokenize_ms   = t_after_tokenize.as_millis(),
            lock_ms       = t_after_lock.as_millis(),
            kv_clear_ms   = t_after_kv_clear.as_millis(),
            batch_ms      = t_after_batch_build.as_millis(),
            prompt_dec_ms = t_after_prompt_decode.as_millis(),
            // Derived
            ttft_ms       = ttft.as_millis(),
            tps           = format_args!("{tps:.1}"),
            e2e_ms        = t_total.as_millis(),
            "llama stats"
        );
        tracing::debug!(output = %output, "← llama response");

        // 8. Parse the output first, then store the assistant turn in the format
        //    the chat template expects for subsequent turns.
        //
        //    For text: plain {"role":"assistant","content":"..."}.
        //    For tool calls: OAI {"role":"assistant","tool_calls":[...]} so the
        //    template can properly format the tool-call context on the next turn.
        //    Storing the raw <|tool_call>... text causes the template to treat it
        //    as plain content, leaving the model without usable context for the
        //    tool result turn (causing an immediate EOS on complex payloads).
        let response = parse_llama_output(&output);
        match &response {
            BackendResponse::ToolCall { name, arguments } => {
                self.messages.push(serde_json::json!({
                    "role": "assistant",
                    "tool_calls": [{
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": arguments,
                        }
                    }]
                }));
            }
            BackendResponse::Text(_) => {
                self.messages
                    .push(serde_json::json!({"role": "assistant", "content": output}));
            }
        }
        Ok(response)
    }
}

// ── Output parsing ────────────────────────────────────────────────────────────

fn parse_llama_output(text: &str) -> BackendResponse {
    // Strip Gemma 4 thinking block: <|channel>thought\n...\n<channel|>
    let text = strip_thinking_block(text);
    let trimmed = text.trim();

    // ── Gemma 4 native tool call via llama.cpp ────────────────────────────────
    // Format: <|tool_call>call:func_name{key1:<|"|>val<|"|>, key2:123}<tool_call|>
    // (The stop sequence check already removed <tool_call|>, so we only need the
    //  opening marker.)
    if let Some(start) = trimmed.find("<|tool_call>") {
        let after = &trimmed[start + "<|tool_call>".len()..];
        // End marker may or may not still be present
        let call_text = after
            .find("<tool_call|>")
            .map(|end| &after[..end])
            .unwrap_or(after);

        if let Some(resp) = parse_gemma_native_call(call_text) {
            return resp;
        }
    }

    // ── JSON tool_call format: <tool_call>JSON</tool_call> ────────────────────
    if let Some(start) = trimmed.find("<tool_call>") {
        let after = &trimmed[start + "<tool_call>".len()..];
        let json_str = after
            .find("</tool_call>")
            .map(|end| &after[..end])
            .unwrap_or(after);

        if let Ok(val) = serde_json::from_str::<Value>(json_str.trim()) {
            if let Some(name) = val.get("name").and_then(|n| n.as_str()) {
                let arguments = val
                    .get("arguments")
                    .or_else(|| val.get("args"))
                    .cloned()
                    .unwrap_or(Value::Object(Default::default()));
                return BackendResponse::ToolCall {
                    name: name.to_string(),
                    arguments,
                };
            }
        }
    }

    // ── OpenAI-format JSON ────────────────────────────────────────────────────
    if trimmed.starts_with('{') {
        if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
            if let Some(calls) = val.get("tool_calls").and_then(|v| v.as_array()) {
                if let Some(call) = calls.first() {
                    let name = call
                        .pointer("/function/name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let arguments = match call.pointer("/function/arguments") {
                        Some(v) if v.is_object() => v.clone(),
                        Some(v) if v.is_string() => {
                            serde_json::from_str(v.as_str().unwrap_or("{}"))
                                .unwrap_or(Value::Object(Default::default()))
                        }
                        _ => Value::Object(Default::default()),
                    };
                    return BackendResponse::ToolCall { name, arguments };
                }
            }
        }
    }

    BackendResponse::Text(text.to_string())
}

/// Strip Gemma 4's thinking block from the beginning of the output.
/// Format: `<|channel>thought\n...\n<channel|>`
fn strip_thinking_block(text: &str) -> &str {
    if let Some(start) = text.find("<|channel>") {
        if let Some(end) = text.find("<channel|>") {
            if end > start {
                return text[end + "<channel|>".len()..].trim_start();
            }
        }
    }
    text
}

/// Parse Gemma 4's native tool call syntax (as emitted by llama.cpp):
/// `call:func_name{key1:<|"|>val<|"|>, key2:123}`
fn parse_gemma_native_call(text: &str) -> Option<BackendResponse> {
    // Clean out <|"|> string-value wrappers.
    let clean = text
        .replace("<|\"|>", "")
        .replace("<|\"", "")
        .replace("\"|>", "");
    let clean = clean.trim();

    let after_call = clean.strip_prefix("call:")?;
    let brace = after_call.find('{')?;
    let name = after_call[..brace].trim().to_string();
    if name.is_empty() {
        return None;
    }

    let inner = after_call
        .get(brace + 1..)?
        .trim_end_matches(|c| c == '}' || char::is_whitespace(c));

    let mut obj = serde_json::Map::new();
    for pair in inner.split(',') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        if let Some(colon) = pair.find(':') {
            let key = pair[..colon].trim().to_string();
            let val = pair[colon + 1..].trim();
            let value = if let Ok(n) = val.parse::<i64>() {
                serde_json::Value::Number(n.into())
            } else if let Ok(f) = val.parse::<f64>() {
                serde_json::Value::Number(serde_json::Number::from_f64(f).unwrap_or(0.into()))
            } else if val == "true" {
                serde_json::Value::Bool(true)
            } else if val == "false" {
                serde_json::Value::Bool(false)
            } else {
                serde_json::Value::String(val.to_string())
            };
            obj.insert(key, value);
        }
    }

    Some(BackendResponse::ToolCall {
        name,
        arguments: Value::Object(obj),
    })
}
