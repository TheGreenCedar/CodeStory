use anyhow::{Context, Result, anyhow, bail};
use codestory_store::FileRole;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tantivy::collector::TopDocs;
use tantivy::doc;
use tantivy::query::QueryParser;
use tantivy::schema::{INDEXED, STORED, Schema, TEXT, Value};
use tantivy::{Index, IndexReader, ReloadPolicy, TantivyDocument};

const LEXICAL_INDEX_FILE: &str = "lexical-index.jsonl";
const CANDIDATE_DIR_PREFIX: &str = "codestory-lexical-tantivy-";
const DEFAULT_TOP_K: usize = 10;
const DEFAULT_REPEATS: usize = 7;
const DEFAULT_PREFILTER_LIMIT: usize = 2048;

fn main() -> Result<()> {
    let opts = Options::parse(std::env::args().skip(1).collect())?;
    if opts.self_test {
        return run_self_test();
    }
    let report = run_compare(&opts)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

#[derive(Debug)]
struct Options {
    shard_dir: PathBuf,
    queries: Vec<String>,
    top_k: usize,
    repeats: usize,
    self_test: bool,
}

impl Options {
    fn parse(args: Vec<String>) -> Result<Self> {
        if args.iter().any(|arg| arg == "--help" || arg == "-h") {
            print_help();
            std::process::exit(0);
        }
        let self_test = args.iter().any(|arg| arg == "--self-test");
        if self_test {
            return Ok(Self {
                shard_dir: PathBuf::new(),
                queries: Vec::new(),
                top_k: DEFAULT_TOP_K,
                repeats: DEFAULT_REPEATS,
                self_test,
            });
        }

        let mut shard_dir = None;
        let mut queries = Vec::new();
        let mut top_k = DEFAULT_TOP_K;
        let mut repeats = DEFAULT_REPEATS;
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--shard-dir" => {
                    i += 1;
                    shard_dir = args.get(i).map(PathBuf::from);
                }
                "--query" => {
                    i += 1;
                    queries.push(required_value(&args, i, "--query")?.to_string());
                }
                "--queries" => {
                    i += 1;
                    queries.extend(
                        required_value(&args, i, "--queries")?
                            .split('|')
                            .map(str::trim)
                            .filter(|query| !query.is_empty())
                            .map(str::to_string),
                    );
                }
                "--top-k" => {
                    i += 1;
                    top_k = required_value(&args, i, "--top-k")?.parse()?;
                }
                "--repeats" => {
                    i += 1;
                    repeats = required_value(&args, i, "--repeats")?.parse()?;
                }
                other => bail!("unknown argument `{other}`"),
            }
            i += 1;
        }

        let shard_dir = shard_dir.ok_or_else(|| anyhow!("--shard-dir is required"))?;
        if queries.is_empty() {
            queries = vec![
                "retrieval manifest freshness".into(),
                "symbol_search_doc component_report".into(),
                "lexical_source zoekt client".into(),
            ];
        }
        Ok(Self {
            shard_dir,
            queries,
            top_k,
            repeats,
            self_test: false,
        })
    }
}

fn required_value<'a>(args: &'a [String], index: usize, flag: &str) -> Result<&'a str> {
    args.get(index)
        .map(String::as_str)
        .ok_or_else(|| anyhow!("{flag} requires a value"))
}

fn generated_candidate_dir() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "{CANDIDATE_DIR_PREFIX}{}-{stamp}",
        std::process::id()
    ))
}

