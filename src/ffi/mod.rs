use std::ffi::{CStr, CString};
use tokio::sync::mpsc::UnboundedSender;

#[allow(non_upper_case_globals)]
#[allow(non_camel_case_types)]
#[allow(non_snake_case)]
#[allow(dead_code)]
#[allow(clippy::all)]
mod bindings {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

use bindings::*;

// ── Engine ────────────────────────────────────────────────────────────────────

pub struct Engine {
    pub(crate) raw: *mut LiteRtLmEngine,
    settings: *mut LiteRtLmEngineSettings,
}

// The C library documents that the engine is safe to share across threads.
unsafe impl Send for Engine {}
unsafe impl Sync for Engine {}

impl Engine {
    pub fn new(model_path: &str) -> anyhow::Result<Self> {
        let path = CString::new(model_path)?;
        let backend = CString::new("cpu")?;

        unsafe {
            let settings = litert_lm_engine_settings_create(
                path.as_ptr(),
                backend.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
            );
            anyhow::ensure!(
                !settings.is_null(),
                "litert_lm_engine_settings_create failed"
            );

            let engine = litert_lm_engine_create(settings);
            if engine.is_null() {
                litert_lm_engine_settings_delete(settings);
                anyhow::bail!("litert_lm_engine_create failed");
            }

            Ok(Engine {
                raw: engine,
                settings,
            })
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        unsafe {
            litert_lm_engine_delete(self.raw);
            litert_lm_engine_settings_delete(self.settings);
        }
    }
}

// ── Conversation ──────────────────────────────────────────────────────────────

pub struct Conversation {
    raw: *mut LiteRtLmConversation,
}

// Sessions/conversations can be moved to another thread but not shared.
unsafe impl Send for Conversation {}

impl Conversation {
    /// Create a new conversation.
    ///
    /// - `system_message_json`: `{"type":"text","text":"..."}` or `None`
    /// - `tools_json`:  OpenAI-format tool array JSON or `None`
    /// - `messages_json`: prior turn array JSON or `None`
    /// - `constrained_decoding`: set `true` when tools are present
    pub fn new(
        engine: &Engine,
        system_message_json: Option<&str>,
        tools_json: Option<&str>,
        messages_json: Option<&str>,
        constrained_decoding: bool,
    ) -> anyhow::Result<Self> {
        let sys_c = system_message_json.map(CString::new).transpose()?;
        let tools_c = tools_json.map(CString::new).transpose()?;
        let msgs_c = messages_json.map(CString::new).transpose()?;

        let sys_ptr = sys_c.as_deref().map_or(std::ptr::null(), |s| s.as_ptr());
        let tools_ptr = tools_c.as_deref().map_or(std::ptr::null(), |s| s.as_ptr());
        let msgs_ptr = msgs_c.as_deref().map_or(std::ptr::null(), |s| s.as_ptr());

        unsafe {
            let config = litert_lm_conversation_config_create(
                engine.raw,
                std::ptr::null_mut(), // use default session config
                sys_ptr,
                tools_ptr,
                msgs_ptr,
                constrained_decoding,
            );
            anyhow::ensure!(
                !config.is_null(),
                "litert_lm_conversation_config_create failed"
            );

            let conv = litert_lm_conversation_create(engine.raw, config);
            litert_lm_conversation_config_delete(config);

            anyhow::ensure!(!conv.is_null(), "litert_lm_conversation_create failed");
            Ok(Conversation { raw: conv })
        }
    }

    /// Blocking: send a message and return the full JSON response string.
    ///
    /// `message_json` format: `{"role":"user","content":[{"type":"text","text":"..."}]}`
    pub fn send_message(&self, message_json: &str) -> anyhow::Result<String> {
        let msg = CString::new(message_json)?;
        unsafe {
            let resp =
                litert_lm_conversation_send_message(self.raw, msg.as_ptr(), std::ptr::null());
            anyhow::ensure!(
                !resp.is_null(),
                "litert_lm_conversation_send_message failed"
            );
            let s = CStr::from_ptr(litert_lm_json_response_get_string(resp))
                .to_string_lossy()
                .into_owned();
            litert_lm_json_response_delete(resp);
            Ok(s)
        }
    }

    /// Streaming: send a message and forward chunks to `tx`.
    ///
    /// Chunks are raw text. The final `is_final=true` chunk signals completion.
    /// Returns once the stream has started (callback fires from a background thread).
    pub fn send_message_stream(
        &self,
        message_json: &str,
        tx: UnboundedSender<StreamChunk>,
    ) -> anyhow::Result<()> {
        let msg = CString::new(message_json)?;
        let tx_box = Box::new(tx);
        let userdata = Box::into_raw(tx_box) as *mut std::ffi::c_void;

        let rc = unsafe {
            litert_lm_conversation_send_message_stream(
                self.raw,
                msg.as_ptr(),
                std::ptr::null(),
                Some(stream_callback),
                userdata,
            )
        };
        anyhow::ensure!(
            rc == 0,
            "litert_lm_conversation_send_message_stream failed: {rc}"
        );
        Ok(())
    }

    pub fn cancel(&self) {
        unsafe { litert_lm_conversation_cancel_process(self.raw) }
    }
}

impl Drop for Conversation {
    fn drop(&mut self) {
        unsafe { litert_lm_conversation_delete(self.raw) }
    }
}

// ── Streaming callback ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct StreamChunk {
    pub text: String,
    pub is_final: bool,
    pub error: Option<String>,
}

unsafe extern "C" fn stream_callback(
    userdata: *mut std::ffi::c_void,
    chunk: *const std::ffi::c_char,
    is_final: bool,
    error_msg: *const std::ffi::c_char,
) {
    // Safety: we box/unbox the sender; it's only dropped on is_final.
    let tx = &*(userdata as *const UnboundedSender<StreamChunk>);

    let text = if chunk.is_null() {
        String::new()
    } else {
        CStr::from_ptr(chunk).to_string_lossy().into_owned()
    };

    let error = if error_msg.is_null() {
        None
    } else {
        Some(CStr::from_ptr(error_msg).to_string_lossy().into_owned())
    };

    let _ = tx.send(StreamChunk {
        text,
        is_final,
        error,
    });

    if is_final {
        // Reclaim the box so the sender is dropped.
        drop(Box::from_raw(userdata as *mut UnboundedSender<StreamChunk>));
    }
}
