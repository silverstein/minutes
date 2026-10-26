//! Semantic search over meetings: keeping the vector index in step with the
//! corpus, and answering a query from it.
//!
//! Sync embeds meetings whose content changed since they were indexed and
//! drops meetings that were deleted, became restricted, or no longer parse.
//! A query uses only vectors computed from a meeting's current content (see
//! [`crate::semantic_index`]) and re-reads every hit through the authorized
//! corpus revision before returning it.
use crate::config::Config;
use crate::error::SearchError;
use crate::markdown::{
    read_stable_active_markdown, split_frontmatter, stable_active_corpus_revision_with_budget,
    ActiveCorpusReadBudget, Frontmatter, Sensitivity,
};
use crate::search::{SearchFilters, SearchResult};
#[cfg(test)]
use crate::semantic::EmbedWorker;
use crate::semantic::{Embedder, SemanticError, DIMENSIONS};
use crate::semantic_index::{quantize, SemanticIndex, StoredChunk};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Names the model behind stored vectors; changing it wipes the index.
pub const MODEL_ID: &str = "embeddinggemma-2-q8_0@031f0d4b";
const CHUNK_CHARS: usize = 1_200;
const CHUNK_OVERLAP: usize = 200;
const MIN_CHUNK_CHARS: usize = 40;
const EMBED_BATCH: usize = 32;
const SNIPPET_CHARS: usize = 320;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncStats {
    pub meetings: usize,
    pub embedded: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub chunks: usize,
}

impl From<SemanticError> for SearchError {
    fn from(error: SemanticError) -> Self {
        SearchError::Io(std::io::Error::other(error.to_string()))
    }
}

impl From<crate::semantic_index::IndexError> for SearchError {
    fn from(error: crate::semantic_index::IndexError) -> Self {
        SearchError::Io(std::io::Error::other(error.to_string()))
    }
}

/// Byte ranges of overlapping, character-aligned chunks of `body`.
pub fn chunk_ranges(body: &str) -> Vec<(usize, usize)> {
    let boundaries: Vec<usize> = body
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(body.len()))
        .collect();
    let chars = boundaries.len() - 1;
    let mut ranges = Vec::new();
    let mut start = 0;
    while start < chars {
        let end = (start + CHUNK_CHARS).min(chars);
        let (from, to) = (boundaries[start], boundaries[end]);
        if body[from..to]
            .chars()
            .filter(|c| !c.is_whitespace())
            .count()
            >= MIN_CHUNK_CHARS
        {
            ranges.push((from, to));
        }
        if end == chars {
            break;
        }
        start += CHUNK_CHARS - CHUNK_OVERLAP;
    }
    ranges
}

/// A meeting is indexable when its frontmatter parses and it is not
/// restricted. Anything else is removed from the index.
fn indexable(content: &str) -> Option<(Frontmatter, &str)> {
    let (frontmatter, body) = split_frontmatter(content);
    if frontmatter.is_empty() {
        return None;
    }
    let parsed: Frontmatter = serde_yaml::from_str(frontmatter).ok()?;
    if parsed.sensitivity == Some(Sensitivity::Restricted) {
        return None;
    }
    Some((parsed, body))
}

/// Bring the index in step with the corpus. `progress` gets (meetings
/// checked, meetings total) after each meeting and returns whether to keep
/// going; stopping early leaves every meeting either fully indexed or not.
pub fn sync(
    config: &Config,
    worker: &mut impl Embedder,
    mut progress: impl FnMut(usize, usize) -> bool,
) -> Result<SyncStats, SearchError> {
    let revision = stable_active_corpus_revision_with_budget(
        &config.output_dir,
        ActiveCorpusReadBudget::new(),
    )
    .map_err(|_| SearchError::Io(std::io::Error::other("meeting corpus could not be listed")))?;
    let mut index = SemanticIndex::open(&Config::minutes_dir(), MODEL_ID, DIMENSIONS)?;
    let indexed = index.indexed()?;
    let mut stats = SyncStats::default();

    for path in indexed.keys() {
        if revision.attested_sha256(path).is_none() {
            index.remove(path)?;
            stats.removed += 1;
        }
    }

    let paths: Vec<PathBuf> = revision.paths().map(Path::to_path_buf).collect();
    stats.meetings = paths.len();
    for (done, path) in paths.iter().enumerate() {
        let attested = revision.attested_sha256(path);
        if attested.is_some() && indexed.get(path) == attested.as_ref() {
            stats.unchanged += 1;
            if !progress(done + 1, paths.len()) {
                break;
            }
            continue;
        }
        let Some(snapshot) = read_stable_active_markdown(path, revision.canonical_root()) else {
            index.remove(path)?;
            if !progress(done + 1, paths.len()) {
                break;
            }
            continue;
        };
        let Some((frontmatter, body)) = indexable(&snapshot.content) else {
            if indexed.contains_key(path) {
                stats.removed += 1;
            }
            index.remove(path)?;
            if !progress(done + 1, paths.len()) {
                break;
            }
            continue;
        };
        let ranges = chunk_ranges(body);
        let mut chunks = Vec::with_capacity(ranges.len());
        for batch in ranges.chunks(EMBED_BATCH) {
            let documents: Vec<(&str, &str)> = batch
                .iter()
                .map(|(start, end)| (frontmatter.title.as_str(), &body[*start..*end]))
                .collect();
            let vectors = worker.embed_documents(&documents)?;
            for ((start, end), vector) in batch.iter().zip(vectors) {
                chunks.push(StoredChunk {
                    start: *start,
                    end: *end,
                    vector: quantize(&vector),
                });
            }
        }
        stats.chunks += chunks.len();
        index.upsert(path, &snapshot.content_sha256, &frontmatter.title, &chunks)?;
        stats.embedded += 1;
        if !progress(done + 1, paths.len()) {
            break;
        }
    }
    Ok(stats)
}

