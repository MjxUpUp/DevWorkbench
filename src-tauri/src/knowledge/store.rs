use crate::error::AppError;
use crate::models::{AgentType, KnowledgeEntry};
use rusqlite::params;

/// FTS sync operation (F5: single helper, all FTS write paths funnel through here).
/// `Insert` indexes `(rowid, title, content)`; `Delete` removes the FTS row by
/// the main table's `rowid`. Both look up the rowid by `entry_id` so callers
/// don't have to — only the rowid lookup is shared between the two ops; the
/// main-table rowid is the cross-table join key for `JOIN knowledge_entries`.
enum FtsOp {
    Insert { title: String, content: String },
    Delete,
}

/// F5: single FTS write helper. Resolves the main-table rowid by `entry_id`
/// and applies `op` to `knowledge_fts`. Use within a `tx`/`Connection` that
/// already holds the matching main-table mutation in the SAME transaction —
/// rolling back the main row must roll back the FTS row too.
///
/// Errors:
/// - `NotFound`: `entry_id` is not in `knowledge_entries` (caller error).
/// - `rusqlite::Error`: bubbles up; transaction rolls back.
fn sync_fts(
    conn: &rusqlite::Connection,
    entry_id: &str,
    op: FtsOp,
) -> Result<(), AppError> {
    let rowid: i64 = conn
        .query_row(
            "SELECT rowid FROM knowledge_entries WHERE id = ?1",
            params![entry_id],
            |row| row.get(0),
        )
        .map_err(|_| AppError::NotFound(format!("Knowledge entry {} 不存在", entry_id)))?;
    match op {
        FtsOp::Insert { title, content } => {
            conn.execute(
                "INSERT INTO knowledge_fts (rowid, title, content) VALUES (?1, ?2, ?3)",
                params![rowid, title, content],
            )?;
        }
        FtsOp::Delete => {
            conn.execute("DELETE FROM knowledge_fts WHERE rowid = ?1", params![rowid])?;
        }
    }
    Ok(())
}

/// Add a knowledge entry to the database. Skips if a near-duplicate already exists
/// (same project_hash and matching first 200 chars of content).
pub fn add_entry(conn: &rusqlite::Connection, entry: &KnowledgeEntry) -> Result<(), AppError> {
    // Dedup check: match on project_hash + first 200 CHARS of content —
    // char-based, NOT byte slicing. It must be chars().take(200) for two reasons:
    //   1. SQLite's SUBSTR(content, 1, 200) counts CHARACTERS, so a byte prefix
    //      would compare against a different string and dedup would silently miss.
    //   2. A byte index of 200 lands inside a 3-byte CJK char (e.g. '的' at bytes
    //      198..201) → panic "byte index 200 is not a char boundary". This
    //      panicked add_entry on EVERY react_kernel completion (CJK output), so
    //      knowledge entries were never inserted and every completion logged a
    //      [PANIC]. chars().take(200) never panics and matches SUBSTR exactly.
    let content_prefix: String = entry.content.chars().take(200).collect();

    // Wrap dedup-check + BOTH inserts (main row + FTS index) in ONE transaction.
    // Two bugs this closes:
    //  1. Drift: if the FTS INSERT failed (locked DB / disk error) the main row
    //     INSERT already committed, so the entry was persisted but NEVER
    //     searchable. The dedup check reads only `knowledge_entries`, so every
    //     retry would silently re-accept it — a lesson that lives invisibly
    //     forever. Rolling both back together keeps the tables consistent.
    //  2. Race: a concurrent writer (the knowledge watcher fires on .jsonl
    //     change while a session's collector runs) could slip a near-duplicate
    //     between our SELECT and INSERT. Holding the writer lock across both
    //     serializes them.
    // `unchecked_transaction` works on the borrowed pooled &Connection (the
    // owned-`&mut` `.transaction()` is unavailable here); it rolls back on drop.
    let tx = conn.unchecked_transaction()?;
    let exists: bool = tx
        .query_row(
            "SELECT COUNT(*) FROM knowledge_entries WHERE project_hash = ?1 AND SUBSTR(content, 1, 200) = ?2",
            params![entry.project_hash, content_prefix],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)
        .unwrap_or(false);

    if exists {
        tx.commit()?;
        return Ok(());
    }

    // D5 gate dedup: a re-learned lesson with the SAME title in this project
    // supersedes the prior active entry — the old row is marked status=
    // 'superseded' (kept for history; retrieve_relevant's status='active'
    // filter excludes it from injection) and the new row inserts as the
    // active one. Without this, re-running a task piles up N near-identical
    // react_session/react_reflection rows that all match the same FTS query.
    // Case-insensitive title match; blank titles skip (build_*_entry never
    // emits blank, so this guards only against malformed callers).
    if !entry.title.trim().is_empty() {
        tx.execute(
            "UPDATE knowledge_entries SET status = 'superseded' \
             WHERE project_hash = ?1 AND lower(title) = lower(?2) AND status = 'active'",
            params![entry.project_hash, entry.title],
        )?;
    }

    tx.execute(
        "INSERT INTO knowledge_entries
            (id, project_hash, category, title, content, source_agent,
             source_session_id, source_type, confidence, created_at, updated_at,
             access_count, status, effectiveness)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            entry.id,
            entry.project_hash,
            entry.category,
            entry.title,
            entry.content,
            serde_json::to_string(&entry.source_agent)?.trim_matches('"'),
            entry.source_session_id,
            entry.source_type,
            entry.confidence,
            entry.created_at,
            entry.updated_at,
            entry.access_count,
            entry.status,
            entry.effectiveness,
        ],
    )?;
    // Keep FTS index in sync (same transaction — rolls back with the row above).
    sync_fts(
        &tx,
        &entry.id,
        FtsOp::Insert {
            title: entry.title.clone(),
            content: entry.content.clone(),
        },
    )?;
    tx.commit()?;
    Ok(())
}

/// Build a `KnowledgeEntry` capturing a completed self-built ReactAgent
/// session's contribution to long-term memory (v1.3 T2). The opaque CLI path
/// feeds the knowledge flywheel via [`collect_from_session`] (it reads their
/// JSONL/sqlite logs); the kernel agent has no such log, so its completed
/// output is written directly as one `react_session` entry — closing the loop
/// so the NEXT session's `memory_prompt_suffix` can surface it.
///
/// `content` is capped at 1000 chars so a verbose run doesn't bloat the FTS
/// index or every future system prompt.
pub fn build_session_memory_entry(
    project_hash: &str,
    session_id: &str,
    title: &str,
    content: &str,
    agent_type: &AgentType,
) -> KnowledgeEntry {
    let title: String = title
        .lines()
        .next()
        .unwrap_or(title)
        .chars()
        .take(120)
        .collect();
    let content: String = content.chars().take(1000).collect();
    KnowledgeEntry {
        id: uuid::Uuid::new_v4().to_string(),
        project_hash: project_hash.to_string(),
        category: "react_session".to_string(),
        title,
        content,
        source_agent: agent_type.clone(),
        source_session_id: Some(session_id.to_string()),
        source_type: "react_agent".to_string(),
        confidence: 0.6,
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
        access_count: 0,
            status: "active".to_string(),
            effectiveness: 0.0,
    }
}

