use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    pub model: Option<ModelConfig>,
    #[serde(default)]
    pub db: DbConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 8080,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct ModelConfig {
    pub path: String,
    #[serde(default = "default_context_size")]
    pub context_size: u32,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default = "default_top_p")]
    pub top_p: f32,
    #[serde(default = "default_top_k")]
    pub top_k: i32,
    #[serde(default = "default_gpu_layers")]
    pub gpu_layers: u32,
    /// KV cache quantization (e.g., "f16", "q8_0", "q4_0").
    #[serde(default)]
    pub kv_quant: Option<String>,
}

impl ModelConfig {
    pub fn from_path(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            context_size: default_context_size(),
            temperature: default_temperature(),
            top_p: default_top_p(),
            top_k: default_top_k(),
            gpu_layers: default_gpu_layers(),
            kv_quant: None,
        }
    }
}

fn default_context_size() -> u32 {
    8192
}

fn default_temperature() -> f32 {
    0.8
}

fn default_top_p() -> f32 {
    0.95
}

fn default_top_k() -> i32 {
    40
}

fn default_gpu_layers() -> u32 {
    99
}

#[derive(Debug, Deserialize)]
pub struct DbConfig {
    pub path: String,
}

impl Default for DbConfig {
    fn default() -> Self {
        Self {
            path: "~/.local/share/lite-llm/lite-llm.db".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct MemoryConfig {
    #[serde(default)]
    pub enabled: bool,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self { enabled: false }
    }
}

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&text)?)
    }

    pub fn load_or_default(path: &str) -> Self {
        Self::load(path).unwrap_or_default()
    }
}
