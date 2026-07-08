//! Unified memory-retrieval entry point (D1/D2/D4 + I4).
//!
//! 消除 kernel path（executor.rs `memory_prompt_suffix` — 全表加载、按
//! confidence+recency 排序）与 opaque path（injector.rs `inject_for_agent` —
//! FTS+cross_project+decay）的双路径分裂（D2）。两条路径现在共用
//! [`retrieve_relevant`]，只在"装填预算 / 渲染格式"上各自保持语义（kernel
//! 是 sys_prompt token 预算，opaque 是 user-prompt char 预算——不宜在此绑死）。
//!
//! - D1: kernel path 不再全表加载；与 opaque 同样走 FTS5 bm25 关键词检索。
//! - D4: 排序统一加 [`decay_factor`] 软加权（kernel path 原本无 decay）。
//! - I4: 只注入 `status='active'`（SQL 层过滤；pending/superseded 不注入）。
//! - 续聊隔离: `is_continuation=true` 排除其他会话的 `react_session`/
//!   `react_reflection`（保 `memory_prompt_suffix` 原行为，防互串回归）。
//!
//! 返回 effective_score DESC 排序的候选列表；budget 装填、渲染、access_count+1
//! （I5）、effectiveness 反馈（I5 反馈环）由调用方按各自语义处理。

use crate::models::KnowledgeEntry;
use kernel_core::EmbedModel;

/// Knowledge decay constants (days). Within `DECAY_START_DAYS` → 1.0 (no decay);
/// after `DECAY_END_DAYS` → 0.0 (expired); between → linear.
pub(crate) const DECAY_START_DAYS: i64 = 30;
pub(crate) const DECAY_END_DAYS: i64 = 90;

/// 跨项目补全的 confidence 门槛（比项目内更严：cross 记忆要先过这道关，
/// 与调用方传入的项目内 confidence_min 无关——cross 永远 ≥0.6）。
const CROSS_PROJECT_CONFIDENCE_MIN: f64 = 0.6;
/// 跨项目补全条数上限（opaque path 既有行为）。
const MAX_CROSS_PROJECT_ENTRIES: usize = 2;
/// 项目内 FTS 命中条数低于此值时用全表兜底（保 kernel path 旧行为：FTS 漏召 /
/// 无关键词时仍注入项目记忆，而非返回空）。
const FALLBACK_TRIGGER: usize = 5;
/// I1 向量 fallback 触发阈值：FTS 命中数低于此值才 embed query 走向量补全。
/// 与 FALLBACK_TRIGGER（全表兜底）解耦——向量补强的是"FTS 召回了但语义漏"
/// （同义/改述），全表兜底保的是"FTS 完全没召回"，两道独立闸，阈值不同。
pub(crate) const VECTOR_FALLBACK_TRIGGER: usize = 3;
/// I1 cosine 下限：低于此相似度的向量候选丢弃，避免灌入弱相关噪声稀释 FTS
/// 命中。embedding 模型 cosine 通常 [0,1]，0.30 是"明显相关"的经验下界。
const VECTOR_MIN_COSINE: f64 = 0.30;

/// 续聊隔离：排除其他会话的完整产出（防 agent 误把他人产出当自己的 history）。
const CONTINUATION_EXCLUDE_CATEGORIES: &[&str] = &["react_session", "react_reflection"];

/// Compute a time-based decay factor for a knowledge entry.
///
/// - Within `DECAY_START_DAYS` (30): returns 1.0 (no decay)
/// - After `DECAY_END_DAYS` (90): returns 0.0 (fully expired)
/// - Between: linear interpolation from 1.0 to 0.0
pub(crate) fn decay_factor(updated_at: &str) -> f64 {
    let updated = match chrono::DateTime::parse_from_rfc3339(updated_at) {
        Ok(dt) => dt.with_timezone(&chrono::Local),
        Err(_) => return 1.0, // unparseable → treat as recent
    };
    let now = chrono::Local::now();
    let age_days = (now - updated).num_days();

    if age_days <= DECAY_START_DAYS {
        1.0
    } else if age_days >= DECAY_END_DAYS {
        0.0
    } else {
        let range = (DECAY_END_DAYS - DECAY_START_DAYS) as f64;
        let elapsed = (age_days - DECAY_START_DAYS) as f64;
        1.0 - (elapsed / range)
    }
}

