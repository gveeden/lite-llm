use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

pub async fn init(path: &str) -> anyhow::Result<SqlitePool> {
    let expanded = shellexpand::tilde(path).into_owned();

    // Ensure parent directory exists.
    if let Some(parent) = std::path::Path::new(&expanded).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let url = format!("sqlite://{}?mode=rwc", expanded);
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await?;

    sqlx::migrate!("src/migrations").run(&pool).await?;

    Ok(pool)
}
