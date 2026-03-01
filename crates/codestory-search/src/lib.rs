use anyhow::{Context, Result, anyhow};
use codestory_core::NodeId;
use ndarray::{Array2, ArrayView2, ArrayView3, Axis};
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as NucleoConfig, Matcher, Utf32String};
use ort::session::Session;
use ort::value::TensorRef;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use tantivy::collector::TopDocs;
use tantivy::doc;
use tantivy::query::QueryParser;
use tantivy::schema::{FAST, INDEXED, STORED, Schema, TEXT, Value};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};

pub const EMBEDDING_DIM: usize = 384;
pub const EMBEDDING_MODEL_ENV: &str = "CODESTORY_EMBED_MODEL_PATH";
pub const EMBEDDING_MODEL_ID_ENV: &str = "CODESTORY_EMBED_MODEL_ID";
pub const EMBEDDING_TOKENIZER_ENV: &str = "CODESTORY_EMBED_TOKENIZER_PATH";
pub const EMBEDDING_MAX_TOKENS_ENV: &str = "CODESTORY_EMBED_MAX_TOKENS";
pub const EMBEDDING_RUNTIME_MODE_ENV: &str = "CODESTORY_EMBED_RUNTIME_MODE";

#[derive(Debug, Clone)]
pub struct LlmSearchDoc {
    pub node_id: NodeId,
    pub doc_text: String,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct HybridSearchHit {
    pub node_id: NodeId,
    pub lexical_score: f32,
    pub semantic_score: f32,
    pub graph_score: f32,
    pub total_score: f32,
}

#[derive(Debug, Clone)]
pub struct HybridSearchConfig {
    pub max_results: usize,
    pub lexical_weight: f32,
    pub semantic_weight: f32,
    pub graph_weight: f32,
    pub lexical_limit: usize,
    pub semantic_limit: usize,
}

impl Default for HybridSearchConfig {
    fn default() -> Self {
        Self {
            max_results: 20,
            lexical_weight: 0.35,
            semantic_weight: 0.55,
            graph_weight: 0.10,
            lexical_limit: 80,
            semantic_limit: 120,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EmbeddingRuntime {
    model_path: PathBuf,
    model_id: String,
    backend: EmbeddingBackend,
}

#[derive(Debug, Clone)]
enum EmbeddingBackend {
    Onnx(Arc<OnnxEmbeddingRuntime>),
    HashProjection,
}

#[derive(Debug)]
struct OnnxEmbeddingRuntime {
    session: Mutex<Session>,
    tokenizer: Mutex<Tokenizer>,
    max_tokens: usize,
}

impl EmbeddingRuntime {
    fn ensure_onnx_initialized() -> Result<()> {
        static INIT: OnceLock<()> = OnceLock::new();
        let _ = INIT.get_or_init(|| {
            // `commit()` returns false when another global environment is already configured.
            let _ = ort::init().with_name("codestory-embeddings").commit();
        });
        Ok(())
    }

    pub fn from_model_artifact<P: AsRef<Path>>(
        path: P,
        model_id: impl Into<String>,
    ) -> Result<Self> {
        Self::ensure_onnx_initialized()?;
        let model_path = path.as_ref().to_path_buf();
        if !model_path.is_file() {
            return Err(anyhow!(
                "embedding model artifact not found at {}",
                model_path.display()
            ));
        }
        let tokenizer_path = std::env::var(EMBEDDING_TOKENIZER_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|_| model_path.with_file_name("tokenizer.json"));
        if !tokenizer_path.is_file() {
            return Err(anyhow!(
                "embedding tokenizer not found at {} (set {EMBEDDING_TOKENIZER_ENV})",
                tokenizer_path.display()
            ));
        }

        let max_tokens = std::env::var(EMBEDDING_MAX_TOKENS_ENV)
            .ok()
            .and_then(|raw| raw.trim().parse::<usize>().ok())
            .map(|value| value.clamp(8, 1024))
            .unwrap_or(256);

        let mut tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|error| anyhow!("failed to load tokenizer: {error}"))?;
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: max_tokens,
                ..Default::default()
            }))
            .map_err(|error| anyhow!("failed to configure tokenizer truncation: {error}"))?;
        tokenizer.with_padding(Some(PaddingParams::default()));