/// Meetings related to `query` by meaning, best first, at most `limit`,
/// each with its cosine similarity (best chunk) so callers can apply a floor.
pub fn search(
    config: &Config,
    worker: &mut impl Embedder,
    query: &str,
    filters: &SearchFilters,
    limit: usize,
) -> Result<Vec<(SearchResult, f32)>, SearchError> {
    let query = query.trim();
    if query.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let index = SemanticIndex::open(&Config::minutes_dir(), MODEL_ID, DIMENSIONS)?;
    let embedded = worker.embed_query(query)?;
    crate::search::with_stable_active_corpus(&config.output_dir, |revision| {
        let chunks = index.current_chunks(|path| revision.attested_sha256(path))?;
        // Best chunk per meeting.
        let mut best: HashMap<&Path, (f32, &StoredChunk)> = HashMap::new();
        for (path, _, chunk) in &chunks {
            let score = chunk
                .vector
                .iter()
                .zip(&embedded)
                .map(|(stored, q)| f32::from(*stored) * q)
                .sum::<f32>()
                / 127.0;
            let entry = best.entry(path.as_path()).or_insert((f32::MIN, chunk));
            if score > entry.0 {
                *entry = (score, chunk);
            }
        }
        let mut ranked: Vec<_> = best.into_iter().collect();
        ranked.sort_by(|a, b| b.1 .0.total_cmp(&a.1 .0).then_with(|| a.0.cmp(b.0)));

        let mut results = Vec::new();
        for (path, (score, chunk)) in ranked {
            if results.len() == limit {
                break;
            }
            let snapshot = revision.read_snapshot(path).map_err(|_| {
                SearchError::Io(std::io::Error::other(
                    "meeting corpus changed during search",
                ))
            })?;
            if let Some(result) = verified_hit(&snapshot.path, &snapshot.content, chunk, filters) {
                results.push((result, score));
            }
        }
        Ok(results)
    })
}