fn remove_generated_temp_dir(path: &Path) -> Result<()> {
    let temp_dir = std::env::temp_dir().canonicalize()?;
    let path = path.canonicalize()?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("candidate dir has no valid file name: {}", path.display()))?;
    if !path.starts_with(&temp_dir) || !name.starts_with(CANDIDATE_DIR_PREFIX) {
        bail!(
            "refusing to remove non-generated temp dir: {}",
            path.display()
        );
    }
    fs::remove_dir_all(&path)?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LexicalIndexEntry {
    path: String,
    content: String,
    #[serde(default)]
    source: LexicalDocumentSource,
    #[serde(default)]
    node_id: Option<String>,
    #[serde(default)]
    symbol_name: Option<String>,
    #[serde(default)]
    start_line: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum LexicalDocumentSource {
    #[default]
    LexicalSource,
    SymbolDoc,
    ComponentReport,
}

impl LexicalDocumentSource {
    fn provenance_label(self) -> &'static str {
        match self {
            Self::LexicalSource => "lexical_source",
            Self::SymbolDoc => "symbol_doc",
            Self::ComponentReport => "component_report",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct CompareReport {
    shard_dir: String,
    candidate_dir: String,
    jsonl_timing_scope: String,
    current_artifact_size_bytes: u64,
    candidate_artifact_size_bytes: u64,
    candidate_cold_build_index_ms: u128,
    entries: usize,
    top_k: usize,
    repeats: usize,
    missing_artifact_failure_behavior: String,
    candidate_strategy: String,
    candidate_prefilter_limit: usize,
    queries: Vec<QueryReport>,
    aggregate: AggregateReport,
}

#[derive(Debug, Clone, Serialize)]
struct QueryReport {
    query: String,
    jsonl_in_memory_ms_p50: f64,
    jsonl_in_memory_ms_p95: f64,
    tantivy_query_ms_p50: f64,
    tantivy_query_ms_p95: f64,
    top_k_overlap: usize,
    top_k_overlap_ratio: f64,
    unresolved_candidate_delta: isize,
    jsonl_hits: usize,
    tantivy_hits: usize,
    jsonl_provenance: Vec<String>,
    tantivy_provenance: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct AggregateReport {
    mean_top_k_overlap_ratio: f64,
    unresolved_candidate_delta: isize,
}

#[derive(Debug, Clone)]
struct SearchHit {
    path: String,
    source: LexicalDocumentSource,
    node_id: Option<String>,
    symbol_name: Option<String>,
    start_line: Option<u32>,
    score: f32,
    doc_id: Option<usize>,
}

fn run_compare(opts: &Options) -> Result<CompareReport> {
    let index_path = opts.shard_dir.join(LEXICAL_INDEX_FILE);
    if !index_path.is_file() {
        bail!(
            "missing lexical artifact: {}; fail-closed with no candidate comparison",
            index_path.display()
        );
    }

    let current_artifact_size_bytes = fs::metadata(&index_path)?.len();
    let entries = read_entries(&index_path)?;
    if entries.is_empty() {
        bail!("lexical artifact is empty: {}", index_path.display());
    }

    let candidate_dir = generated_candidate_dir();
    fs::create_dir(&candidate_dir).with_context(|| {
        format!(
            "create generated candidate dir {}; rerun if it already exists",
            candidate_dir.display()
        )
    })?;

    let started = Instant::now();
    let candidate = TantivyCandidate::build(&candidate_dir, &entries)?;
    let candidate_cold_build_index_ms = started.elapsed().as_millis();
    let candidate_artifact_size_bytes = dir_size_bytes(&candidate_dir)?;

    let mut query_reports = Vec::new();
    for query in &opts.queries {
        let _ = candidate.search_reranked(&entries, query, opts.top_k)?;
        let _ = jsonl_search(&entries, query, opts.top_k);

        let mut jsonl_times = Vec::new();
        let mut tantivy_times = Vec::new();
        let mut jsonl_hits = Vec::new();
        let mut tantivy_hits = Vec::new();
        for _ in 0..opts.repeats {
            let started = Instant::now();
            jsonl_hits = jsonl_search(&entries, query, opts.top_k);
            jsonl_times.push(started.elapsed().as_secs_f64() * 1000.0);

            let started = Instant::now();
            tantivy_hits = candidate.search_reranked(&entries, query, opts.top_k)?;
            tantivy_times.push(started.elapsed().as_secs_f64() * 1000.0);
        }

        query_reports.push(query_report(
            query,
            &jsonl_hits,
            &tantivy_hits,
            &jsonl_times,
            &tantivy_times,
            opts.top_k,
        ));
    }

    let aggregate = aggregate_report(&query_reports);
    Ok(CompareReport {
        shard_dir: opts.shard_dir.display().to_string(),
        candidate_dir: candidate_dir.display().to_string(),
        jsonl_timing_scope: "preloaded_in_memory_entries; production search also reads/parses lexical-index.jsonl per query".into(),
        current_artifact_size_bytes,
        candidate_artifact_size_bytes,
        candidate_cold_build_index_ms,
        entries: entries.len(),
        top_k: opts.top_k,
        repeats: opts.repeats,
        missing_artifact_failure_behavior:
            "missing lexical-index.jsonl returns an error before building the candidate index"
                .into(),
        candidate_strategy:
            "tantivy_or_prefilter_then_current_jsonl_score_rerank; diagnostic-only".into(),
        candidate_prefilter_limit: DEFAULT_PREFILTER_LIMIT,
        queries: query_reports,
        aggregate,
    })
}

fn read_entries(index_path: &Path) -> Result<Vec<LexicalIndexEntry>> {
    let file = File::open(index_path)?;
    let reader = BufReader::new(file);
    let mut entries = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let entry = serde_json::from_str::<LexicalIndexEntry>(&line)
            .with_context(|| format!("parse {} line {}", index_path.display(), index + 1))?;
        entries.push(entry);
    }
    Ok(entries)
}

struct TantivyCandidate {
    index: Index,
    reader: IndexReader,
    path_field: tantivy::schema::Field,
    source_field: tantivy::schema::Field,
    node_id_field: tantivy::schema::Field,
    symbol_name_field: tantivy::schema::Field,
    start_line_field: tantivy::schema::Field,
    doc_id_field: tantivy::schema::Field,
}

impl TantivyCandidate {
    fn build(index_dir: &Path, entries: &[LexicalIndexEntry]) -> Result<Self> {
        let mut schema = Schema::builder();
        let doc_id_field = schema.add_i64_field("doc_id", INDEXED | STORED);
        let path_field = schema.add_text_field("path", TEXT | STORED);
        let content_field = schema.add_text_field("content", TEXT);
        let source_field = schema.add_text_field("source", STORED);
        let node_id_field = schema.add_text_field("node_id", STORED);
        let symbol_name_field = schema.add_text_field("symbol_name", TEXT | STORED);
        let start_line_field = schema.add_i64_field("start_line", STORED);
        let index = Index::create_in_dir(index_dir, schema.build())?;
        let mut writer = index.writer(64_000_000)?;

        for (doc_id, entry) in entries.iter().enumerate() {
            writer.add_document(doc!(
                doc_id_field => doc_id as i64,
                path_field => entry.path.clone(),
                content_field => entry.content.clone(),
                source_field => entry.source.provenance_label(),
                node_id_field => entry.node_id.clone().unwrap_or_default(),
                symbol_name_field => entry.symbol_name.clone().unwrap_or_default(),
                start_line_field => i64::from(entry.start_line.unwrap_or(0)),
            ))?;
        }
        writer.commit()?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        Ok(Self {
            index,
            reader,
            path_field,
            source_field,
            node_id_field,
            symbol_name_field,
            start_line_field,
            doc_id_field,
        })
    }

    fn search_reranked(
        &self,
        entries: &[LexicalIndexEntry],
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let prefilter_hits = self.search_prefilter(query, DEFAULT_PREFILTER_LIMIT.max(limit))?;
        let allowed_doc_ids = prefilter_hits
            .iter()
            .filter_map(|hit| hit.doc_id)
            .collect::<HashSet<_>>();
        Ok(jsonl_search_filtered(
            entries,
            query,
            limit,
            Some(&allowed_doc_ids),
        ))
    }

    fn search_prefilter(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let query = sanitize_tantivy_query(query);
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let schema = self.index.schema();
        let query_parser = QueryParser::for_index(
            &self.index,
            vec![
                schema.get_field("path")?,
                schema.get_field("content")?,
                schema.get_field("symbol_name")?,
            ],
        );
        let query = query_parser.parse_query(&query)?;
        let searcher = self.reader.searcher();
        let docs = searcher.search(&query, &TopDocs::with_limit(limit).order_by_score())?;
        let mut hits = Vec::new();
        for (score, address) in docs {
            let doc: TantivyDocument = searcher.doc(address)?;
            let path = string_field(&doc, self.path_field).unwrap_or_default();
            let source = match string_field(&doc, self.source_field).as_deref() {
                Some("symbol_doc") => LexicalDocumentSource::SymbolDoc,
                Some("component_report") => LexicalDocumentSource::ComponentReport,
                _ => LexicalDocumentSource::LexicalSource,
            };
            hits.push(SearchHit {
                path,
                source,
                node_id: none_if_empty(string_field(&doc, self.node_id_field)),
                symbol_name: none_if_empty(string_field(&doc, self.symbol_name_field)),
                start_line: doc
                    .get_first(self.start_line_field)
                    .and_then(|value| value.as_i64())
                    .and_then(|value| u32::try_from(value).ok())
                    .filter(|value| *value > 0),
                score,
                doc_id: doc
                    .get_first(self.doc_id_field)
                    .and_then(|value| value.as_i64())
                    .and_then(|value| usize::try_from(value).ok()),
            });
        }
        Ok(hits)
    }
}

fn string_field(doc: &TantivyDocument, field: tantivy::schema::Field) -> Option<String> {
    doc.get_first(field)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn none_if_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

fn sanitize_tantivy_query(query: &str) -> String {
    query
        .split(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
        .filter(|token| token.len() >= 2)
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn jsonl_search(entries: &[LexicalIndexEntry], query: &str, limit: usize) -> Vec<SearchHit> {
    jsonl_search_filtered(entries, query, limit, None)
}

fn jsonl_search_filtered(
    entries: &[LexicalIndexEntry],
    query: &str,
    limit: usize,
    allowed_doc_ids: Option<&HashSet<usize>>,
) -> Vec<SearchHit> {
    let tokens = lexical_query_tokens(query);
    if tokens.is_empty() {
        return Vec::new();
    }
    let token_frequencies = token_document_frequencies(entries, &tokens);
    let command_tokens = command_query_tokens(query);
    let token_weights = token_frequencies
        .iter()
        .zip(tokens.iter())
        .map(|(frequency, token)| {
            let mut weight = lexical_token_weight(*frequency, entries.len());
            if command_tokens
                .iter()
                .any(|command_token| command_token == token)
            {
                weight *= 2.0;
            }
            weight
        })
        .collect::<Vec<_>>();
    let total_weight = token_weights.iter().sum::<f32>();
    let required_weight = required_lexical_match_weight(tokens.len(), total_weight);
    let mut hits = Vec::new();

    for (doc_id, entry) in entries.iter().enumerate() {
        if allowed_doc_ids.is_some_and(|allowed| !allowed.contains(&doc_id)) {
            continue;
        }
        let path_lower = entry.path.to_ascii_lowercase();
        let content_lower = entry.content.to_ascii_lowercase();
        let token_match = lexical_token_match(&tokens, &token_weights, &path_lower, &content_lower);
        if token_match.matched_weight >= required_weight
            && (tokens.len() < 8 || token_match.meaningful_path_weight > 0.0)
        {
            hits.push(SearchHit {
                path: entry.path.clone(),
                source: entry.source,
                node_id: entry.node_id.clone(),
                symbol_name: entry.symbol_name.clone(),
                start_line: entry.start_line,
                score: score_lexical_match(&entry.path, entry.source, &token_match),
                doc_id: Some(doc_id),
            });
        }
    }
    hits.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.path.cmp(&right.path))
    });
    hits.truncate(limit);
    hits
}

fn lexical_query_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for token in query
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|token| token.len() >= 2)
        .map(str::to_ascii_lowercase)
        .filter(|token| !LEXICAL_STOP_WORDS.contains(&token.as_str()))
    {
        if !tokens.iter().any(|existing| existing == &token) {
            tokens.push(token);
        }
    }
    tokens
}

fn command_query_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut in_backticks = false;
    let mut current = String::new();
    for ch in query.chars() {
        if ch == '`' {
            if in_backticks {
                push_command_tokens(&current, &mut tokens);
                current.clear();
            }
            in_backticks = !in_backticks;
            continue;
        }
        if in_backticks {
            current.push(ch);
        }
    }
    tokens
}

