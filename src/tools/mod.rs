use std::collections::HashMap;
use serde::{Deserialize, Serialize};

pub mod executor;
pub mod registry;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// JSON Schema object for the tool's parameters.
    pub parameters: serde_json::Value,
    pub handler: ToolHandler,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ToolHandler {
    Http {
        method: String,
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
        body: Option<String>,
    },
    Mqtt {
        broker: String,
        command_topic: String,
        payload: String,
        response_topic: Option<String>,
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u64,
    },
}

fn default_timeout_ms() -> u64 {
    3000
}

impl ToolDefinition {
    /// Serialize this tool as an OpenAI-format function declaration for the LiteRT-LM API.
    pub fn to_function_declaration(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters,
            }
        })
    }
}

/// Render a template string with args substituted in.
///
/// If the template contains Jinja syntax (`{%` or `{{`), it is rendered with
/// minijinja. Otherwise, simple `{param_name}` placeholder substitution is
/// used so that plain templates like `felix/homekit/{room}/set/On` continue
/// to work without escaping.
pub fn substitute(template: &str, args: &serde_json::Value) -> String {
    if template.contains("{%") || template.contains("{{") {
        render_jinja(template, args)
    } else {
        render_simple(template, args)
    }
}

fn render_simple(template: &str, args: &serde_json::Value) -> String {
    let mut result = template.to_owned();
    if let Some(obj) = args.as_object() {
        for (key, val) in obj {
            let placeholder = format!("{{{key}}}");
            let replacement = match val {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            result = result.replace(&placeholder, &replacement);
        }
    }
    result
}

fn render_jinja(template: &str, args: &serde_json::Value) -> String {
    let mut env = minijinja::Environment::new();
    env.add_template("t", template).unwrap_or(());
    let tmpl = match env.get_template("t") {
        Ok(t) => t,
        Err(_) => return template.to_owned(),
    };
    let ctx = minijinja::Value::from_serialize(args);
    tmpl.render(ctx).unwrap_or_else(|_| template.to_owned())
}