/// I5 reuse/effectiveness score boost. A memory that's been injected before
/// (`access_count`) and validated by a Completed session (`effectiveness`)
/// ranks higher — closing the feedback loop into retrieval ranking.
/// Logarithmic so the first reuse matters most and the curve plateaus; a
/// brand-new entry (access_count=0, effectiveness=0) returns 1.0 so it's never
/// penalized relative to the confidence×decay baseline.
pub(crate) fn reuse_boost(access_count: i64, effectiveness: f64) -> f64 {
    let reuse = (1.0 + access_count.max(0) as f64).ln();
    let eff = effectiveness.max(0.0);
    1.0 + 0.15 * reuse + 0.1 * eff
}

/// Cosine similarity between two vectors (I1). Returns 0.0 for empty or
/// mismatched-length inputs (treated as orthogonal — a bad/foreign-dim row never
/// crashes retrieval, just ranks last). Computed in f64 to avoid f32 underflow
/// on long vectors. Range [-1.0, 1.0]; normalized embeddings usually [0.0, 1.0].
pub(crate) fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        let x = *x as f64;
        let y = *y as f64;
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom == 0.0 {
        0.0
    } else {
        dot / denom
    }
}

/// Extract search keywords from a prompt for FTS5 queries.
///
/// CJK characters are split individually (no word boundaries in Chinese).
/// English words are kept whole. Stop words are filtered. Returns an OR-joined
/// FTS5 query for broad recall.
pub(crate) fn extract_keywords(prompt: &str) -> String {
    const STOP_WORDS: &[&str] = &[
        "the", "is", "at", "which", "on", "a", "an", "and", "or", "but",
        "in", "with", "to", "for", "of", "not", "no", "this", "that", "it",
        "from", "by", "as", "be", "was", "are", "been", "has", "have", "had",
        "do", "does", "did", "will", "would", "can", "could", "should", "may",
        "if", "then", "so", "than", "too", "very", "just", "about", "up",
        "out", "all", "its", "my", "your", "our", "their", "what", "when",
        "where", "how", "who", "which",
        // Chinese stop words
        "的", "了", "是", "在", "有", "和", "不", "这", "我", "你",
        "他", "她", "它", "们", "吗", "呢", "吧", "啊", "把", "被",
        "让", "给", "到", "也", "都", "还", "就", "又", "而", "但",
        "个", "上", "下", "中", "里", "看", "说", "做", "会", "能",
        "要", "用", "去", "来", "过", "着", "得", "地",
    ];

    let mut keywords = Vec::new();
    let mut current_alpha = String::new();

    for ch in prompt.chars() {
        let cp = ch as u32;
        let is_cjk = (0x4E00..=0x9FFF).contains(&cp) // CJK Unified Ideographs
            || (0x3400..=0x4DBF).contains(&cp) // CJK Extension A
            || (0x3000..=0x303F).contains(&cp); // CJK Symbols

        if is_cjk {
            // Flush accumulated alpha word
            if !current_alpha.is_empty() {
                keywords.push(current_alpha.clone());
                current_alpha.clear();
            }
            // Each CJK character is its own token
            keywords.push(ch.to_string());
        } else if ch.is_alphanumeric() {
            current_alpha.push(ch);
        } else {
            // Non-alphanumeric separator
            if !current_alpha.is_empty() {
                keywords.push(current_alpha.clone());
                current_alpha.clear();
            }
        }
    }
    if !current_alpha.is_empty() {
        keywords.push(current_alpha);
    }

    // Filter stop words and short tokens
    keywords.retain(|word| {
        let lower = word.to_lowercase();
        if STOP_WORDS.contains(&lower.as_str()) {
            return false;
        }
        // CJK single chars are valid keywords (already filtered stop words above)
        let is_cjk = word.chars().any(|c| {
            let cp = c as u32;
            (0x4E00..=0x9FFF).contains(&cp)
                || (0x3400..=0x4DBF).contains(&cp)
                || (0x3000..=0x303F).contains(&cp)
        });
        if is_cjk {
            return true;
        }
        word.len() >= 3
    });

    // Deduplicate
    let mut seen = std::collections::HashSet::new();
    keywords.retain(|w| seen.insert(w.to_lowercase()));

    // Take top 10, wrap each in quotes for safe FTS5 phrase matching
    keywords.truncate(10);
    if keywords.is_empty() {
        return String::new();
    }
    let quoted: Vec<String> = keywords
        .into_iter()
        .map(|w| format!("\"{}\"", w.replace('"', "")))
        .collect();
    quoted.join(" OR ")
}