fn push_command_tokens(command: &str, tokens: &mut Vec<String>) {
    for token in command
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .map(|token| token.trim_start_matches('-').to_ascii_lowercase())
        .filter(|token| token.len() >= 2)
        .filter(|token| token != "codex")
    {
        if !tokens.iter().any(|existing| existing == &token) {
            tokens.push(token);
        }
    }
}

const LEXICAL_STOP_WORDS: &[&str] = &[
    "about", "after", "and", "are", "cite", "does", "explain", "file", "files", "flow", "flows",
    "for", "from", "how", "into", "level", "path", "source", "sources", "support", "that", "the",
    "through", "top", "what", "where", "which", "with",
];

fn token_document_frequencies(entries: &[LexicalIndexEntry], tokens: &[String]) -> Vec<usize> {
    tokens
        .iter()
        .map(|token| {
            entries
                .iter()
                .filter(|entry| {
                    entry.path.to_ascii_lowercase().contains(token)
                        || entry.content.to_ascii_lowercase().contains(token)
                })
                .count()
        })
        .collect()
}

fn lexical_token_weight(document_frequency: usize, document_count: usize) -> f32 {
    let rarity = ((document_count as f32 + 1.0) / (document_frequency as f32 + 1.0)).ln();
    (1.0 + rarity).clamp(0.25, 5.0)
}