/// Build a `react_reflection` KnowledgeEntry — the STRUCTURED companion to a
/// session's `react_session` natural-language memory (D6 reflection). Where
/// [`build_session_memory_entry`] stores what the agent SAID, this stores what
/// it DID (tool usage / files touched / errors) so the next session can match
/// on behavior patterns via FTS. `title`/`content` are pre-formatted by
/// `kernel_impl::session_reflection::summarize`; we only cap + tag here.
pub fn build_session_reflection_entry(
    project_hash: &str,
    session_id: &str,
    title: &str,
    content: &str,
    agent_type: &AgentType,
) -> KnowledgeEntry {
    KnowledgeEntry {
        id: uuid::Uuid::new_v4().to_string(),
        project_hash: project_hash.to_string(),
        category: "react_reflection".to_string(),
        title: title.chars().take(120).collect(),
        content: content.chars().take(1000).collect(),
        source_agent: agent_type.clone(),
        source_session_id: Some(session_id.to_string()),
        source_type: "react_agent".to_string(),
        confidence: 0.6,
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
        access_count: 0,
            status: "active".to_string(),
            effectiveness: 0.0,
    }
}

/// Build a safe FTS5 query from raw user input with BROAD RECALL: split into
/// alphanumeric terms, double-quote each, OR-join. A multi-word search like
/// "tokio async" then matches any document containing either token — the
/// recall a search box needs.
///
/// The previous implementation wrapped the WHOLE input as one FTS5 phrase,
/// which required every token to appear CONSECUTIVELY. Normal multi-word
/// queries ("tokio async", "error handling thiserror") matched nothing and
/// search_entries silently returned empty — the v07 integration regressions.
///
/// Safety (F8) is preserved: each term is alphanumeric-only and quoted, so
/// bare special chars (`"`/`*`/`(`/`)`) and operator words (`OR`/`NOT`) in the
/// input can't raise `fts5: syntax error` or inject operators.
fn sanitize_fts_query(query: &str) -> String {
    let mut terms: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in query.chars() {
        if ch.is_alphanumeric() {
            cur.push(ch);
        } else if !cur.is_empty() {
            terms.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        terms.push(cur);
    }
    // Case-insensitive dedup: "Rust rust RUST" → one term (FTS5 unicode61
    // lowercases anyway; dedup avoids a redundant 3x OR of the same token).
    let mut seen = std::collections::HashSet::new();
    let quoted: Vec<String> = terms
        .into_iter()
        .filter(|t| seen.insert(t.to_lowercase()))
        .map(|t| format!("\"{t}\""))
        .collect();
    if quoted.is_empty() {
        // Empty / all-special-chars input — a phrase that matches nothing,
        // never an FTS syntax error.
        return "\"\"".to_string();
    }
    quoted.join(" OR ")
}

/// Search knowledge entries using FTS5 full-text search.
pub fn search_entries(
    conn: &rusqlite::Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<KnowledgeEntry>, AppError> {
    // bm25 relevance ranking: JOIN the FTS table so bm25(knowledge_fts) is in
    // scope (it isn't with the old `rowid IN (subquery)` form — that's why the
    // doc claimed bm25 but the SQL sorted by updated_at, a doc/impl mismatch).
    // bm25 returns negative values by convention; ASC puts the most relevant
    // (most negative) first, updated_at as a tiebreaker.
    let mut stmt = conn.prepare(
        "SELECT ke.id, ke.project_hash, ke.category, ke.title, ke.content, \
                ke.source_agent, ke.source_session_id, ke.source_type, ke.confidence, \
                ke.created_at, ke.updated_at, ke.access_count, ke.status, ke.effectiveness \
         FROM knowledge_fts \
         JOIN knowledge_entries ke ON ke.rowid = knowledge_fts.rowid \
         WHERE knowledge_fts MATCH ?1 \
         ORDER BY bm25(knowledge_fts) ASC, ke.updated_at DESC \
         LIMIT ?2",
    )?;

    let safe = sanitize_fts_query(query);
    let entries = stmt.query_map(params![&safe, limit as i64], row_to_entry)?;
    let mut result = Vec::new();
    for e in entries {
        result.push(e?);
    }
    Ok(result)
}

/// FTS5 search scoped to a single project, filtered by confidence, ranked by
/// bm25 relevance. Only `status = 'active'` entries are returned (I4: pending /
/// superseded lessons are not injected).
pub fn search_entries_for_project(
    conn: &rusqlite::Connection,
    project_hash: &str,
    fts_query: &str,
    confidence_min: f64,
    limit: usize,
) -> Result<Vec<KnowledgeEntry>, AppError> {
    // JOIN the FTS table so bm25(knowledge_fts) is in scope for relevance
    // ranking (the old `rowid IN (subquery)` form could only sort by updated_at,
    // not bm25 — the doc/impl mismatch this fixes).
    let mut stmt = conn.prepare(
        "SELECT ke.id, ke.project_hash, ke.category, ke.title, ke.content, \
                ke.source_agent, ke.source_session_id, ke.source_type, ke.confidence, \
                ke.created_at, ke.updated_at, ke.access_count, ke.status, ke.effectiveness \
         FROM knowledge_fts \
         JOIN knowledge_entries ke ON ke.rowid = knowledge_fts.rowid \
         WHERE knowledge_fts MATCH ?1 \
         AND ke.project_hash = ?2 \
         AND ke.confidence >= ?3 \
         AND ke.status = 'active' \
         ORDER BY bm25(knowledge_fts) ASC, ke.updated_at DESC \
         LIMIT ?4",
    )?;

    // fts_query is ALREADY a pre-formatted FTS5 query: extract_keywords
    // emits safely double-quoted terms joined by OR (`"fix" OR "rust"`).
    // Re-running sanitize_fts_query here would wrap that whole OR expression
    // in one outer phrase → the literal sequence never matches anything, so
    // cross-project injection silently returned empty. sanitize_fts_query is
    // only for RAW user input (the search_entries path below).
    let entries = stmt.query_map(
        params![fts_query, project_hash, confidence_min, limit as i64],
        row_to_entry,
    )?;
    let mut result = Vec::new();
    for e in entries {
        result.push(e?);
    }
    Ok(result)
}

/// FTS5 search across all projects except the given one. Used for cross-project
/// knowledge sharing. Only `status = 'active'` entries (I4), ranked by bm25.
pub fn search_entries_cross_project(
    conn: &rusqlite::Connection,
    exclude_project_hash: &str,
    fts_query: &str,
    confidence_min: f64,
    limit: usize,
) -> Result<Vec<KnowledgeEntry>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT ke.id, ke.project_hash, ke.category, ke.title, ke.content, \
                ke.source_agent, ke.source_session_id, ke.source_type, ke.confidence, \
                ke.created_at, ke.updated_at, ke.access_count, ke.status, ke.effectiveness \
         FROM knowledge_fts \
         JOIN knowledge_entries ke ON ke.rowid = knowledge_fts.rowid \
         WHERE knowledge_fts MATCH ?1 \
         AND ke.project_hash != ?2 \
         AND ke.confidence >= ?3 \
         AND ke.status = 'active' \
         ORDER BY bm25(knowledge_fts) ASC, ke.updated_at DESC \
         LIMIT ?4",
    )?;

    // fts_query is a pre-formatted FTS5 OR query from extract_keywords — do
    // NOT sanitize_fts_query here (see search_entries_for_project above).
    let entries = stmt.query_map(
        params![
            fts_query,
            exclude_project_hash,
            confidence_min,
            limit as i64
        ],
        row_to_entry,
    )?;
    let mut result = Vec::new();
    for e in entries {
        result.push(e?);
    }
    Ok(result)
}

