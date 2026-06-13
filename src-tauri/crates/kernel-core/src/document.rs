//! Document — the shared currency of RAG (retriever/indexer/embedding).
//!
//! Mirrors eino's `schema.Document`: content + an open metadata map. Well-known
//! metadata keys are accessed via free functions so producers/consumers agree
//! on names without coupling to a struct layout.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A retrieved or to-be-indexed piece of text with provenance metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub content: String,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Document {
    pub fn new(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            content: content.into(),
            metadata: HashMap::new(),
        }
    }

    /// Relevance score in [0.0, 1.0]. Populated by retrievers; absent ⇒ None.
    pub fn score(&self) -> Option<f64> {
        self.metadata.get("score").and_then(|v| v.as_f64())
    }

    pub fn with_score(mut self, score: f64) -> Self {
        self.metadata.insert("score".into(), score.into());
        self
    }

    /// Human-readable source label (file path, URL, session id, …).
    pub fn source(&self) -> Option<&str> {
        self.metadata.get("source").and_then(|v| v.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_score_roundtrips_through_metadata() {
        let d = Document::new("k1", "rust ownership rules").with_score(0.87);
        assert_eq!(d.score(), Some(0.87));
    }

    #[test]
    fn document_without_score_returns_none() {
        let d = Document::new("k1", "plain");
        assert_eq!(d.score(), None);
    }
}