fn required_lexical_match_weight(token_count: usize, total_weight: f32) -> f32 {
    if token_count <= 3 {
        return total_weight;
    }
    (total_weight * 0.28).max(2.5)
}

#[derive(Debug, Clone, Copy)]
struct LexicalTokenMatch {
    matched_weight: f32,
    path_weight: f32,
    content_weight: f32,
    total_weight: f32,
    meaningful_path_weight: f32,
}

fn lexical_token_match(
    tokens: &[String],
    token_weights: &[f32],
    path_lower: &str,
    content_lower: &str,
) -> LexicalTokenMatch {
    let mut matched_weight = 0.0;
    let mut path_weight = 0.0;
    let mut content_weight = 0.0;
    let mut total_weight = 0.0;
    let mut meaningful_path_weight = 0.0;
    for (token, weight) in tokens.iter().zip(token_weights.iter().copied()) {
        total_weight += weight;
        let path_factor = path_match_factor(path_lower, token);
        let path_match = path_factor > 0.0;
        let content_match = content_lower.contains(token);
        if path_match || content_match {
            matched_weight += weight;
        }
        if path_match {
            path_weight += weight * path_factor;
            if path_factor >= 1.0 && weight >= 1.5 {
                meaningful_path_weight += weight;
            }
        }
        if content_match {
            content_weight += weight;
        }
    }
    LexicalTokenMatch {
        matched_weight,
        path_weight,
        content_weight,
        total_weight,
        meaningful_path_weight,
    }
}