/// Increment `access_count` for the given entry IDs (I5: track which memories
/// were actually injected so the effectiveness feedback loop can weight by
/// reuse, and so access_count is no longer a write-never field). Best-effort:
/// a DB error is propagated but callers gate injection on retrieval, not on
/// this counter — a failed bump must not block memory injection.
pub fn bump_access_counts(
    conn: &rusqlite::Connection,
    ids: &[String],
) -> Result<usize, AppError> {
    if ids.is_empty() {
        return Ok(0);
    }
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "UPDATE knowledge_entries SET access_count = access_count + 1 WHERE id IN ({})",
        placeholders
    );
    let params: Vec<&dyn rusqlite::ToSql> =
        ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
    conn.execute(&sql, params.as_slice()).map_err(AppError::from)
}

/// Increment `effectiveness` for the given entry IDs (I5 feedback loop). Unlike
/// [`bump_access_counts`] (bumped on every injection), `effectiveness` reflects
/// OUTCOME feedback. Best-effort — callers gate on retrieval, not this counter.
pub fn bump_effectiveness(
    conn: &rusqlite::Connection,
    ids: &[String],
) -> Result<usize, AppError> {
    if ids.is_empty() {
        return Ok(0);
    }
    let placeholders = ids
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "UPDATE knowledge_entries SET effectiveness = effectiveness + 1 WHERE id IN ({})",
        placeholders
    );
    let params: Vec<&dyn rusqlite::ToSql> =
        ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
    conn.execute(&sql, params.as_slice()).map_err(AppError::from)
}

