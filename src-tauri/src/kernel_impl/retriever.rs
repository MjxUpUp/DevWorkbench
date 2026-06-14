//! `Retriever` implementation over the existing FTS5 knowledge store.
//!
//! Bridges kernel-core's async `Retriever` trait to the synchronous
//! `knowledge::store` functions by running DB access on a blocking thread.

use async_trait::async_trait;
use kernel_core::{Document, Error, RetrieveOptions, Retriever};
use serde_json::Value;

use crate::activity::hash_project_path;
use crate::db::DbState;
use crate::knowledge::store;

/// A Retriever backed by DevWorkbench's project-scoped FTS5 knowledge base.
pub struct KnowledgeRetriever {
    db: DbState,
    /// The project to scope queries to (None ⇒ cross-project search).
    project_path: Option<String>,
}

impl KnowledgeRetriever {
    pub fn new(db: DbState, project_path: Option<String>) -> Self {
        Self { db, project_path }
    }
}

#[async_trait]
impl Retriever for KnowledgeRetriever {
    async fn retrieve(
        &self,
        query: &str,
        opts: &RetrieveOptions,
    ) -> Result<Vec<Document>, Error> {
        let limit = opts.top_k.unwrap_or(5).min(20);
        let db = self.db.clone();
        let project_path = self.project_path.clone();
        let q = query.to_string();
        let scope = opts.scope.clone().or(project_path);
        let scope_for_task = scope.clone();

        // DB access is synchronous + may contend on the connection Mutex; push
        // it to the blocking pool so we don't stall the async runtime.
        let entries = tokio::task::spawn_blocking(move || -> Result<Vec<_>, Error> {
            let conn = db.get().map_err(|e| Error::Retrieval(format!("db lock: {e}")))?;
            match scope_for_task {
                Some(path) => {
                    let hash = hash_project_path(&path);
                    store::search_entries_for_project(&conn, &hash, &q, 0.3, limit)
                        .map_err(|e| Error::Retrieval(e.to_string()))
                }
                None => store::search_entries(&conn, &q, limit)
                    .map_err(|e| Error::Retrieval(e.to_string())),
            }
        })
        .await
        .map_err(|e| Error::Retrieval(format!("join: {e}")))??;

        Ok(entries
            .into_iter()
            .map(|e| {
                let mut d = Document::new(e.id, e.content);
                d.metadata.insert("title".into(), e.title.into());
                d.metadata.insert("category".into(), e.category.into());
                d.metadata.insert("confidence".into(), e.confidence.into());
                d.metadata.insert(
                    "source_agent".into(),
                    serde_json::to_value(&e.source_agent).unwrap_or(Value::Null),
                );
                if let Some(p) = &scope {
                    d.metadata.insert("source".into(), p.clone().into());
                }
                d
            })
            .collect())
    }
}