        let session = Session::builder()
            .context("failed to create ONNX session builder")?
            .commit_from_file(&model_path)
            .with_context(|| format!("failed to load ONNX model {}", model_path.display()))?;

        Ok(Self {
            model_path,
            model_id: model_id.into(),
            backend: EmbeddingBackend::Onnx(Arc::new(OnnxEmbeddingRuntime {
                session: Mutex::new(session),
                tokenizer: Mutex::new(tokenizer),
                max_tokens,
            })),
        })
    }

    pub fn from_env() -> Result<Self> {
        let runtime_mode = std::env::var(EMBEDDING_RUNTIME_MODE_ENV)
            .unwrap_or_else(|_| "onnx".to_string())
            .trim()
            .to_ascii_lowercase();
        let model_id = std::env::var(EMBEDDING_MODEL_ID_ENV)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "sentence-transformers/all-MiniLM-L6-v2-local".to_string());

        if runtime_mode == "hash" || runtime_mode == "hash_projection" {
            return Ok(Self {
                model_path: PathBuf::from("hash-projection"),
                model_id,
                backend: EmbeddingBackend::HashProjection,
            });
        }

        let path = std::env::var(EMBEDDING_MODEL_ENV).with_context(|| {
            format!(
                "Missing {EMBEDDING_MODEL_ENV}. Configure a local embedding model artifact path."
            )
        })?;
        Self::from_model_artifact(path, model_id)
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    pub fn model_path(&self) -> &Path {
        &self.model_path
    }

    pub fn embed_query(&self, query: &str) -> Result<Vec<f32>> {
        if query.trim().is_empty() {
            return Err(anyhow!("query cannot be empty for semantic retrieval"));
        }
        let mut vectors = self.embed_texts(&[query.to_string()])?;
        vectors
            .pop()
            .ok_or_else(|| anyhow!("embedding runtime returned no query embedding"))
    }

    pub fn embed_texts(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        match &self.backend {
            EmbeddingBackend::HashProjection => {
                let mut out = Vec::with_capacity(texts.len());
                for text in texts {
                    if text.trim().is_empty() {
                        out.push(vec![0.0; EMBEDDING_DIM]);
                    } else {
                        out.push(embed_text_with_hash_projection(text, EMBEDDING_DIM));
                    }
                }
                Ok(out)
            }
            EmbeddingBackend::Onnx(runtime) => runtime.embed_texts(texts),
        }
    }

    #[cfg(test)]
    pub fn test_runtime() -> Self {
        Self {
            model_path: PathBuf::from("test-model.onnx"),
            model_id: "test-model".to_string(),
            backend: EmbeddingBackend::HashProjection,
        }
    }
}

impl OnnxEmbeddingRuntime {
    fn embed_texts(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let tokenizer = self
            .tokenizer
            .lock()
            .map_err(|_| anyhow!("embedding tokenizer lock poisoned"))?;
        let encodings = tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|error| anyhow!("failed to tokenize batch: {error}"))?;
        drop(tokenizer);

        let seq_len = encodings
            .iter()
            .map(|encoding| encoding.get_ids().len())
            .max()
            .unwrap_or(1)
            .clamp(1, self.max_tokens);

        let batch = encodings.len();
        let mut input_ids = Array2::<i64>::zeros((batch, seq_len));
        let mut attention_mask = Array2::<i64>::zeros((batch, seq_len));

        for (row, encoding) in encodings.iter().enumerate() {
            for (col, token_id) in encoding.get_ids().iter().take(seq_len).enumerate() {
                input_ids[[row, col]] = i64::from(*token_id);
            }
            for (col, mask) in encoding
                .get_attention_mask()
                .iter()
                .take(seq_len)
                .enumerate()
            {
                attention_mask[[row, col]] = i64::from(*mask);
            }
        }

        let input_ids_ref = TensorRef::from_array_view(&input_ids)
            .context("failed to build ONNX input_ids tensor")?;
        let attention_mask_ref = TensorRef::from_array_view(&attention_mask)
            .context("failed to build ONNX attention_mask tensor")?;

        let mut session = self
            .session
            .lock()
            .map_err(|_| anyhow!("embedding session lock poisoned"))?;
        let outputs = session
            .run(ort::inputs![
                "input_ids" => input_ids_ref,
                "attention_mask" => attention_mask_ref
            ])
            .context("ONNX embedding inference failed")?;