/// Bump `effectiveness` for every entry a session PRODUCED (I5 self-feedback).
/// A Completed session's `react_session`/`react_reflection` output is the most
/// reliable signal a lesson is worth re-injecting, so the completion hook marks
/// it effective +1 — and the next session's retrieval ranks it higher. This
/// closes the loop from `source_session_id` alone, WITHOUT tracking which
/// memories were injected into the run (that would need a per-injection ledger
/// threaded across the kernel sys_prompt build AND the pty inject thread — high
/// blast radius for marginal signal). Only `status='active'` rows are bumped;
/// superseded history keeps its old score.
pub fn bump_effectiveness_by_session(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<usize, AppError> {
    conn.execute(
        "UPDATE knowledge_entries SET effectiveness = effectiveness + 1 \
         WHERE source_session_id = ?1 AND status = 'active'",
        params![session_id],
    )
    .map_err(AppError::from)
}

// ---------------------------------------------------------------------------
// I1: vector embeddings (FTS-confidence fallback storage)
// ---------------------------------------------------------------------------

/// Encode an embedding vector as little-endian f32 bytes for BLOB storage.
/// `Vec<f32>` ↔ `&[u8]` round-trips losslessly; dim is stored separately so a
/// read can validate length before casting.
pub(crate) fn encode_embedding(vec: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vec.len() * 4);
    for v in vec {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes
}

/// Decode a stored BLOB back into `Vec<f32>`. A length that isn't a multiple of
/// 4 (corrupt/truncated row) yields a SHORTER vec via `chunks_exact` rather than
/// panicking — a bad row is silently dropped at retrieval, never a hard crash.
pub(crate) fn decode_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Upsert an embedding for a knowledge entry (I1). The completion hook embeds
/// each newly-written entry's text and stores it, so the NEXT session's
/// `retrieve_relevant_with_vector` can cosine-rank it when FTS confidence is
/// low. `model` is the embedding model id (cache invalidation on a model swap);
/// `dim` is derived from the vector length.
pub fn upsert_embedding(
    conn: &rusqlite::Connection,
    entry_id: &str,
    embedding: &[f32],
    model: &str,
) -> Result<(), AppError> {
    let bytes = encode_embedding(embedding);
    let dim = embedding.len() as i64;
    conn.execute(
        "INSERT INTO knowledge_embeddings (entry_id, embedding, dim, model, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(entry_id) DO UPDATE SET \
            embedding = excluded.embedding, \
            dim = excluded.dim, \
            model = excluded.model, \
            created_at = excluded.created_at",
        params![entry_id, bytes, dim, model, chrono::Local::now().to_rfc3339()],
    )?;
    Ok(())
}

/// Return `(id, title, content)` for every ACTIVE entry a session PRODUCED —
/// the I1 write-side input. The completion hook embeds `title + "\n" + content`
/// for each row (same text shape a future retrieval query is embedded against,
/// so doc/query live in one semantic space) and stores it via
/// [`upsert_embedding`]. Best-effort: a DB error propagates and the caller
/// skips the embed round (never blocks the completion path).
pub fn entries_by_session(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Vec<(String, String, String)>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, title, content FROM knowledge_entries \
         WHERE source_session_id = ?1 AND status = 'active'",
    )?;
    let rows = stmt.query_map(params![session_id], |row| {
        // F4: named-column reads (the SELECT is already explicit; this pins
        // the reads to column names, not position).
        Ok((
            row.get::<_, String>("id")?,
            row.get::<_, String>("title")?,
            row.get::<_, String>("content")?,
        ))
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Load every ACTIVE entry's embedding for a project, filtered to the given
/// embedding model id. A model swap ignores stale rows (different `model`)
/// rather than cosine-ranking incomparable vectors — the next completion
/// re-embeds under the new id. Returns `(entry_id, vector)`; the caller joins
/// back to `knowledge_entries` for content + category gates. status='active' is
/// filtered HERE so a pending/superseded row is never cosine-ranked.
pub fn get_active_embeddings(
    conn: &rusqlite::Connection,
    project_hash: &str,
    model: &str,
) -> Result<Vec<(String, Vec<f32>)>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT ke.entry_id, ke.embedding \
         FROM knowledge_embeddings ke \
         JOIN knowledge_entries kn ON kn.id = ke.entry_id \
         WHERE kn.project_hash = ?1 AND kn.status = 'active' AND ke.model = ?2",
    )?;
    let rows = stmt.query_map(params![project_hash, model], |row| {
        let id: String = row.get(0)?;
        let blob: Vec<u8> = row.get(1)?;
        Ok((id, decode_embedding(&blob)))
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Delete knowledge entries older than `max_age_days`. Also cleans up FTS rows.
/// Returns the number of deleted entries.
pub fn prune_old_entries(
    conn: &rusqlite::Connection,
    max_age_days: i64,
) -> Result<usize, AppError> {
    let cutoff = chrono::Local::now() - chrono::Duration::days(max_age_days);
    let cutoff_str = cutoff.to_rfc3339();

    // Wrap in a transaction to keep FTS and the main table consistent. Use the
    // rusqlite Transaction guard, NOT a raw `execute_batch("BEGIN")`: the guard
    // rolls back automatically on early return / error and `commit()` closes
    // the txn cleanly. A raw BEGIN whose ROLLBACK is ignored (`let _ = …`) can
    // leave a pooled connection with an open transaction, poisoning every later
    // query routed through it. Same fix the knowledge `add_entry` path already
    // received; `unchecked_transaction` because `conn` is borrowed immutably.
    let tx = conn.unchecked_transaction()?;

    // Collect IDs to delete
    let ids: Vec<String> = {
        let mut stmt = tx.prepare("SELECT id FROM knowledge_entries WHERE updated_at < ?1")?;
        let rows = stmt.query_map(params![cutoff_str], |row| row.get::<_, String>(0))?;
        let mut v = Vec::new();
        for r in rows {
            v.push(r?);
        }
        v
    };

    let count = ids.len();
    if count == 0 {
        // Nothing to delete — commit the empty txn so the guard doesn't roll it
        // back, then return.
        tx.commit()?;
        return Ok(0);
    }

    // Delete FTS rows first (by rowid) — F5: funneled through sync_fts.
    // sync_fts's rowid lookup also surfaces rows that vanished between the
    // collect-IDs SELECT and now (concurrent writer raced us); the `let _`
    // discards that NotFound, matching the previous `if let Ok` guard. Order
    // matters: FTS DELETE first, then main DELETE, so a mid-prune crash
    // leaves a main row that is no longer in the FTS index — search excludes
    // it (mildly bloated result count), but the main-row reference is still
    // there for any external FK consumers. Reverse order would leave a
    // dangling FTS rowid pointing at nothing; the JOIN silently drops it.
    for id in &ids {
        let _ = sync_fts(&tx, id, FtsOp::Delete);
    }

    // Delete main entries
    for id in &ids {
        tx.execute("DELETE FROM knowledge_entries WHERE id = ?1", params![id])?;
    }

    log::info!(
        "Knowledge prune: deleted {} entries older than {} days",
        count,
        max_age_days
    );
    tx.commit()?;
    Ok(count)
}

/// Look up a single active knowledge entry by exact (case-insensitive) title
/// within a project. Used by the `@memory:<title>` explicit-reference feature
/// (D3) — the user names a SPECIFIC memory to inject, as opposed to the
/// implicit FTS retrieval in [`super::retrieval::retrieve_relevant`]. Only
/// `status='active'` matches (pending/superseded are not referenceable). When
/// several entries share a title, the most recently updated wins.
pub fn get_entry_by_title_for_project(
    conn: &rusqlite::Connection,
    project_hash: &str,
    title: &str,
) -> Result<Option<KnowledgeEntry>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, project_hash, category, title, content, source_agent, \
                source_session_id, source_type, confidence, created_at, updated_at, \
                access_count, status, effectiveness \
         FROM knowledge_entries \
         WHERE project_hash = ?1 AND lower(title) = lower(?2) AND status = 'active' \
         ORDER BY updated_at DESC LIMIT 1",
    )?;
    let mut entries = stmt.query_map(params![project_hash, title], row_to_entry)?;
    match entries.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

/// Get all knowledge entries for a project.
pub fn get_entries_for_project(
    conn: &rusqlite::Connection,
    project_hash: &str,
) -> Result<Vec<KnowledgeEntry>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, project_hash, category, title, content, source_agent, \
                source_session_id, source_type, confidence, created_at, updated_at, \
                access_count, status, effectiveness \
         FROM knowledge_entries WHERE project_hash = ?1 ORDER BY updated_at DESC",
    )?;

    let entries = stmt.query_map(params![project_hash], row_to_entry)?;
    let mut result = Vec::new();
    for e in entries {
        result.push(e?);
    }
    Ok(result)
}

/// Delete a knowledge entry by ID.
pub fn delete_entry(conn: &rusqlite::Connection, id: &str) -> Result<(), AppError> {
    // T6 atomicity: the main row and its FTS row must go together. Without a
    // transaction wrapper, a crash between the two DELETEs leaves a dangling
    // FTS rowid — the search JOIN silently drops it so the entry just stops
    // matching (mild leak). Worse case: if main goes first and a future INSERT
    // ever reuses that rowid, the new entry's add_entry DELETE-by-rowid (FTS
    // sync path) would erase the wrong slot. Same consistency contract as
    // add_entry / update_entry: row mutation + FTS sync share one transaction
    // so a crash rolls both back together.
    let tx = conn.unchecked_transaction()?;
    // sync_fts FIRST: its internal `SELECT rowid FROM knowledge_entries
    // WHERE id = ?1` must see the row still present, otherwise the lookup
    // returns `QueryReturnedNoRows` → `NotFound`, and `tx` is dropped without
    // commit, rolling the main DELETE back. Same FTS-first ordering as
    // `prune_old_entries` (orphan main row, if a mid-prune crash happens, is
    // a no-op for search; a deleted entry with a stale FTS row would silently
    // match in searches).
    sync_fts(&tx, id, FtsOp::Delete)?;
    tx.execute("DELETE FROM knowledge_entries WHERE id = ?1", params![id])?;
    tx.commit()?;
    Ok(())
}

/// Update a knowledge entry's title + content (the user-editable fields) and
/// keep the FTS index in sync so the edited text is searchable. User-facing
/// reason this exists (C1): a mislearned lesson — wrong root cause, stale
/// convention — gets re-injected into every future session's prompt until fixed;
/// before this the only remedy was delete-and-lose. Edit lets the user correct
/// the lesson in place. Other fields (category/confidence/sources) are structural
/// and stay unchanged. Bumps `updated_at` so the edit sorts as recent.
pub fn update_entry(
    conn: &rusqlite::Connection,
    id: &str,
    title: &str,
    content: &str,
) -> Result<(), AppError> {
    let now = chrono::Local::now().to_rfc3339();
    let tx = conn.unchecked_transaction()?;
    // Verify the row exists (mirrors delete_entry's NotFound contract) — a miss
    // is a programming error, not a silent no-op. Exists separately from
    // sync_fts's rowid lookup; the helper resolves rowid at FTS op time, so
    // total cost is `SELECT 1` (this) + `SELECT rowid` (sync_fts Delete) +
    // `SELECT rowid` (sync_fts Insert) — three cheap point lookups in one
    // transaction. F5's design (FTS-helper-is-source-of-rowid) trades this
    // redundancy for "one place that knows how FTS rowid is wired".
    tx.query_row(
        "SELECT 1 FROM knowledge_entries WHERE id = ?1",
        params![id],
        |_| Ok(()),
    )
    .map_err(|_| AppError::NotFound(format!("Knowledge entry {} 不存在", id)))?;
    tx.execute(
        "UPDATE knowledge_entries SET title = ?1, content = ?2, updated_at = ?3 WHERE id = ?4",
        params![title, content, now, id],
    )?;
    // Keep FTS in sync within the same transaction (same consistency contract as
    // add_entry): delete the old indexed row, insert the new title/content. If
    // this were skipped the main row would show the edit but search would still
    // match the old text — an invisible lie about what the prompt injects.
    sync_fts(&tx, id, FtsOp::Delete)?;
    sync_fts(
        &tx,
        id,
        FtsOp::Insert {
            title: title.to_string(),
            content: content.to_string(),
        },
    )?;
    tx.commit()?;
    Ok(())
}

/// Set the confidence of a knowledge entry and bump `updated_at` (D6 improvement
/// tracking). Resolved-but-not-accepted reviews decay their lessons' confidence
/// instead of deleting them, so the experience flywheel keeps a traceable record
/// of what was improved — purge (full exit) is reserved for accepted reviews.
/// Bumps `updated_at` so the decayed row sorts as recent in recency rankings.
pub fn set_entry_confidence(
    conn: &rusqlite::Connection,
    id: &str,
    confidence: f64,
) -> Result<(), AppError> {
    let now = chrono::Local::now().to_rfc3339();
    conn.execute(
        "UPDATE knowledge_entries SET confidence = ?1, updated_at = ?2 WHERE id = ?3",
        params![confidence, now, id],
    )?;
    Ok(())
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> Result<KnowledgeEntry, rusqlite::Error> {
    // Named-column reads (F2/F3): `row.get("col")` makes a future schema add
    // (e.g. a new NOT NULL column) fail-loud with `InvalidColumnName` at the
    // first read instead of silently falling back to defaults. Pair with the
    // explicit-column SELECTs above; a `SELECT *` path is still correct, but
    // column names anchor this function to the schema, not to the SELECT order.
    let agent_type_str: String = row.get("source_agent")?;
    let agent_type: AgentType = serde_json::from_value(serde_json::Value::String(agent_type_str))
        .unwrap_or(AgentType::ClaudeCode);

    Ok(KnowledgeEntry {
        id: row.get("id")?,
        project_hash: row.get("project_hash")?,
        category: row.get("category")?,
        title: row.get("title")?,
        content: row.get("content")?,
        source_agent: agent_type,
        source_session_id: row.get("source_session_id")?,
        source_type: row.get("source_type")?,
        confidence: row.get("confidence")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        access_count: row.get("access_count")?,
        status: row.get("status")?,
        effectiveness: row.get("effectiveness")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::models::AgentType;

    #[test]
    fn sanitize_fts_query_or_joins_terms_and_neutralizes_special_chars() {
        // F8: bare special chars (`"`/`*`/`(`/`)`/`OR`/`NOT`) would raise
        // `fts5: syntax error` and fail the whole knowledge search. Splitting
        // into alphanumeric-only terms and quoting each neutralizes operators;
        // OR-joining gives multi-word searches the recall they need (any term
        // matches), replacing the old whole-string phrase wrap that required
        // consecutive tokens and returned empty for "tokio async" etc.
        assert_eq!(sanitize_fts_query("rust async"), "\"rust\" OR \"async\"");
        // Special chars are term separators, not operators: "c++" → "c".
        assert_eq!(sanitize_fts_query("c++ (templates)"), "\"c\" OR \"templates\"");
        // Internal quotes drop out (alphanumeric-only terms).
        assert_eq!(sanitize_fts_query("a \"b\" c"), "\"a\" OR \"b\" OR \"c\"");
        // Case-insensitive dedup (FTS5 unicode61 lowercases anyway).
        assert_eq!(sanitize_fts_query("Rust rust RUST"), "\"Rust\"");
        // Empty / all-special input → matches nothing, never a syntax error.
        assert_eq!(sanitize_fts_query(""), "\"\"");
        assert_eq!(sanitize_fts_query("+++"), "\"\"");
    }

    struct TempDb {
        _tmp: tempfile::TempDir,
        conn: rusqlite::Connection,
    }

    impl TempDb {
        fn new() -> Self {
            let tmp = tempfile::TempDir::new().unwrap();
            let db_path = tmp.path().join("test.db");
            let conn = db::init_db(&db_path).expect("init_db failed");
            Self { _tmp: tmp, conn }
        }
    }

    fn make_entry(id: &str, project_hash: &str, title: &str, content: &str) -> KnowledgeEntry {
        KnowledgeEntry {
            id: id.to_string(),
            project_hash: project_hash.to_string(),
            category: "insight".to_string(),
            title: title.to_string(),
            content: content.to_string(),
            source_agent: AgentType::ClaudeCode,
            source_session_id: None,
            source_type: "auto_collect".to_string(),
            confidence: 0.8,
            created_at: chrono::Local::now().to_rfc3339(),
            updated_at: chrono::Local::now().to_rfc3339(),
            access_count: 0,
            status: "active".to_string(),
            effectiveness: 0.0,
        }
    }

    /// F2/F3 regression: row_to_entry reads `status` and `effectiveness` from
    /// the actual columns (not positional defaults). If a future schema change
    /// drops a column or renames it, this test fails loud — better than a
    /// silent `unwrap_or` fallback that would resurrect "active" rows or
    /// zero-out the effectiveness feedback loop.
    #[test]
    fn row_to_entry_preserves_status_and_effectiveness() {
        let db = TempDb::new();
        // entry with non-default status + non-zero effectiveness
        let mut entry = make_entry("k1", "proj_a", "Lesson", "Body");
        entry.status = "superseded".to_string();
        entry.effectiveness = 0.73;
        add_entry(&db.conn, &entry).unwrap();

        let rows = get_entries_for_project(&db.conn, "proj_a").unwrap();
        assert_eq!(rows.len(), 1, "expected one entry, got {}", rows.len());
        let got = &rows[0];
        // F2: status read from the column, NOT the unwrap_or default.
        assert_eq!(
            got.status, "superseded",
            "row_to_entry dropped status; the unwrap_or fallback would have produced 'active'"
        );
        // F2: effectiveness read from the column, NOT the unwrap_or default.
        assert!(
            (got.effectiveness - 0.73).abs() < 1e-9,
            "row_to_entry dropped effectiveness (got {}); unwrap_or default is 0.0",
            got.effectiveness
        );
    }

    #[test]
    fn test_add_and_search() {
        let db = TempDb::new();
        let e1 = make_entry(
            "k1",
            "proj_a",
            "Rust error handling",
            "Use thiserror for error types in Rust",
        );
        let e2 = make_entry(
            "k2",
            "proj_a",
            "CSS variables",
            "Define CSS custom properties for theming",
        );
        let e3 = make_entry(
            "k3",
            "proj_b",
            "Tauri commands",
            "Use State for dependency injection",
        );

        add_entry(&db.conn, &e1).unwrap();
        add_entry(&db.conn, &e2).unwrap();
        add_entry(&db.conn, &e3).unwrap();

        let results = search_entries(&db.conn, "Rust error", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "k1");
    }

    #[test]
    fn test_get_entries_for_project() {
        let db = TempDb::new();
        add_entry(
            &db.conn,
            &make_entry("k1", "proj_a", "Title 1", "Content 1"),
        )
        .unwrap();
        add_entry(
            &db.conn,
            &make_entry("k2", "proj_b", "Title 2", "Content 2"),
        )
        .unwrap();
        add_entry(
            &db.conn,
            &make_entry("k3", "proj_a", "Title 3", "Content 3"),
        )
        .unwrap();

        let proj_a = get_entries_for_project(&db.conn, "proj_a").unwrap();
        assert_eq!(proj_a.len(), 2);
    }

    #[test]
    fn test_delete_entry() {
        let db = TempDb::new();
        add_entry(&db.conn, &make_entry("k1", "proj_a", "Title", "Content")).unwrap();
        delete_entry(&db.conn, "k1").unwrap();
        assert!(delete_entry(&db.conn, "k1").is_err());
    }

    #[test]
    fn delete_entry_removes_main_and_fts_atomically() {
        // T6 regression guard: delete_entry wraps the main + FTS DELETEs in
        // one transaction. A non-atomic version (main DELETE then FTS DELETE)
        // would let a crash between them leak a dangling FTS rowid — search
        // silently drops the orphan, so the entry just stops matching for
        // good. Verify the happy path keeps both tables in sync.
        let db = TempDb::new();
        add_entry(
            &db.conn,
            &make_entry("k1", "proj_a", "findme", "searchable rust tokio content"),
        )
        .unwrap();

        let rowid: i64 = db
            .conn
            .query_row(
                "SELECT rowid FROM knowledge_entries WHERE id = 'k1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let fts_before: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM knowledge_fts WHERE rowid = ?1",
                rusqlite::params![rowid],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fts_before, 1, "FTS row must exist before delete");

        delete_entry(&db.conn, "k1").unwrap();

        let main_after: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM knowledge_entries WHERE id = 'k1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(main_after, 0, "main row must be gone after delete");
        let fts_after: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM knowledge_fts WHERE rowid = ?1",
                rusqlite::params![rowid],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fts_after, 0, "FTS row must be gone after delete (atomic)");
        // And the deleted entry no longer matches FTS search.
        assert!(
            !search_entries(&db.conn, "tokio", 10)
                .unwrap()
                .iter()
                .any(|x| x.id == "k1"),
            "deleted entry must not match FTS"
        );
    }

    #[test]
    fn test_dedup_same_content_skipped() {
        let db = TempDb::new();
        add_entry(
            &db.conn,
            &make_entry(
                "k1",
                "proj_a",
                "Title",
                "Same content here that is long enough",
            ),
        )
        .unwrap();
        // Same project + same content prefix → should be silently skipped
        add_entry(
            &db.conn,
            &make_entry(
                "k2",
                "proj_a",
                "Title 2",
                "Same content here that is long enough",
            ),
        )
        .unwrap();
        let entries = get_entries_for_project(&db.conn, "proj_a").unwrap();
        assert_eq!(entries.len(), 1); // dedup: only first inserted
        assert_eq!(entries[0].id, "k1");
    }

    #[test]
    fn test_dedup_different_content_allowed() {
        let db = TempDb::new();
        add_entry(
            &db.conn,
            &make_entry("k1", "proj_a", "Title", "Content about Rust"),
        )
        .unwrap();
        add_entry(
            &db.conn,
            &make_entry("k2", "proj_a", "Title 2", "Content about Python"),
        )
        .unwrap();
        let entries = get_entries_for_project(&db.conn, "proj_a").unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_add_entry_multibyte_content_does_not_panic() {
        // Regression: the OLD `&content[..content.len().min(200)]` byte-sliced.
        // 300 CJK chars = 900 bytes, so byte index 200 lands mid-char (inside
        // '的' at bytes 198..201) → panic "byte index 200 is not a char boundary".
        // This fired on every react_kernel completion (CJK output). Char-based
        // truncation must never panic and must insert cleanly.
        let db = TempDb::new();
        let cjk = "的".repeat(300);
        add_entry(&db.conn, &make_entry("k1", "proj_a", "中文知识", &cjk)).unwrap();
        let entries = get_entries_for_project(&db.conn, "proj_a").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content.chars().count(), 300);
    }

    #[test]
    fn test_dedup_multibyte_uses_char_prefix_not_byte() {
        // Dedup must key on the first 200 CHARS (matching SQLite SUBSTR), not
        // 200 bytes. Two CJK entries that share their first 200 chars but diverge
        // afterward must dedup; two that differ within the first 200 chars must not.
        let db = TempDb::new();
        let shared = "知识".repeat(150); // 300 chars; first 200 chars identical below
        let mut same_prefix_a = shared.clone();
        same_prefix_a.push_str("尾巴甲"); // diverge AFTER the 200-char window
        let mut same_prefix_b = shared;
        same_prefix_b.push_str("尾巴乙");
        add_entry(&db.conn, &make_entry("k1", "proj_a", "T1", &same_prefix_a)).unwrap();
        add_entry(&db.conn, &make_entry("k2", "proj_a", "T2", &same_prefix_b)).unwrap();
        let entries = get_entries_for_project(&db.conn, "proj_a").unwrap();
        assert_eq!(entries.len(), 1); // dedup: same first-200-char prefix
        assert_eq!(entries[0].id, "k1");
    }

    #[test]
    fn test_search_entries_for_project() {
        let db = TempDb::new();
        add_entry(
            &db.conn,
            &make_entry(
                "k1",
                "proj_a",
                "Rust error handling",
                "Use thiserror for error types in Rust",
            ),
        )
        .unwrap();
        add_entry(
            &db.conn,
            &make_entry(
                "k2",
                "proj_a",
                "CSS theming",
                "Define CSS custom properties for dark mode",
            ),
        )
        .unwrap();
        add_entry(
            &db.conn,
            &make_entry(
                "k3",
                "proj_b",
                "Rust async",
                "Use tokio for async runtime in Rust",
            ),
        )
        .unwrap();

        // Scoped to proj_a, search for "Rust" → should only get k1
        let results = search_entries_for_project(&db.conn, "proj_a", "Rust", 0.5, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "k1");
    }

    #[test]
    fn test_search_entries_cross_project() {
        let db = TempDb::new();
        add_entry(
            &db.conn,
            &make_entry(
                "k1",
                "proj_a",
                "Rust error handling",
                "Use thiserror for error types in Rust",
            ),
        )
        .unwrap();
        add_entry(
            &db.conn,
            &make_entry(
                "k2",
                "proj_b",
                "Rust async runtime",
                "Use tokio for async Rust applications",
            ),
        )
        .unwrap();
        add_entry(
            &db.conn,
            &make_entry(
                "k3",
                "proj_b",
                "CSS theming",
                "Define CSS custom properties",
            ),
        )
        .unwrap();

        // Exclude proj_a, search for "Rust" → should get k2 from proj_b only
        let results = search_entries_cross_project(&db.conn, "proj_a", "Rust", 0.5, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "k2");
    }

    #[test]
    fn test_prune_old_entries() {
        let db = TempDb::new();
        // Recent entry
        let mut recent = make_entry("k1", "proj_a", "Recent", "Fresh content here");
        recent.updated_at = chrono::Local::now().to_rfc3339();
        add_entry(&db.conn, &recent).unwrap();

        // Old entry (200 days ago)
        let mut old = make_entry("k2", "proj_a", "Old", "Stale content from long ago");
        old.updated_at = (chrono::Local::now() - chrono::Duration::days(200)).to_rfc3339();
        add_entry(&db.conn, &old).unwrap();

        let before = get_entries_for_project(&db.conn, "proj_a").unwrap();
        assert_eq!(before.len(), 2);

        let pruned = prune_old_entries(&db.conn, 180).unwrap();
        assert_eq!(pruned, 1);

        let after = get_entries_for_project(&db.conn, "proj_a").unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, "k1");
    }

    #[test]
    fn set_entry_confidence_updates_confidence_and_bumps_updated_at() {
        // D6 improvement tracking: decaying a resolved lesson lowers confidence
        // (not deletes) and bumps updated_at so the row re-sorts as recent.
        let db = TempDb::new();
        let mut e = make_entry("k1", "proj_a", "no tests", "Forge 任务 x 评分 … no tests");
        e.confidence = 0.85;
        let stamp = "2020-01-01T00:00:00+08:00";
        e.updated_at = stamp.into();
        add_entry(&db.conn, &e).unwrap();

        set_entry_confidence(&db.conn, "k1", 0.425).unwrap();
        let got = get_entries_for_project(&db.conn, "proj_a").unwrap();
        assert_eq!(got.len(), 1);
        assert!(
            (got[0].confidence - 0.425).abs() < 1e-6,
            "confidence updated: {got:?}"
        );
        assert_ne!(got[0].updated_at, stamp, "updated_at bumped");
    }

    #[test]
    fn build_session_memory_entry_caps_content_and_tags_fields() {
        let e = build_session_memory_entry(
            "hash1",
            "sid1",
            "实现 auto-compact",
            &"x".repeat(2000),
            &AgentType::ClaudeCode,
        );
        assert_eq!(e.project_hash, "hash1");
        assert_eq!(e.category, "react_session");
        assert_eq!(e.source_type, "react_agent");
        assert_eq!(e.source_session_id.as_deref(), Some("sid1"));
        assert_eq!(e.source_agent, AgentType::ClaudeCode);
        assert!((e.confidence - 0.6).abs() < 1e-9);
        assert_eq!(
            e.content.chars().count(),
            1000,
            "content capped at 1000 chars"
        );
        assert_eq!(e.title, "实现 auto-compact");
        assert!(!e.id.is_empty());
    }

    #[test]
    fn build_session_memory_entry_takes_first_line_of_multiline_title() {
        let e = build_session_memory_entry(
            "h",
            "s",
            "第一行标题\n第二行不该出现",
            "内容",
            &AgentType::Codex,
        );
        assert_eq!(e.title, "第一行标题");
        assert!(!e.title.contains("第二行"));
        assert_eq!(e.source_agent, AgentType::Codex);
    }

    #[test]
    fn build_session_reflection_entry_tags_category_and_caps() {
        // D6 reflection builder: distinct category from react_session, same
        // confidence (0.6 → clears the memory-suffix threshold), title/content
        // capped so a noisy run can't bloat FTS.
        let e = build_session_reflection_entry(
            "hash1",
            "sid1",
            &"R".repeat(200),
            &"c".repeat(2000),
            &AgentType::ClaudeCode,
        );
        assert_eq!(
            e.category, "react_reflection",
            "distinct from react_session"
        );
        assert_eq!(e.source_type, "react_agent");
        assert_eq!(e.source_session_id.as_deref(), Some("sid1"));
        assert!((e.confidence - 0.6).abs() < 1e-9);
        assert_eq!(e.content.chars().count(), 1000, "content capped at 1000");
        assert!(
            e.title.chars().count() <= 120,
            "title capped at 120: {}",
            e.title.chars().count()
        );
    }

    #[test]
    fn update_entry_persists_and_keeps_fts_in_sync() {
        // C1: editing a mislearned lesson must (a) persist the new title/content
        // and (b) keep the FTS index in sync — the old text must stop matching
        // and the new text must start matching, or the prompt would still inject
        // the corrected-away lesson (an invisible lie about what's injected).
        let db = TempDb::new();
        let e = make_entry("k1", "proj_a", "old title", "old content about rust async");
        add_entry(&db.conn, &e).unwrap();
        // Sanity: the original term is searchable before the edit.
        assert!(search_entries(&db.conn, "rust", 10)
            .unwrap()
            .iter()
            .any(|x| x.id == "k1"));

        update_entry(&db.conn, "k1", "corrected title", "corrected content about python").unwrap();

        // Old term no longer matches (FTS row replaced).
        assert!(
            !search_entries(&db.conn, "rust", 10)
                .unwrap()
                .iter()
                .any(|x| x.id == "k1"),
            "old term must stop matching after edit"
        );
        // New term now matches.
        assert!(search_entries(&db.conn, "python", 10)
            .unwrap()
            .iter()
            .any(|x| x.id == "k1"));
        // Row persisted with the new title/content + bumped updated_at.
        let entries = get_entries_for_project(&db.conn, "proj_a").unwrap();
        let updated = entries.iter().find(|x| x.id == "k1").expect("entry survived edit");
        assert_eq!(updated.title, "corrected title");
        assert_eq!(updated.content, "corrected content about python");
    }

    #[test]
    fn add_entry_supersedes_same_title_active() {
        // D5: a re-learned lesson with the same title (case-insensitive) in the
        // same project supersedes the prior active row — old → status=
        // 'superseded' (kept for history, not injected), new → 'active'. Without
        // this, re-running a task piles up N near-identical react_session rows.
        let db = TempDb::new();
        add_entry(
            &db.conn,
            &make_entry("k1", "proj", "Error handling", "use thiserror v1"),
        )
        .unwrap();
        add_entry(
            &db.conn,
            &make_entry("k2", "proj", "error handling", "use thiserror v2 updated"),
        )
        .unwrap();

        let all = get_entries_for_project(&db.conn, "proj").unwrap();
        assert_eq!(all.len(), 2, "both rows kept (history)");
        let active: Vec<_> = all.iter().filter(|e| e.status == "active").collect();
        assert_eq!(active.len(), 1, "exactly one active");
        assert_eq!(active[0].id, "k2", "newest stays active");
        let old = all.iter().find(|e| e.id == "k1").unwrap();
        assert_eq!(old.status, "superseded");
    }

    #[test]
    fn add_entry_supersede_only_touches_active_same_title() {
        // Only an ACTIVE same-title row is superseded; a different title is
        // untouched, and an already-superseded row isn't re-touched.
        let db = TempDb::new();
        add_entry(&db.conn, &make_entry("k1", "proj", "Title A", "a1")).unwrap();
        add_entry(&db.conn, &make_entry("k2", "proj", "Title A", "a2")).unwrap();
        add_entry(&db.conn, &make_entry("k3", "proj", "Title B", "b1")).unwrap();
        add_entry(&db.conn, &make_entry("k4", "proj", "Title A", "a3")).unwrap();

        let all = get_entries_for_project(&db.conn, "proj").unwrap();
        let active: Vec<_> = all.iter().filter(|e| e.status == "active").collect();
        assert_eq!(active.len(), 2, "Title A newest + Title B");
        assert!(active.iter().any(|e| e.id == "k4"), "k4 active: {:?}", active);
        assert!(active.iter().any(|e| e.id == "k3"), "k3 active: {:?}", active);
        // k1 + k2 both superseded (k2 was active when k4 landed).
        assert_eq!(
            all.iter().filter(|e| e.status == "superseded").count(),
            2
        );
    }

    #[test]
    fn bump_effectiveness_increments_named_entries() {
        // I5: bump_effectiveness +1 the listed IDs only; unrelated entries stay.
        let db = TempDb::new();
        add_entry(&db.conn, &make_entry("e1", "proj", "Title 1", "c1")).unwrap();
        add_entry(&db.conn, &make_entry("e2", "proj", "Title 2", "c2")).unwrap();
        add_entry(&db.conn, &make_entry("e3", "proj", "Title 3", "c3")).unwrap();

        let n = bump_effectiveness(&db.conn, &["e1".to_string(), "e3".to_string()]).unwrap();
        assert_eq!(n, 2);

        let all = get_entries_for_project(&db.conn, "proj").unwrap();
        let by_id = |id: &str| {
            all.iter().find(|e| e.id == id).unwrap_or_else(|| panic!("missing {id}"))
        };
        assert_eq!(by_id("e1").effectiveness as i64, 1);
        assert_eq!(by_id("e2").effectiveness as i64, 0, "e2 must stay untouched");
        assert_eq!(by_id("e3").effectiveness as i64, 1);
    }

    #[test]
    fn bump_effectiveness_empty_ids_is_noop() {
        let db = TempDb::new();
        add_entry(&db.conn, &make_entry("x1", "proj", "Title", "c")).unwrap();
        let n = bump_effectiveness(&db.conn, &[]).unwrap();
        assert_eq!(n, 0);
        let all = get_entries_for_project(&db.conn, "proj").unwrap();
        assert_eq!(all[0].effectiveness as i64, 0);
    }

    #[test]
    fn bump_effectiveness_by_session_touches_only_active_owned() {
        // I5: a Completed session bumps effectiveness on every ACTIVE entry it
        // PRODUCED (source_session_id match). A superseded row from the same
        // session keeps its old score; an entry from a different session is
        // untouched.
        let db = TempDb::new();
        let mut a = make_entry("a1", "proj", "Active lesson", "active body");
        a.source_session_id = Some("sess-99".into());
        let mut b = make_entry("a2", "proj", "Superseded lesson", "old body");
        b.source_session_id = Some("sess-99".into());
        let mut c = make_entry("a3", "proj", "Other session lesson", "other body");
        c.source_session_id = Some("sess-other".into());
        add_entry(&db.conn, &a).unwrap();
        add_entry(&db.conn, &b).unwrap();
        add_entry(&db.conn, &c).unwrap();
        // Force a2 into 'superseded' directly (add_entry inserts as 'active').
        db.conn
            .execute("UPDATE knowledge_entries SET status='superseded' WHERE id='a2'", [])
            .unwrap();

        let n = bump_effectiveness_by_session(&db.conn, "sess-99").unwrap();
        assert_eq!(n, 1, "only the active sess-99 row (a1)");

        let all = get_entries_for_project(&db.conn, "proj").unwrap();
        let by_id = |id: &str| {
            all.iter().find(|e| e.id == id).unwrap_or_else(|| panic!("missing {id}"))
        };
        assert_eq!(by_id("a1").effectiveness as i64, 1, "active sess-99 row bumped");
        assert_eq!(by_id("a2").effectiveness as i64, 0, "superseded row untouched");
        assert_eq!(by_id("a3").effectiveness as i64, 0, "other-session row untouched");
    }

    #[test]
    fn encode_decode_embedding_roundtrip() {
        let v = vec![0.1_f32, -0.2, 0.3, 1.5, 0.0];
        let bytes = encode_embedding(&v);
        assert_eq!(bytes.len(), v.len() * 4);
        let decoded = decode_embedding(&bytes);
        assert_eq!(decoded.len(), v.len());
        for (a, b) in v.iter().zip(&decoded) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
    }

    #[test]
    fn decode_embedding_ignores_trailing_garbage() {
        // corrupt row (length not a multiple of 4) → chunks_exact drops the tail,
        // never panics. A bad row is silently shortened at retrieval, not crashed.
        let v = decode_embedding(&[0u8, 0, 0, 0, 99]);
        assert_eq!(v, vec![0.0]);
    }

    #[test]
    fn upsert_and_get_active_embeddings_filters_status_and_model() {
        let db = TempDb::new();
        let mut e1 = make_entry("em1", "proj", "Title 1", "content one");
        e1.source_session_id = Some("sess".into());
        let mut e2 = make_entry("em2", "proj", "Title 2", "content two");
        e2.source_session_id = Some("sess".into());
        add_entry(&db.conn, &e1).unwrap();
        add_entry(&db.conn, &e2).unwrap();
        // Force em2 superseded: get_active_embeddings must drop it.
        db.conn
            .execute("UPDATE knowledge_entries SET status='superseded' WHERE id='em2'", [])
            .unwrap();

        upsert_embedding(&db.conn, "em1", &[1.0, 0.0], "test-embed").unwrap();
        upsert_embedding(&db.conn, "em2", &[0.0, 1.0], "test-embed").unwrap();

        let got = get_active_embeddings(&db.conn, "proj", "test-embed").unwrap();
        assert_eq!(got.len(), 1, "only active em1: {got:?}");
        assert_eq!(got[0].0, "em1");
        assert_eq!(got[0].1, vec![1.0, 0.0]);

        // Different model id → empty (stale rows ignored on a model swap).
        let stale = get_active_embeddings(&db.conn, "proj", "other-model").unwrap();
        assert!(stale.is_empty(), "model swap ignores stale rows");
    }

    #[test]
    fn upsert_embedding_overwrites_on_conflict() {
        let db = TempDb::new();
        let e = make_entry("ov1", "proj", "Title", "content");
        add_entry(&db.conn, &e).unwrap();
        upsert_embedding(&db.conn, "ov1", &[1.0, 0.0], "m1").unwrap();
        upsert_embedding(&db.conn, "ov1", &[0.0, 1.0], "m2").unwrap();
        let got = get_active_embeddings(&db.conn, "proj", "m2").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1, vec![0.0, 1.0], "second upsert overwrites");
    }

    #[test]
    fn entries_by_session_returns_active_only() {
        let db = TempDb::new();
        let mut a = make_entry("s1", "proj", "Active", "active body");
        a.source_session_id = Some("sess-x".into());
        let mut b = make_entry("s2", "proj", "Old", "old body");
        b.source_session_id = Some("sess-x".into());
        add_entry(&db.conn, &a).unwrap();
        add_entry(&db.conn, &b).unwrap();
        db.conn
            .execute("UPDATE knowledge_entries SET status='superseded' WHERE id='s2'", [])
            .unwrap();

        let rows = entries_by_session(&db.conn, "sess-x").unwrap();
        let ids: Vec<&str> = rows.iter().map(|(id, _, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["s1"], "only active sess-x row: {ids:?}");
        // tuple shape (id, title, content)
        assert_eq!(rows[0].1, "Active");
        assert_eq!(rows[0].2, "active body");
    }
}