/// 统一检索入口。
///
/// `query` 通常是当前 turn 的 prompt（提取 FTS 关键词）；空 query 时退回全表
/// 兜底，等价 kernel path 旧行为，保证无 prompt 时仍有项目记忆注入。
/// `is_continuation=true` 时排除其他会话的 `react_session`/`react_reflection`
/// （续聊隔离）。`exclude_categories` 额外按 category 排除——kernel path 用来
/// 把 `quality_failure` 让给 experience lane；opaque path 无独立 experience lane，
/// 传空切片。`confidence_min` 是项目内命中的置信门槛——kernel path 传 0.6
/// （长期记忆 flywheel），opaque path 传 0.5（FTS 命中即可）。
///
/// 返回 effective_score（confidence × decay）DESC 排序的候选列表。**未做 budget
/// 装填、未 bump access_count**——调用方按各自预算/渲染语义处理（kernel token
/// 预算 vs opaque char 预算），并调用 [`super::store::bump_access_counts`] 标记
/// 最终注入的条目（I5）。跨项目条目可由调用方用 `entry.project_hash !=
/// project_hash` 判定（opaque 渲染 `[Cross-project]` 标签用）。
pub fn retrieve_relevant(
    conn: &rusqlite::Connection,
    query: &str,
    project_hash: &str,
    is_continuation: bool,
    exclude_categories: &[&str],
    confidence_min: f64,
) -> Vec<KnowledgeEntry> {
    let keywords = extract_keywords(query);

    // Step 1: 项目内 FTS5 bm25 检索（status='active' 在 SQL 层过滤，confidence
    // 用调用方传入的门槛）。
    let mut candidates: Vec<KnowledgeEntry> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    if !keywords.is_empty() {
        if let Ok(relevant) = super::store::search_entries_for_project(
            conn,
            project_hash,
            &keywords,
            confidence_min,
            10,
        ) {
            for e in relevant {
                seen.insert(e.id.clone());
                candidates.push(e);
            }
        }
    }

    // Step 2: FTS 不足或 query 无关键词 → 全表兜底。保 kernel path 旧行为：
    // 无 prompt / FTS 漏召时仍注入项目记忆。get_entries_for_project 不过滤
    // status/confidence（供管理 UI 列全部），故在此内存过滤——I4 + 调用方门槛。
    if candidates.len() < FALLBACK_TRIGGER {
        if let Ok(all) = super::store::get_entries_for_project(conn, project_hash) {
            for e in all {
                // In-memory filter (get_entries_for_project returns ALL rows for
                // the管理 UI; status/confidence gates are I4 + caller threshold).
                // seen.insert doubles as the dedup gate and the sole mutable
                // borrow — a filter(|e| !seen.contains(..)) closure would hold
                // an immutable borrow across the loop body and clash with insert.
                if e.status != "active" || e.confidence < confidence_min {
                    continue;
                }
                if seen.insert(e.id.clone()) {
                    candidates.push(e);
                }
            }
        }
    }

    // Step 3: 跨项目补全（FTS 命中，confidence≥0.6，status='active' 在 SQL 过滤）。
    let project_count = candidates.len();
    if project_count < FALLBACK_TRIGGER && !keywords.is_empty() {
        let remaining = FALLBACK_TRIGGER
            .saturating_sub(project_count)
            .min(MAX_CROSS_PROJECT_ENTRIES);
        if let Ok(cross) = super::store::search_entries_cross_project(
            conn,
            project_hash,
            &keywords,
            CROSS_PROJECT_CONFIDENCE_MIN,
            remaining,
        ) {
            for e in cross {
                if seen.insert(e.id.clone()) {
                    candidates.push(e);
                }
            }
        }
    }

    if candidates.is_empty() {
        return Vec::new();
    }

    // Step 4: exclude_categories + 续聊隔离 + decay 软加权（D4）+ I5 复用
    // 反馈加权。effective_score = confidence × decay_factor × reuse_boost
    // (access_count, effectiveness)。decay=0（超 90 天）的条目丢弃（opaque
    // 既有行为）。
    let mut scored: Vec<(f64, KnowledgeEntry)> = candidates
        .into_iter()
        .filter(|e| !exclude_categories.contains(&e.category.as_str()))
        .filter(|e| {
            !is_continuation
                || !CONTINUATION_EXCLUDE_CATEGORIES.contains(&e.category.as_str())
        })
        .filter_map(|e| {
            let decay = decay_factor(&e.updated_at);
            if decay <= 0.0 {
                return None;
            }
            // I5: fold in reuse_boost so a re-injected + Completed-validated
            // memory outranks an equally fresh+confident but unproven one.
            Some((e.confidence * decay * reuse_boost(e.access_count, e.effectiveness), e))
        })
        .collect();

    // Step 5: effective_score DESC（confidence×decay）。FTS 的 bm25 相关性已在
    // Step 1/3 的 SQL ORDER BY 阶段决定了"哪些条目进入候选"，这里只做融合排序。
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    scored.into_iter().map(|(_, e)| e).collect()
}