        let (name, output) = outputs
            .iter()
            .next()
            .ok_or_else(|| anyhow!("ONNX embedding model produced no outputs"))?;
        let output_array = output
            .try_extract_array::<f32>()
            .with_context(|| format!("failed to extract ONNX output tensor `{name}`"))?;

        let mut embeddings = match output_array.ndim() {
            2 => rows_to_vecs(output_array.view().into_dimensionality::<ndarray::Ix2>()?),
            3 => mean_pool_last_hidden(
                output_array.view().into_dimensionality::<ndarray::Ix3>()?,
                attention_mask.view(),
            ),
            n => {
                return Err(anyhow!(
                    "unsupported ONNX embedding output rank {n}; expected 2 or 3"
                ));
            }
        };

        for embedding in &mut embeddings {
            l2_normalize(embedding);
        }

        Ok(embeddings)
    }
}

fn rows_to_vecs(values: ArrayView2<'_, f32>) -> Vec<Vec<f32>> {
    values
        .axis_iter(Axis(0))
        .map(|row| row.iter().copied().collect::<Vec<_>>())
        .collect()
}

fn mean_pool_last_hidden(
    values: ArrayView3<'_, f32>,
    attention_mask: ArrayView2<'_, i64>,
) -> Vec<Vec<f32>> {
    let mut pooled = Vec::with_capacity(values.len_of(Axis(0)));
    for (batch_index, token_matrix) in values.axis_iter(Axis(0)).enumerate() {
        let hidden_size = token_matrix.len_of(Axis(1));
        let mut sum = vec![0.0_f32; hidden_size];
        let mut denom = 0.0_f32;

        for (token_index, token_vec) in token_matrix.axis_iter(Axis(0)).enumerate() {
            if attention_mask
                .get((batch_index, token_index))
                .copied()
                .unwrap_or(0)
                <= 0
            {
                continue;
            }
            denom += 1.0;
            for (idx, value) in token_vec.iter().copied().enumerate() {
                sum[idx] += value;
            }
        }

        if denom > 0.0 {
            for value in &mut sum {
                *value /= denom;
            }
        }
        pooled.push(sum);
    }
    pooled
}

pub struct SearchEngine {
    matcher: Matcher,
    symbols: Vec<(Utf32String, NodeId)>,
    index: Index,
    reader: IndexReader,
    llm_docs: HashMap<NodeId, LlmSearchDoc>,
    embedding_runtime: Option<EmbeddingRuntime>,
}

impl SearchEngine {
    pub fn new(storage_path: Option<&Path>) -> Result<Self> {
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("name", TEXT | STORED);
        schema_builder.add_i64_field("node_id", INDEXED | STORED | FAST);
        let schema = schema_builder.build();

        let index = if let Some(path) = storage_path {
            std::fs::create_dir_all(path)?;
            match Index::open_in_dir(path) {
                Ok(index) => index,
                Err(open_err) => Index::create_in_dir(path, schema.clone()).with_context(|| {
                    format!(
                        "Failed to open existing tantivy index at {}: {open_err}",
                        path.display()
                    )
                })?,
            }
        } else {
            Index::create_in_ram(schema)
        };

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;

        Ok(Self {
            matcher: Matcher::new(NucleoConfig::DEFAULT),
            symbols: Vec::new(),
            index,
            reader,
            llm_docs: HashMap::new(),
            embedding_runtime: None,
        })
    }

    pub fn set_embedding_runtime(&mut self, runtime: EmbeddingRuntime) {
        self.embedding_runtime = Some(runtime);
    }

    pub fn set_embedding_runtime_from_env(&mut self) -> Result<()> {
        let runtime = EmbeddingRuntime::from_env()?;
        self.embedding_runtime = Some(runtime);
        Ok(())
    }

    pub fn embedding_model_id(&self) -> Option<&str> {
        self.embedding_runtime
            .as_ref()
            .map(EmbeddingRuntime::model_id)
    }

    pub fn embedding_runtime_ready(&self) -> bool {
        self.embedding_runtime.is_some()
    }

    pub fn semantic_index_ready(&self) -> bool {
        self.embedding_runtime.is_some() && !self.llm_docs.is_empty()
    }