fn path_match_factor(path_lower: &str, token: &str) -> f32 {
    if path_lower.split('/').any(|segment| segment == token) {
        return 1.8;
    }
    if path_lower
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .flat_map(|part| part.split('_'))
        .any(|part| part == token)
    {
        return 1.0;
    }
    if path_lower.contains(token) {
        return 0.35;
    }
    0.0
}

fn score_lexical_match(
    path: &str,
    source: LexicalDocumentSource,
    token_match: &LexicalTokenMatch,
) -> f32 {
    let coverage = if token_match.total_weight <= 0.0 {
        0.0
    } else {
        token_match.matched_weight / token_match.total_weight
    };
    let mut score = 0.20_f32
        + coverage * 0.25
        + token_match.path_weight * 0.09
        + token_match.content_weight * 0.035;
    let path_lower = path.replace('\\', "/").to_ascii_lowercase();
    if path_lower.contains("/src/") || path_lower.starts_with("src/") {
        score += 0.04;
    }
    if source == LexicalDocumentSource::ComponentReport {
        score += 0.08;
    } else {
        score *= lexical_file_role_multiplier(FileRole::classify_path(Path::new(path)));
    }
    score.min(0.99)
}

fn lexical_file_role_multiplier(file_role: FileRole) -> f32 {
    match file_role {
        FileRole::Entrypoint => 1.08,
        FileRole::Source => 1.0,
        FileRole::Test => 0.68,
        FileRole::Docs => 0.72,
        FileRole::Benchmark => 0.64,
        FileRole::Generated => 0.55,
        FileRole::Vendor => 0.45,
    }
}

fn query_report(
    query: &str,
    jsonl_hits: &[SearchHit],
    tantivy_hits: &[SearchHit],
    jsonl_times: &[f64],
    tantivy_times: &[f64],
    top_k: usize,
) -> QueryReport {
    let jsonl_keys = jsonl_hits.iter().map(hit_key).collect::<HashSet<_>>();
    let tantivy_keys = tantivy_hits.iter().map(hit_key).collect::<HashSet<_>>();
    let overlap = jsonl_keys.intersection(&tantivy_keys).count();
    let unresolved_candidate_delta =
        unresolved_count(tantivy_hits) as isize - unresolved_count(jsonl_hits) as isize;
    QueryReport {
        query: query.to_string(),
        jsonl_in_memory_ms_p50: percentile(jsonl_times, 0.50),
        jsonl_in_memory_ms_p95: percentile(jsonl_times, 0.95),
        tantivy_query_ms_p50: percentile(tantivy_times, 0.50),
        tantivy_query_ms_p95: percentile(tantivy_times, 0.95),
        top_k_overlap: overlap,
        top_k_overlap_ratio: if top_k == 0 {
            0.0
        } else {
            overlap as f64 / top_k as f64
        },
        unresolved_candidate_delta,
        jsonl_hits: jsonl_hits.len(),
        tantivy_hits: tantivy_hits.len(),
        jsonl_provenance: provenance(jsonl_hits),
        tantivy_provenance: provenance(tantivy_hits),
    }
}

