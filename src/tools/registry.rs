use sqlx::SqlitePool;
use crate::tools::ToolDefinition;

pub struct ToolRegistry {
    tools: std::sync::RwLock<Vec<ToolDefinition>>,
    db: SqlitePool,
}

impl ToolRegistry {
    pub async fn load(db: SqlitePool) -> anyhow::Result<Self> {
        let rows = sqlx::query(
            "SELECT name, description, parameters, handler FROM tools WHERE enabled = 1",
        )
        .fetch_all(&db)
        .await?;

        let tools = rows
            .into_iter()
            .filter_map(|row| {
                use sqlx::Row;
                let name: String = row.try_get("name").ok()?;
                let description: String = row.try_get("description").ok()?;
                let parameters_str: String = row.try_get("parameters").ok()?;
                let handler_str: String = row.try_get("handler").ok()?;
                let parameters: serde_json::Value = serde_json::from_str(&parameters_str).ok()?;
                let handler = serde_json::from_str(&handler_str).ok()?;
                Some(ToolDefinition {
                    name,
                    description,
                    parameters,
                    handler,
                    response: Default::default(),
                    enabled: true,
                })
            })
            .collect();

        Ok(Self {
            tools: std::sync::RwLock::new(tools),
            db,
        })
    }

    pub fn all(&self) -> Vec<ToolDefinition> {
        let mut tools = vec![crate::tools::datetime_tool()];
        tools.extend(self.tools.read().unwrap().clone());
        tools
    }

    pub fn get(&self, name: &str) -> Option<ToolDefinition> {
        if name == "get_datetime" {
            return Some(crate::tools::datetime_tool());
        }
        self.tools
            .read()
            .unwrap()
            .iter()
            .find(|t| t.name == name)
            .cloned()
    }

    pub fn by_names(&self, names: &[String]) -> Vec<ToolDefinition> {
        let mut tools: Vec<ToolDefinition> = Vec::new();
        if names.contains(&"get_datetime".to_string()) {
            tools.push(crate::tools::datetime_tool());
        }
        tools.extend(
            self.tools
                .read()
                .unwrap()
                .iter()
                .filter(|t| names.contains(&t.name))
                .cloned(),
        );
        tools
    }

    pub async fn insert(&self, tool: ToolDefinition) -> anyhow::Result<()> {
        let parameters = tool.parameters.to_string();
        let handler = serde_json::to_string(&tool.handler)?;
        let now = chrono::Utc::now().timestamp();

        sqlx::query(
            "INSERT INTO tools (name, description, parameters, handler, enabled, created_at)
             VALUES (?, ?, ?, ?, 1, ?)
             ON CONFLICT(name) DO UPDATE SET
               description = excluded.description,
               parameters  = excluded.parameters,
               handler     = excluded.handler,
               enabled     = 1",
        )
        .bind(&tool.name)
        .bind(&tool.description)
        .bind(&parameters)
        .bind(&handler)
        .bind(now)
        .execute(&self.db)
        .await?;

        let mut tools = self.tools.write().unwrap();
        tools.retain(|t| t.name != tool.name);
        tools.push(tool);
        Ok(())
    }

    pub async fn delete(&self, name: &str) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM tools WHERE name = ?")
            .bind(name)
            .execute(&self.db)
            .await?;

        if result.rows_affected() > 0 {
            self.tools.write().unwrap().retain(|t| t.name != name);
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