    pub fn embed_texts(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let runtime = self
            .embedding_runtime
            .as_ref()
            .ok_or_else(|| anyhow!("embedding runtime is not configured"))?;
        runtime.embed_texts(texts)
    }

    pub fn index_llm_symbol_docs(&mut self, docs: Vec<LlmSearchDoc>) {
        self.llm_docs.clear();
        for doc in docs {
            self.llm_docs.insert(doc.node_id, doc);
        }
    }

    pub fn llm_doc_count(&self) -> usize {
        self.llm_docs.len()
    }

    pub fn index_nodes(&mut self, nodes: Vec<(NodeId, String)>) -> Result<()> {
        let mut index_writer: IndexWriter<TantivyDocument> = self.index.writer(50_000_000)?;
        let schema = self.index.schema();
        let name_field = schema.get_field("name")?;
        let id_field = schema.get_field("node_id")?;

        for (id, name) in nodes {
            self.symbols.push((Utf32String::from(name.as_str()), id));
            index_writer.add_document(doc!(
                name_field => name,
                id_field => id.0
            ))?;
        }

        index_writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }

    pub fn search_symbol(&mut self, query: &str) -> Vec<NodeId> {
        if query.is_empty() {
            return Vec::new();
        }
        self.search_symbol_with_scores(query)
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    pub fn search_symbol_with_scores(&mut self, query: &str) -> Vec<(NodeId, f32)> {
        if query.is_empty() {
            return Vec::new();
        }

        let pattern = Pattern::new(
            query,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );

        let mut matches = Vec::new();

        for (name, id) in &self.symbols {
            if let Some(score) = pattern.score(name.slice(..), &mut self.matcher) {
                matches.push((*id, score));
            }
        }

        matches.sort_by_key(|b| std::cmp::Reverse(b.1));

        let mut seen = HashSet::new();
        matches
            .into_iter()
            .map(|(id, score)| (id, score as f32))
            .filter(|(id, _)| seen.insert(*id))
            .take(200)
            .collect()
    }

    pub fn search_hybrid_with_scores(
        &mut self,
        query: &str,
        graph_boosts: &HashMap<NodeId, f32>,
        config: HybridSearchConfig,
    ) -> Result<Vec<HybridSearchHit>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        if !self.semantic_index_ready() {
            return Err(anyhow!(
                "semantic retrieval is required but embedding runtime or semantic index is unavailable"
            ));
        }

        let runtime = self
            .embedding_runtime
            .as_ref()
            .ok_or_else(|| anyhow!("embedding runtime is not configured"))?;
        let query_embedding = runtime.embed_query(query)?;

        let lexical_matches = self.search_symbol_with_scores(query);
        let lexical_max = lexical_matches
            .iter()
            .map(|(_, score)| *score)
            .fold(0.0_f32, f32::max)
            .max(1.0);
        let lexical_map = lexical_matches
            .into_iter()
            .take(config.lexical_limit)
            .map(|(node_id, score)| (node_id, (score / lexical_max).clamp(0.0, 1.0)))
            .collect::<HashMap<_, _>>();

        let mut semantic_scored = self
            .llm_docs
            .values()
            .map(|doc| {
                let cosine = cosine_similarity(&query_embedding, &doc.embedding);
                (doc.node_id, ((cosine + 1.0) * 0.5).clamp(0.0, 1.0))
            })
            .collect::<Vec<_>>();
        semantic_scored.sort_by(|left, right| right.1.total_cmp(&left.1));

        let semantic_map = semantic_scored
            .into_iter()
            .take(config.semantic_limit)
            .collect::<HashMap<_, _>>();

        let mut candidate_ids = HashSet::new();
        candidate_ids.extend(lexical_map.keys().copied());
        candidate_ids.extend(semantic_map.keys().copied());
        candidate_ids.extend(graph_boosts.keys().copied());

        let lexical_weight = config.lexical_weight.clamp(0.0, 1.0);
        let semantic_weight = config.semantic_weight.clamp(0.0, 1.0);
        let graph_weight = config.graph_weight.clamp(0.0, 1.0);

        let mut hits = candidate_ids
            .into_iter()
            .map(|node_id| {
                let lexical_score = lexical_map.get(&node_id).copied().unwrap_or(0.0);
                let semantic_score = semantic_map.get(&node_id).copied().unwrap_or(0.0);
                let graph_score = graph_boosts
                    .get(&node_id)
                    .copied()
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);
                let total_score = lexical_weight * lexical_score
                    + semantic_weight * semantic_score
                    + graph_weight * graph_score;

                HybridSearchHit {
                    node_id,
                    lexical_score,
                    semantic_score,
                    graph_score,
                    total_score,
                }
            })
            .collect::<Vec<_>>();