fn aggregate_report(reports: &[QueryReport]) -> AggregateReport {
    let mean_top_k_overlap_ratio = if reports.is_empty() {
        0.0
    } else {
        reports
            .iter()
            .map(|report| report.top_k_overlap_ratio)
            .sum::<f64>()
            / reports.len() as f64
    };
    AggregateReport {
        mean_top_k_overlap_ratio,
        unresolved_candidate_delta: reports
            .iter()
            .map(|report| report.unresolved_candidate_delta)
            .sum(),
    }
}

fn hit_key(hit: &SearchHit) -> String {
    format!(
        "{}|{}|{}|{}",
        hit.path,
        hit.node_id.as_deref().unwrap_or(""),
        hit.symbol_name.as_deref().unwrap_or(""),
        hit.start_line.unwrap_or(0)
    )
}

fn unresolved_count(hits: &[SearchHit]) -> usize {
    hits.iter().filter(|hit| hit.node_id.is_none()).count()
}

fn provenance(hits: &[SearchHit]) -> Vec<String> {
    let mut labels = Vec::new();
    for hit in hits {
        let label = hit.source.provenance_label().to_string();
        if !labels.iter().any(|existing| existing == &label) {
            labels.push(label);
        }
    }
    labels
}

fn percentile(values: &[f64], percentile: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    let index = ((sorted.len() as f64 * percentile).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[index]
}

fn dir_size_bytes(path: &Path) -> Result<u64> {
    let mut total = 0_u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            total += dir_size_bytes(&entry.path())?;
        } else {
            total += metadata.len();
        }
    }
    Ok(total)
}

fn run_self_test() -> Result<()> {
    let root = generated_candidate_dir().with_extension("self-test");
    if root.exists() {
        remove_generated_temp_dir(&root)?;
    }
    let shard = root.join("shard");
    fs::create_dir_all(&shard)?;
    let mut index = File::create(shard.join(LEXICAL_INDEX_FILE))?;
    writeln!(
        index,
        "{}",
        serde_json::to_string(&LexicalIndexEntry {
            path: "src/retrieval.rs".into(),
            content: "retrieval manifest freshness checks lexical_source".into(),
            source: LexicalDocumentSource::LexicalSource,
            node_id: None,
            symbol_name: None,
            start_line: None,
        })?
    )?;
    writeln!(
        index,
        "{}",
        serde_json::to_string(&LexicalIndexEntry {
            path: "codestory://component_report".into(),
            content: "component_report covers symbol_search_doc provenance".into(),
            source: LexicalDocumentSource::ComponentReport,
            node_id: Some("42".into()),
            symbol_name: Some("component_report:demo".into()),
            start_line: Some(7),
        })?
    )?;

    let report = run_compare(&Options {
        shard_dir: shard.clone(),
        queries: vec!["retrieval manifest".into(), "component_report".into()],
        top_k: 2,
        repeats: 2,
        self_test: false,
    })?;
    assert_eq!(report.entries, 2);
    assert_eq!(report.queries.len(), 2);
    assert!(report.candidate_artifact_size_bytes > 0);
    remove_generated_temp_dir(Path::new(&report.candidate_dir))?;

    let missing = run_compare(&Options {
        shard_dir: root.join("missing"),
        queries: vec!["retrieval".into()],
        top_k: 1,
        repeats: 1,
        self_test: false,
    });
    assert!(missing.is_err());
    remove_generated_temp_dir(&root)?;
    eprintln!("self-test ok");
    Ok(())
}

fn print_help() {
    eprintln!(
        "Usage: cargo run -p codestory-runtime --example lexical_compare -- --shard-dir <dir> [--query <q>] [--queries \"a|b\"] [--top-k 10] [--repeats 7]\n       cargo run -p codestory-runtime --example lexical_compare -- --self-test"
    );
}
