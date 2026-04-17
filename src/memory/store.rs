use sqlx::SqlitePool;

pub struct MemoryStore {
    db: SqlitePool,
}

impl MemoryStore {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }

    /// Insert a memory, skipping if a duplicate already exists.
    /// Returns `"stored"` or `"already known"`.
    pub async fn insert(&self, content: &str) -> anyhow::Result<&'static str> {
        // Deduplication: if FTS5 finds any match for this content, skip.
        let hits = self.search(content, 1).await?;
        if !hits.is_empty() {
            return Ok("already known");
        }

        let now = chrono::Utc::now().timestamp();
        sqlx::query("INSERT INTO memories (content, created_at) VALUES (?, ?)")
            .bind(content)
            .bind(now)
            .execute(&self.db)
            .await?;

        Ok("stored")
    }

    /// Search memories using FTS5 BM25 ranking.
    pub async fn search(&self, query: &str, limit: i64) -> anyhow::Result<Vec<String>> {
        // Sanitise the query for FTS5: strip special characters that would
        // cause a parse error (quotes, parentheses, operators, etc.).
        let sanitised = sanitise_fts_query(query);
        if sanitised.is_empty() {
            return Ok(vec![]);
        }

        let rows = sqlx::query_scalar::<_, String>(
            "SELECT content FROM memories_fts WHERE memories_fts MATCH ?
             ORDER BY bm25(memories_fts) LIMIT ?",
        )
        .bind(sanitised)
        .bind(limit)
        .fetch_all(&self.db)
        .await?;

        Ok(rows)
    }

    /// Return all memories, newest first.
    pub async fn list(&self) -> anyhow::Result<Vec<String>> {
        let rows = sqlx::query_scalar::<_, String>(
            "SELECT content FROM memories ORDER BY created_at DESC",
        )
        .fetch_all(&self.db)
        .await?;
        Ok(rows)
    }
}

/// Strip characters that are syntactically meaningful to FTS5 so that an
/// arbitrary user/model string can be passed as a query without errors.
fn sanitise_fts_query(input: &str) -> String {
    // Keep alphanumeric characters and spaces; everything else becomes a space.
    let cleaned: String = input
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' { c } else { ' ' })
        .collect();

    // Collapse runs of spaces and trim.
    cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