        hits.sort_by(|left, right| {
            right
                .total_score
                .total_cmp(&left.total_score)
                .then_with(|| left.node_id.0.cmp(&right.node_id.0))
        });
        hits.truncate(config.max_results.max(1));

        Ok(hits)
    }

    pub fn remove_nodes(&mut self, nodes: &[NodeId]) -> Result<()> {
        if nodes.is_empty() {
            return Ok(());
        }

        let mut remove_ids = HashSet::new();
        for id in nodes {
            remove_ids.insert(id.0);
        }

        self.symbols.retain(|(_, id)| !remove_ids.contains(&id.0));
        self.llm_docs.retain(|id, _| !remove_ids.contains(&id.0));

        let mut index_writer: IndexWriter<TantivyDocument> = self.index.writer(50_000_000)?;
        let schema = self.index.schema();
        let node_field = schema.get_field("node_id")?;
        for id in &remove_ids {
            index_writer.delete_term(Term::from_field_i64(node_field, *id));
        }
        index_writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }

    pub fn search_full_text(&self, query_str: &str) -> Result<Vec<NodeId>> {
        if query_str.is_empty() {
            return Ok(Vec::new());
        }

        let searcher = self.reader.searcher();
        let schema = self.index.schema();
        let name_field = schema.get_field("name")?;
        let id_field = schema.get_field("node_id")?;

        let query_parser = QueryParser::for_index(&self.index, vec![name_field]);
        let query = query_parser
            .parse_query(query_str)
            .context("Failed to parse tantivy query")?;

        let top_docs = searcher.search(&query, &TopDocs::with_limit(20))?;

        let mut results = Vec::new();
        let mut seen = HashSet::new();
        for (_score, doc_address) in top_docs {
            let retrieved_doc: TantivyDocument = searcher.doc(doc_address)?;
            if let Some(id_val) = retrieved_doc.get_first(id_field).and_then(|v| v.as_i64()) {
                let id = NodeId(id_val);
                if seen.insert(id) {
                    results.push(id);
                }
            }
        }

        Ok(results)
    }
}

fn embed_text_with_hash_projection(text: &str, dim: usize) -> Vec<f32> {
    let mut vector = vec![0.0_f32; dim];

    for token in text.split_whitespace() {
        let norm = token.trim().to_ascii_lowercase();
        if norm.is_empty() {
            continue;
        }

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        norm.hash(&mut hasher);
        let hash = hasher.finish();
        let index = (hash as usize) % dim;
        let sign = if ((hash >> 8) & 1) == 0 { 1.0 } else { -1.0 };
        vector[index] += sign;

        // Add a tiny secondary feature for short-range context.
        let index2 = ((hash >> 17) as usize) % dim;
        vector[index2] += 0.5 * sign;
    }

    l2_normalize(&mut vector);
    vector
}

