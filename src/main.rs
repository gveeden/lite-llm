use std::sync::Arc;
use clap::Parser;
use tracing::info;

mod api;
mod config;
mod db;
mod engine;
mod ffi;
mod tools;

use api::router::{AppState, build};
use engine::model_manager::ModelManager;
use tools::registry::ToolRegistry;

#[derive(Parser)]
#[command(name = "lite-llm", about = "Local LLM server with tool calling")]
struct Cli {
    #[arg(short, long, default_value = "config.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,lite_llm=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let cfg = config::Config::load_or_default(&cli.config);

    // Database
    let db = db::init(&cfg.db.path).await?;
    info!("Database ready at {}", cfg.db.path);

    // Tool registry
    let tools = Arc::new(ToolRegistry::load(db.clone()).await?);
    info!("Loaded {} tool(s)", tools.all().len());

    // Model manager
    let models = Arc::new(ModelManager::new());
    if let Some(model_cfg) = &cfg.model {
        info!("Loading model from {}...", model_cfg.path);
        models.load("default".into(), &model_cfg.path)?;
        info!("Model ready");
    }

    // HTTP client for tool execution
    let http = Arc::new(reqwest::Client::new());

    let state = AppState { db, models, tools, http };
    let router = build(state);

    let addr = format!("{}:{}", cfg.server.host, cfg.server.port);
    info!("Listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, router).await?;

    Ok(())
}
