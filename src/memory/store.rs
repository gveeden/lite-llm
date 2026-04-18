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

/// Build an FTS5 OR query from an arbitrary string.
///
/// Strips FTS5 syntax characters, discards tokens shorter than 3 characters
/// (prepositions, articles, etc. that don't add signal), then joins the
/// remaining tokens with OR so that any memory matching *any* word in the
/// user's message is returned.  Combined with the porter tokenizer this means
/// "Turn on all the lights" retrieves memories containing "light" or "room"
/// or "living", etc.
fn sanitise_fts_query(input: &str) -> String {
    let terms: Vec<String> = input
        .split_whitespace()
        .filter_map(|w| {
            let clean: String = w.chars().filter(|c| c.is_alphanumeric()).collect();
            if clean.len() >= 3 { Some(clean) } else { None }
        })
        .collect();

    terms.join(" OR ")
}