fn l2_normalize(values: &mut [f32]) {
    let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm <= f32::EPSILON {
        return;
    }
    for value in values {
        *value /= norm;
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    a.iter()
        .zip(b.iter())
        .map(|(left, right)| left * right)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_engine() -> Result<()> {
        let mut engine = SearchEngine::new(None)?;

        let nodes = vec![
            (NodeId(1), "MyClass".to_string()),
            (NodeId(2), "my_function".to_string()),
            (NodeId(3), "another_function".to_string()),
        ];

        engine.index_nodes(nodes)?;

        let results = engine.search_symbol("MyC");
        assert!(!results.is_empty(), "Should find at least one match");
        assert_eq!(
            results[0],
            NodeId(1),
            "MyClass should be the best match for 'MyC'"
        );

        let results = engine.search_symbol("func");
        assert_eq!(results.len(), 2);

        let results = engine.search_full_text("another")?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], NodeId(3));

        Ok(())
    }

    #[test]
    fn test_remove_nodes() -> Result<()> {
        let mut engine = SearchEngine::new(None)?;

        engine.index_nodes(vec![
            (NodeId(1), "AlphaSymbol".to_string()),
            (NodeId(2), "BetaSymbol".to_string()),
            (NodeId(3), "GammaSymbol".to_string()),
        ])?;

        let before = engine.search_symbol("Beta");
        assert!(before.contains(&NodeId(2)));
        assert_eq!(engine.search_full_text("betasymbol")?, vec![NodeId(2)]);

        engine.remove_nodes(&[NodeId(2)])?;

        let after = engine.search_symbol("Beta");
        assert!(!after.contains(&NodeId(2)));
        assert!(engine.search_full_text("betasymbol")?.is_empty());

        let remaining = engine.search_symbol("Gamma");
        assert!(remaining.contains(&NodeId(3)));

        Ok(())
    }

    #[test]
    fn test_hybrid_search_requires_semantic_runtime() -> Result<()> {
        let mut engine = SearchEngine::new(None)?;
        engine.index_nodes(vec![(NodeId(1), "AlphaSymbol".to_string())])?;
        let err = engine
            .search_hybrid_with_scores("alpha", &HashMap::new(), HybridSearchConfig::default())
            .expect_err("semantic runtime should be required");
        assert!(
            err.to_string().contains("semantic retrieval is required"),
            "unexpected error: {err}"
        );
        Ok(())
    }

    #[test]
    fn test_hybrid_search_scores() -> Result<()> {
        let mut engine = SearchEngine::new(None)?;
        engine.index_nodes(vec![
            (NodeId(1), "pkg::alpha".to_string()),
            (NodeId(2), "pkg::beta".to_string()),
        ])?;
        engine.set_embedding_runtime(EmbeddingRuntime::test_runtime());
        engine.index_llm_symbol_docs(vec![
            LlmSearchDoc {
                node_id: NodeId(1),
                doc_text: "alpha symbol".to_string(),
                embedding: embed_text_with_hash_projection("alpha symbol", EMBEDDING_DIM),
            },
            LlmSearchDoc {
                node_id: NodeId(2),
                doc_text: "beta symbol".to_string(),
                embedding: embed_text_with_hash_projection("beta symbol", EMBEDDING_DIM),
            },
        ]);

        let mut graph_boosts = HashMap::new();
        graph_boosts.insert(NodeId(1), 1.0);

        let hits = engine.search_hybrid_with_scores(
            "alpha",
            &graph_boosts,
            HybridSearchConfig {
                max_results: 5,
                lexical_weight: 0.3,
                semantic_weight: 0.5,
                graph_weight: 0.2,
                lexical_limit: 10,
                semantic_limit: 10,
            },
        )?;

        assert!(!hits.is_empty());
        assert_eq!(hits[0].node_id, NodeId(1));
        Ok(())
    }

    #[test]
    fn test_hybrid_search_semantic_can_win_with_weak_lexical_overlap() -> Result<()> {
        let mut engine = SearchEngine::new(None)?;
        engine.index_nodes(vec![
            (NodeId(10), "fn_a".to_string()),
            (NodeId(11), "fn_b".to_string()),
        ])?;
        engine.set_embedding_runtime(EmbeddingRuntime::test_runtime());
        engine.index_llm_symbol_docs(vec![
            LlmSearchDoc {
                node_id: NodeId(10),
                doc_text: "handles authorization policy and permission checks".to_string(),
                embedding: embed_text_with_hash_projection(
                    "handles authorization policy and permission checks",
                    EMBEDDING_DIM,
                ),
            },
            LlmSearchDoc {
                node_id: NodeId(11),
                doc_text: "renders UI theme colors and spacing".to_string(),
                embedding: embed_text_with_hash_projection(
                    "renders UI theme colors and spacing",
                    EMBEDDING_DIM,
                ),
            },
        ]);

        let hits = engine.search_hybrid_with_scores(
            "permission validation",
            &HashMap::new(),
            HybridSearchConfig {
                max_results: 5,
                lexical_weight: 0.2,
                semantic_weight: 0.7,
                graph_weight: 0.1,
                lexical_limit: 5,
                semantic_limit: 5,
            },
        )?;

        assert!(!hits.is_empty());
        assert_eq!(hits[0].node_id, NodeId(10));
        Ok(())
    }
}