/// The live-snapshot gate for a semantic hit: still indexable (parses, not
/// restricted), passes the caller's filters, and the chunk still lies inside
/// the body it was embedded from.
fn verified_hit(
    path: &Path,
    content: &str,
    chunk: &StoredChunk,
    filters: &SearchFilters,
) -> Option<SearchResult> {
    let (frontmatter, body) = indexable(content)?;
    if filters
        .content_type
        .as_ref()
        .is_some_and(|expected| frontmatter.r#type.as_str() != expected)
    {
        return None;
    }
    let date = frontmatter.date.to_rfc3339();
    if filters.since.as_ref().is_some_and(|since| date < *since) {
        return None;
    }
    let excerpt = body.get(chunk.start..chunk.end)?;
    let mut snippet: String = excerpt.split_whitespace().collect::<Vec<_>>().join(" ");
    if snippet.chars().count() > SNIPPET_CHARS {
        snippet = snippet.chars().take(SNIPPET_CHARS).collect::<String>() + "…";
    }
    Some(SearchResult {
        path: path.to_path_buf(),
        title: frontmatter.title,
        date,
        content_type: frontmatter.r#type.as_str().into(),
        snippet,
        matched_via_alias: None,
        evidence: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real worker and model, temporary corpus:
    /// `MINUTES_EMBED_WORKER=<binary> MINUTES_EMBED_MODEL=<gguf> cargo test
    /// -p minutes-core --lib semantic_search::tests::real -- --ignored`.
    #[test]
    #[ignore]
    fn real_sync_and_search_follow_content_and_sensitivity() {
        let _guard = crate::test_support::home_env_lock();
        let data = tempfile::tempdir().unwrap();
        let meetings = tempfile::tempdir().unwrap();
        // SAFETY: serialized by the HOME/env lock above.
        unsafe { std::env::set_var("MINUTES_DATA_DIR", data.path()) };
        let write = |name: &str, title: &str, extra: &str, body: &str| {
            std::fs::write(
                meetings.path().join(name),
                format!(
                    "---\ntitle: {title}\ntype: meeting\ndate: 2026-10-01\n{extra}---\n\n{body}"
                ),
            )
            .unwrap();
        };
        let filler = " We talked through the plan in detail and agreed on next steps.".repeat(4);
        write(
            "payroll.md",
            "Ops sync",
            "",
            &format!("We decided to move payroll from Gusto to Mercury.{filler}"),
        );
        write(
            "pills.md",
            "Ad ideas",
            "",
            &format!("Brainstorm about an ad with a broken pill container.{filler}"),
        );
        write(
            "secret.md",
            "Board",
            "sensitivity: restricted\n",
            &format!("We will switch payroll providers next quarter.{filler}"),
        );
        let config = Config {
            output_dir: meetings.path().to_path_buf(),
            ..Config::default()
        };
        let worker = PathBuf::from(std::env::var_os("MINUTES_EMBED_WORKER").unwrap());
        let model = PathBuf::from(std::env::var_os("MINUTES_EMBED_MODEL").unwrap());
        let mut worker = EmbedWorker::spawn(&worker, &model, DIMENSIONS).unwrap();
        let titles = |worker: &mut EmbedWorker| -> Vec<String> {
            search(
                &config,
                worker,
                "switching our payroll provider",
                &SearchFilters::default(),
                5,
            )
            .unwrap()
            .into_iter()
            .map(|(r, _)| r.title)
            .collect()
        };

        let stats = sync(&config, &mut worker, |_, _| true).unwrap();
        assert_eq!((stats.meetings, stats.embedded), (3, 2), "{stats:?}");
        assert_eq!(titles(&mut worker), vec!["Ops sync", "Ad ideas"]);

        // Edited after indexing: its old vectors are never used.
        write(
            "payroll.md",
            "Ops sync",
            "",
            &format!("Lunch order for Friday.{filler}"),
        );
        assert_eq!(titles(&mut worker), vec!["Ad ideas"]);
        // Restricted later: dropped at the next sync.
        write(
            "pills.md",
            "Ad ideas",
            "sensitivity: restricted\n",
            "Brainstorm about pills.",
        );
        let stats = sync(&config, &mut worker, |_, _| true).unwrap();
        assert_eq!(stats.removed, 1, "{stats:?}");
        assert_eq!(titles(&mut worker), vec!["Ops sync"]);
        unsafe { std::env::remove_var("MINUTES_DATA_DIR") };
    }

    #[test]
    fn chunks_overlap_respect_char_boundaries_and_skip_blank_tails() {
        let body = "é".repeat(2_500);
        let ranges = chunk_ranges(&body);
        assert_eq!(ranges.len(), 3);
        for (start, end) in &ranges {
            assert!(body.is_char_boundary(*start) && body.is_char_boundary(*end));
        }
        // Consecutive chunks overlap by CHUNK_OVERLAP characters.
        assert_eq!(
            body[ranges[1].0..ranges[0].1].chars().count(),
            CHUNK_OVERLAP
        );
        assert!(chunk_ranges("short").is_empty());
        assert!(chunk_ranges(&" ".repeat(3_000)).is_empty());
    }

    #[test]
    fn restricted_and_unparseable_meetings_are_not_indexable() {
        let normal = "---\ntitle: A\ntype: meeting\ndate: 2026-10-01\n---\n\nbody";
        let restricted =
            "---\ntitle: A\ntype: meeting\ndate: 2026-10-01\nsensitivity: restricted\n---\n\nbody";
        assert!(indexable(normal).is_some());
        assert!(indexable(restricted).is_none());
        assert!(indexable("no frontmatter").is_none());
        assert!(indexable("---\ntitle: [\n---\nx").is_none());
    }

    #[test]
    fn a_hit_is_rechecked_against_filters_and_the_live_body() {
        let content = "---\ntitle: Payroll\ntype: memo\ndate: 2026-10-01\n---\n\nWe   moved\npayroll to Mercury.";
        let body = split_frontmatter(content).1;
        let chunk = StoredChunk {
            start: 0,
            end: body.len(),
            vector: vec![],
        };
        let path = Path::new("/m/p.md");
        let hit = verified_hit(path, content, &chunk, &SearchFilters::default()).unwrap();
        assert_eq!(hit.title, "Payroll");
        assert_eq!(hit.snippet, "We moved payroll to Mercury.");
        let meetings_only = SearchFilters {
            content_type: Some("meeting".into()),
            ..SearchFilters::default()
        };
        assert!(verified_hit(path, content, &chunk, &meetings_only).is_none());
        let out_of_range = StoredChunk {
            start: 0,
            end: body.len() + 50,
            vector: vec![],
        };
        assert!(verified_hit(path, content, &out_of_range, &SearchFilters::default()).is_none());
    }
}
