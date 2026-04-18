use axum::{extract::State, response::sse::{Event, Sse}};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::StreamExt;
use tokio::io::AsyncWriteExt;

use crate::api::router::AppState;

const DEFAULT_REPO_ID: &str = "litert-community/gemma-4-E2B-it-litert-lm";
const DEFAULT_MODELS_DIR: &str = "~/.local/share/lite-llm/models";

#[derive(Deserialize, Default)]
pub struct DownloadRequest {
    pub repo_id: Option<String>,
    pub filename: Option<String>,
    /// ID to register the model under after download. Defaults to filename stem.
    pub model_id: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
enum DownloadEvent {
    Resolving { message: String },
    Downloading { filename: String, downloaded: u64, total: Option<u64> },
    Loading { message: String },
    Complete { model_id: String, path: String },
    Error { message: String },
}

pub async fn download_model(
    State(state): State<AppState>,
    // Body is optional — omitting it uses all defaults.
    body: Option<axum::extract::Json<DownloadRequest>>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let req = body.map(|b| b.0).unwrap_or_default();

    let (tx, rx) = mpsc::unbounded_channel::<DownloadEvent>();

    tokio::spawn(async move {
        if let Err(e) = run_download(req, state, &tx).await {
            let _ = tx.send(DownloadEvent::Error { message: e.to_string() });
        }
    });

    let stream = UnboundedReceiverStream::new(rx).map(|event| {
        Ok(Event::default().data(serde_json::to_string(&event).unwrap_or_default()))
    });

    Sse::new(stream)
}

async fn run_download(
    req: DownloadRequest,
    state: AppState,
    tx: &mpsc::UnboundedSender<DownloadEvent>,
) -> anyhow::Result<()> {
    let repo_id = req.repo_id.unwrap_or_else(|| DEFAULT_REPO_ID.to_string());

    // ── Resolve filename ──────────────────────────────────────────────────────
    let filename = match req.filename {
        Some(f) => f,
        None => {
            let _ = tx.send(DownloadEvent::Resolving {
                message: format!("Listing files in {repo_id}..."),
            });

            let api = build_hf_api()?;
            let repo = api.model(repo_id.clone());
            let info = repo.info().await
                .map_err(|e| anyhow::anyhow!("Failed to list repo files: {e}"))?;

            info.siblings
                .into_iter()
                .map(|s| s.rfilename)
                .find(|f| f.ends_with(".litertlm"))
                .ok_or_else(|| anyhow::anyhow!("No .litertlm file found in {repo_id}"))?
        }
    };

    let model_id = req.model_id.unwrap_or_else(|| {
        std::path::Path::new(&filename)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });

    // ── Prepare destination path ──────────────────────────────────────────────
    let models_dir = shellexpand::tilde(DEFAULT_MODELS_DIR).into_owned();
    tokio::fs::create_dir_all(&models_dir).await?;
    let dest = std::path::PathBuf::from(&models_dir).join(&filename);

    if dest.exists() {
        let _ = tx.send(DownloadEvent::Resolving {
            message: format!("{filename} already downloaded, loading..."),
        });
        load_and_report(state, model_id, dest, tx).await?;
        return Ok(());
    }

    // ── Build download URL ────────────────────────────────────────────────────
    let url = format!("https://huggingface.co/{repo_id}/resolve/main/{filename}");

    let _ = tx.send(DownloadEvent::Resolving {
        message: format!("Downloading {filename} from {repo_id}..."),
    });

    // ── Stream download ───────────────────────────────────────────────────────
    let mut req_builder = state.http.get(&url);
    if let Ok(token) = std::env::var("HF_TOKEN") {
        req_builder = req_builder.header("Authorization", format!("Bearer {token}"));
    }

    let response = req_builder.send().await?;

    if !response.status().is_success() {
        anyhow::bail!("HTTP {}: {url}", response.status());
    }

    let total = response.content_length();
    let mut downloaded: u64 = 0;

    // Write to a temp file then rename atomically.
    let tmp = dest.with_extension("litertlm.part");
    let mut file = tokio::fs::File::create(&tmp).await?;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = tokio_stream::StreamExt::next(&mut stream).await {
        let chunk = chunk.map_err(anyhow::Error::from)?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;

        // Emit progress ~every 5 MB to avoid flooding the SSE stream.
        if downloaded % (5 * 1024 * 1024) < chunk.len() as u64 {
            let _ = tx.send(DownloadEvent::Downloading {
                filename: filename.clone(),
                downloaded,
                total,
            });
        }
    }

    file.flush().await?;
    drop(file);
    tokio::fs::rename(&tmp, &dest).await?;

    // Final progress event showing 100%.
    let _ = tx.send(DownloadEvent::Downloading {
        filename: filename.clone(),
        downloaded,
        total: Some(downloaded),
    });

    load_and_report(state, model_id, dest, tx).await
}

async fn load_and_report(
    state: AppState,
    model_id: String,
    path: std::path::PathBuf,
    tx: &mpsc::UnboundedSender<DownloadEvent>,
) -> anyhow::Result<()> {
    let _ = tx.send(DownloadEvent::Loading {
        message: format!("Loading {model_id}..."),
    });

    let path_str = path.to_string_lossy().into_owned();
    let cfg = crate::config::ModelConfig::from_path(&path_str);
    state.models.load(model_id.clone(), &cfg)?;

    let _ = tx.send(DownloadEvent::Complete {
        model_id,
        path: path_str,
    });

    Ok(())
}

fn build_hf_api() -> anyhow::Result<hf_hub::api::tokio::Api> {
    let mut builder = hf_hub::api::tokio::ApiBuilder::new();
    if let Ok(token) = std::env::var("HF_TOKEN") {
        builder = builder.with_token(Some(token));
    }
    Ok(builder.build()?)
}