/// 向量检索（I1 内部）：FTS 置信度不足时的语义 fallback。对项目内所有 active
/// entry 的 embedding 做 cosine，返回 top-k `(cosine, entry)`。`exclude_categories`
/// + 续聊隔离与 FTS 路径同语义。调用方保证 `embed_model` 与 query_emb 来自同一
/// 模型（retrieve 路径用 [`EmbedModel::embed_model_id`] 过滤存储行，保证可比）。
pub(crate) fn vector_search(
    conn: &rusqlite::Connection,
    query_emb: &[f32],
    project_hash: &str,
    embed_model: &str,
    exclude_categories: &[&str],
    is_continuation: bool,
    limit: usize,
) -> Vec<(f64, KnowledgeEntry)> {
    let pairs = match super::store::get_active_embeddings(conn, project_hash, embed_model) {
        Ok(p) if !p.is_empty() => p,
        _ => return Vec::new(),
    };
    // get_entries_for_project returns ALL rows (管理 UI 用)；内存过滤 active
    // 并建 id→entry map，与 embedding 配对 cosine。一次查询拿全量，避免 N 次
    // get_entry_by_id 的往返。
    let by_id: std::collections::HashMap<String, KnowledgeEntry> = super::store::get_entries_for_project(conn, project_hash)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.status == "active")
        .map(|e| (e.id.clone(), e))
        .collect();

    let mut scored: Vec<(f64, KnowledgeEntry)> = pairs
        .into_iter()
        .filter_map(|(id, emb)| {
            let score = cosine_similarity(query_emb, &emb);
            if score < VECTOR_MIN_COSINE {
                return None;
            }
            by_id.get(&id).map(|e| (score, e.clone()))
        })
        .filter(|(_, e)| !exclude_categories.contains(&e.category.as_str()))
        .filter(|(_, e)| {
            !is_continuation
                || !CONTINUATION_EXCLUDE_CATEGORIES.contains(&e.category.as_str())
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    scored
}

/// FTS-first 向量 fallback 检索（I1）。先走 [`retrieve_relevant`]（FTS bm25）；
/// 只有 FTS 召回不足（命中数 < `VECTOR_FALLBACK_TRIGGER`）且提供了 embedder 时，
/// 才 embed query 并 [`vector_search`] 补回语义相关但关键词漏召的条目。
///
/// `embedder=None` 时纯退化为 `retrieve_relevant`（Anthropic 协议无 embed API，
/// 或测试桩）——I1 是 opt-in，永不阻塞主检索路径。置信度判断用"命中条数"而
/// 非 bm25 分数：bm25 分跨 query 不可比（长度敏感），而 FTS 命中数 < 阈值是稳定
/// 的"召回不足"信号。向量补回的条目 append 在 FTS 结果之后——FTS 关键词命中通常
/// 更精确，应优先装填；向量只补语义洞。
pub async fn retrieve_relevant_with_vector(
    conn: &rusqlite::Connection,
    query: &str,
    project_hash: &str,
    is_continuation: bool,
    exclude_categories: &[&str],
    confidence_min: f64,
    embedder: Option<&dyn EmbedModel>,
) -> Vec<KnowledgeEntry> {
    let fts = retrieve_relevant(
        conn,
        query,
        project_hash,
        is_continuation,
        exclude_categories,
        confidence_min,
    );

    let embedder = match embedder {
        Some(e) => e,
        None => return fts, // I1 opt-out: no embedder → FTS-only path
    };
    // FTS already covered the query well → skip the embed round-trip entirely.
    if fts.len() >= VECTOR_FALLBACK_TRIGGER {
        return fts;
    }
    let model_id = embedder.embed_model_id();
    // Empty model id → a test stub (no real stored row matches ""). Skip the
    // storage lookup rather than always-empty-pass.
    if model_id.is_empty() {
        return fts;
    }
    let q_emb = match embedder.embed(&[query]).await {
        Ok(v) if v.len() == 1 => v.into_iter().next().unwrap(),
        _ => return fts, // embed failed → don't block retrieval, fall back to FTS
    };
    if q_emb.is_empty() {
        return fts;
    }
    let vec_results = vector_search(
        conn,
        &q_emb,
        project_hash,
        model_id,
        exclude_categories,
        is_continuation,
        VECTOR_FALLBACK_TRIGGER,
    );
    let mut seen: std::collections::HashSet<String> = fts.iter().map(|e| e.id.clone()).collect();
    let mut out = fts;
    for (_, e) in vec_results {
        if seen.insert(e.id.clone()) {
            out.push(e);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::models::{AgentType, KnowledgeEntry};

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

    fn make_entry(id: &str, project_hash: &str, category: &str, title: &str, content: &str) -> KnowledgeEntry {
        KnowledgeEntry {
            id: id.to_string(),
            project_hash: project_hash.to_string(),
            category: category.to_string(),
            title: title.to_string(),
            content: content.to_string(),
            source_agent: AgentType::ClaudeCode,
            source_session_id: None,
            source_type: "test".to_string(),
            confidence: 0.8,
            created_at: chrono::Local::now().to_rfc3339(),
            updated_at: chrono::Local::now().to_rfc3339(),
            access_count: 0,
            status: "active".to_string(),
            effectiveness: 0.0,
        }
    }

    #[test]
    fn test_extract_keywords_english() {
        let q = extract_keywords("Fix the rust async error handling");
        // stop words "the" filtered; "fix" is <3 chars dropped; remainder kept
        assert!(q.contains("\"rust\""), "q = {q}");
        assert!(q.contains("\"async\""), "q = {q}");
        assert!(q.contains("\"error\""), "q = {q}");
        assert!(q.contains("\"handling\""), "q = {q}");
    }

    #[test]
    fn test_extract_keywords_cjk() {
        let q = extract_keywords("修复这个模块的错误处理");
        // CJK split per-char; stop words 的/这/个 filtered; remainder kept
        assert!(q.contains("\"修\""), "q = {q}");
        assert!(q.contains("\"错\""), "q = {q}");
        assert!(!q.contains("\"的\""));
    }

    #[test]
    fn test_extract_keywords_empty() {
        assert_eq!(extract_keywords("the a an of"), "");
        assert_eq!(extract_keywords(""), "");
    }

    #[test]
    fn test_decay_factor_recent() {
        let now = chrono::Local::now().to_rfc3339();
        assert!((decay_factor(&now) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_decay_factor_expired() {
        let old = (chrono::Local::now() - chrono::Duration::days(120)).to_rfc3339();
        assert!((decay_factor(&old) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn test_decay_factor_mid_range() {
        // 60 days = midpoint of [30, 90] → 1.0 - (60-30)/(90-30) = 0.5. Locks
        // the linear-interpolation slope so a future refactor can't silently
        // flatten or invert the decay curve (the two endpoint tests above
        // alone wouldn't catch a step function or a reversed slope).
        let mid = (chrono::Local::now() - chrono::Duration::days(60)).to_rfc3339();
        let d = decay_factor(&mid);
        assert!((d - 0.5).abs() < 0.05, "60-day decay should be ~0.5, got {d}");
    }

    #[test]
    fn test_retrieve_relevant_fts_hit() {
        let db = TempDb::new();
        let e = make_entry(
            "r1",
            "proj_x",
            "insight",
            "Rust async patterns",
            "Use tokio spawn for concurrent tasks",
        );
        crate::knowledge::store::add_entry(&db.conn, &e).unwrap();

        let got = retrieve_relevant(&db.conn, "tokio async tasks", "proj_x", false, &[], 0.5);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "r1");
    }

    #[test]
    fn test_retrieve_relevant_excludes_inactive() {
        // I4: pending/superseded entries must NOT be injected.
        let db = TempDb::new();
        let mut pending = make_entry(
            "p1",
            "proj_y",
            "insight",
            "Pending lesson",
            "draft not yet validated",
        );
        pending.status = "pending".into();
        let mut superseded = make_entry(
            "s1",
            "proj_y",
            "insight",
            "Old lesson",
            "replaced by newer",
        );
        superseded.status = "superseded".into();
        let active = make_entry(
            "a1",
            "proj_y",
            "insight",
            "Active lesson",
            "current and valid",
        );
        crate::knowledge::store::add_entry(&db.conn, &pending).unwrap();
        crate::knowledge::store::add_entry(&db.conn, &superseded).unwrap();
        crate::knowledge::store::add_entry(&db.conn, &active).unwrap();

        let got = retrieve_relevant(&db.conn, "lesson", "proj_y", false, &[], 0.5);
        let ids: Vec<&str> = got.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"a1"), "active must be returned: {ids:?}");
        assert!(!ids.contains(&"p1"), "pending must be excluded: {ids:?}");
        assert!(!ids.contains(&"s1"), "superseded must be excluded: {ids:?}");
    }

    #[test]
    fn test_retrieve_relevant_continuation_isolation() {
        // 续聊隔离: is_continuation=true excludes react_session/react_reflection.
        let db = TempDb::new();
        let session = make_entry(
            "sess1",
            "proj_z",
            "react_session",
            "Other session output",
            "full session summary from another conversation",
        );
        let insight = make_entry(
            "i1",
            "proj_z",
            "insight",
            "Project insight",
            "general reusable knowledge",
        );
        crate::knowledge::store::add_entry(&db.conn, &session).unwrap();
        crate::knowledge::store::add_entry(&db.conn, &insight).unwrap();

        // Fresh turn (not continuation): both retrievable
        let fresh = retrieve_relevant(&db.conn, "session insight", "proj_z", false, &[], 0.5);
        let fresh_ids: Vec<&str> = fresh.iter().map(|e| e.id.as_str()).collect();
        assert!(fresh_ids.contains(&"sess1"), "fresh turn keeps react_session");

        // Continuation: react_session excluded
        let cont = retrieve_relevant(&db.conn, "session insight", "proj_z", true, &[], 0.5);
        let cont_ids: Vec<&str> = cont.iter().map(|e| e.id.as_str()).collect();
        assert!(
            !cont_ids.contains(&"sess1"),
            "continuation must exclude react_session: {cont_ids:?}"
        );
    }

    #[test]
    fn test_retrieve_relevant_exclude_categories() {
        // kernel path excludes quality_failure (experience lane).
        let db = TempDb::new();
        let mut qf = make_entry(
            "qf1",
            "proj_w",
            "quality_failure",
            "Test failure lesson",
            "cargo test failed on windows",
        );
        qf.confidence = 0.9; // would clear the threshold
        let insight = make_entry(
            "i2",
            "proj_w",
            "insight",
            "Build tip",
            "use cargo nextest",
        );
        crate::knowledge::store::add_entry(&db.conn, &qf).unwrap();
        crate::knowledge::store::add_entry(&db.conn, &insight).unwrap();

        let got = retrieve_relevant(&db.conn, "cargo test build", "proj_w", false, &["quality_failure"], 0.5);
        let ids: Vec<&str> = got.iter().map(|e| e.id.as_str()).collect();
        assert!(!ids.contains(&"qf1"), "quality_failure must be excluded: {ids:?}");
        assert!(ids.contains(&"i2"), "insight must remain: {ids:?}");
    }

    #[test]
    fn test_retrieve_relevant_empty_project() {
        let db = TempDb::new();
        let got = retrieve_relevant(&db.conn, "anything", "nonexistent", false, &[], 0.5);
        assert!(got.is_empty());
    }

    #[test]
    fn test_retrieve_relevant_no_keywords_falls_back() {
        // Empty query (no keywords) still returns project entries via 全表兜底,
        // preserving the kernel path's old behavior.
        let db = TempDb::new();
        let e = make_entry(
            "fb1",
            "proj_fb",
            "insight",
            "General tip",
            "always run tests",
        );
        crate::knowledge::store::add_entry(&db.conn, &e).unwrap();

        let got = retrieve_relevant(&db.conn, "", "proj_fb", false, &[], 0.5);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "fb1");
    }

    #[test]
    fn test_reuse_boost_new_entry_is_neutral() {
        // I5: a fresh entry (never injected, no effectiveness signal) must not
        // be penalized or inflated — reuse_boost == 1.0 keeps effective_score at
        // confidence × decay.
        assert!((reuse_boost(0, 0.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_reuse_boost_grows_with_reuse_and_effectiveness() {
        // Both injection frequency (access_count) and a successful-completion
        // signal (effectiveness) push reuse_boost above the neutral 1.0.
        let fresh = reuse_boost(0, 0.0);
        let reused = reuse_boost(5, 0.0);
        let validated = reuse_boost(0, 3.0);
        let both = reuse_boost(5, 3.0);
        assert!(reused > fresh, "reuse boosts: {reused} vs {fresh}");
        assert!(validated > fresh, "effectiveness boosts: {validated} vs {fresh}");
        assert!(both > reused, "reuse+effectiveness compounds: {both} vs {reused}");
    }

    #[test]
    fn test_reuse_boost_clamps_negatives() {
        // Bad data (negative counts/effectiveness) must never invert ranking.
        assert!(reuse_boost(-3, -1.0) >= 1.0);
    }

    #[test]
    fn test_cosine_similarity_basics() {
        // identical → 1.0
        let a = vec![1.0_f32, 0.0, 0.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-9);
        // orthogonal → 0.0
        assert!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-9);
        // mismatched length → 0.0, never panic
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 1.0]), 0.0);
        // empty → 0.0
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }

    #[test]
    fn test_vector_search_ranks_by_cosine_and_drops_low() {
        // I1: vector_search cosine-ranks stored embeddings, drops sub-MIN, and
        // respects the active-status gate baked into get_active_embeddings.
        let db = TempDb::new();
        let near = make_entry(
            "v1", "proj_v", "insight", "Near lesson", "matches query semantics",
        );
        let far = make_entry(
            "v2", "proj_v", "insight", "Far lesson", "unrelated content",
        );
        crate::knowledge::store::add_entry(&db.conn, &near).unwrap();
        crate::knowledge::store::add_entry(&db.conn, &far).unwrap();

        let q = vec![1.0_f32, 0.0];
        let near_emb = vec![0.9, 0.1]; // cosine ≈ 0.994 ≥ MIN
        let far_emb = vec![0.0, 1.0]; // cosine = 0.0 < MIN → dropped
        crate::knowledge::store::upsert_embedding(&db.conn, "v1", &near_emb, "test-embed")
            .unwrap();
        crate::knowledge::store::upsert_embedding(&db.conn, "v2", &far_emb, "test-embed")
            .unwrap();

        let got = vector_search(&db.conn, &q, "proj_v", "test-embed", &[], false, 5);
        let ids: Vec<&str> = got.iter().map(|(_, e)| e.id.as_str()).collect();
        assert!(ids.contains(&"v1"), "near entry must rank in: {ids:?}");
        assert!(!ids.contains(&"v2"), "far entry dropped (< MIN_COSINE): {ids:?}");
    }

    #[test]
    fn test_vector_search_excludes_categories() {
        // exclude_categories gates vector results the same way it gates FTS.
        let db = TempDb::new();
        let qf = make_entry("qf1", "proj_x", "quality_failure", "QF tip", "a failure lesson");
        crate::knowledge::store::add_entry(&db.conn, &qf).unwrap();
        let emb = vec![1.0_f32, 0.0];
        crate::knowledge::store::upsert_embedding(&db.conn, "qf1", &emb, "test-embed").unwrap();

        let got = vector_search(&db.conn, &emb, "proj_x", "test-embed", &["quality_failure"], false, 5);
        assert!(got.is_empty(), "quality_failure excluded from vector results");
    }

    // Returns the same fixed vector for every input text — enough to drive the
    // vector-fallback path (stored doc embedding == query embedding → cosine 1.0)
    // without modeling a real embedding model.
    struct SameVecEmbedder {
        vec: Vec<f32>,
        model: String,
    }
    #[async_trait::async_trait]
    impl EmbedModel for SameVecEmbedder {
        async fn embed(
            &self,
            texts: &[&str],
        ) -> Result<Vec<Vec<f32>>, kernel_core::Error> {
            Ok(texts.iter().map(|_| self.vec.clone()).collect())
        }
        fn embed_model_id(&self) -> &str {
            &self.model
        }
    }

    // An embedder that PANICS if embed() is called — used to prove the
    // VECTOR_FALLBACK_TRIGGER gate skips the embed round-trip when FTS is enough.
    struct PanicEmbedder;
    #[async_trait::async_trait]
    impl EmbedModel for PanicEmbedder {
        async fn embed(
            &self,
            _: &[&str],
        ) -> Result<Vec<Vec<f32>>, kernel_core::Error> {
            unreachable!("embed must not be called when FTS recall is sufficient")
        }
        fn embed_model_id(&self) -> &str {
            "panic"
        }
    }

    #[tokio::test]
    async fn test_retrieve_relevant_with_vector_fills_fts_gap() {
        // FTS漏召: query 关键词与 entry 不重合, 且 confidence_min 让 FTS 全过滤
        // (entry conf 0.8 < 0.9 门槛). 只有 vector fallback (mock 同向量→cosine 1)
        // 能补回 gap1. 证明 I1 的"FTS 置信不足→向量补"链路.
        let db = TempDb::new();
        let e = make_entry(
            "gap1", "proj_g", "insight", "Error handling",
            "handle errors gracefully in rust async",
        );
        crate::knowledge::store::add_entry(&db.conn, &e).unwrap();
        let emb = vec![0.7_f32, 0.7];
        crate::knowledge::store::upsert_embedding(&db.conn, "gap1", &emb, "test-embed")
            .unwrap();

        let embedder = SameVecEmbedder { vec: emb.clone(), model: "test-embed".into() };
        let got = retrieve_relevant_with_vector(
            &db.conn, "exception processing", "proj_g", false, &[], 0.9, Some(&embedder),
        )
        .await;
        let ids: Vec<&str> = got.iter().map(|e| e.id.as_str()).collect();
        assert!(
            ids.contains(&"gap1"),
            "vector fallback must recover the FTS gap: {ids:?}"
        );
    }

    #[tokio::test]
    async fn test_retrieve_relevant_with_vector_skips_embed_when_fts_sufficient() {
        // FTS 命中 ≥ VECTOR_FALLBACK_TRIGGER → embed 必须不被调用 (PanicEmbedder
        // 若被调则 unreachable! 触发), 直接返回 FTS 结果.
        let db = TempDb::new();
        for i in 0..5 {
            let e = make_entry(
                &format!("f{i}"), "proj_f", "insight", &format!("Tip {i}"),
                "shared keyword rust async",
            );
            crate::knowledge::store::add_entry(&db.conn, &e).unwrap();
        }
        let embedder = PanicEmbedder;
        let got = retrieve_relevant_with_vector(
            &db.conn, "rust async", "proj_f", false, &[], 0.5, Some(&embedder),
        )
        .await;
        assert!(got.len() >= 3, "FTS sufficient, returned {got:?}");
    }

    #[tokio::test]
    async fn test_retrieve_relevant_with_vector_no_embedder_degrades_to_fts() {
        // embedder=None (Anthropic protocol, no embed API) → pure FTS, no panic.
        let db = TempDb::new();
        let e = make_entry("n1", "proj_n", "insight", "Tip", "rust async tip");
        crate::knowledge::store::add_entry(&db.conn, &e).unwrap();
        let got = retrieve_relevant_with_vector(
            &db.conn, "rust", "proj_n", false, &[], 0.5, None,
        )
        .await;
        let ids: Vec<&str> = got.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"n1"), "FTS-only still works without embedder");
    }
}
