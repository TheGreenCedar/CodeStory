#[cfg(not(windows))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!(
        "turbovec_dense_anchor_gate is Windows-only until turbovec 0.9.0 no longer requires a system BLAS link on Linux/macOS"
    )
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    windows_gate::run()
}

#[cfg(windows)]
mod windows_gate {
    use anyhow::{Context, Result, bail};
    use codestory_contracts::graph::{NodeId, NodeKind};
    use codestory_retrieval::{
        QdrantClient, SidecarLayout, diagnostic_query_vector, embedding_runtime_id,
        qdrant_vector_dim, sidecar_project_id_for_root, strict_sidecar_status,
    };
    use codestory_store::{LlmSymbolDoc, Store};
    use serde::Serialize;
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};
    use turbovec::IdMapIndex;

    const DEFAULT_QUERIES: &[&str] = &[
        "Where is strict retrieval sidecar readiness enforced?",
        "How are dense anchors selected for semantic retrieval?",
        "Where does Qdrant text query search embed the query?",
    ];

    pub fn run() -> Result<()> {
        let args = Args::parse()?;
        if args.self_test {
            self_test()?;
            println!("self-test ok");
            return Ok(());
        }

        let project = args.project.context("missing --project")?;
        let storage_path = args.storage.context("missing --storage")?;
        let project_id = sidecar_project_id_for_root(&project);
        let storage = Store::open(&storage_path).context("open storage")?;
        let strict_status = strict_sidecar_status(&project, Some(&storage_path))
            .context("check strict sidecar readiness")?;
        if strict_status.retrieval_mode != "full" {
            bail!(
                "strict_sidecar_unavailable: mode={} reason={}",
                strict_status.retrieval_mode,
                strict_status
                    .degraded_reason
                    .as_deref()
                    .unwrap_or("<missing>")
            );
        }
        let manifest = storage
            .get_retrieval_index_manifest(&project_id)
            .context("load retrieval manifest")?
            .with_context(|| format!("retrieval_manifest_missing for project_id={project_id}"))?;

        let dense_docs = dense_docs(&storage)?;
        validate_identity(&storage, &manifest, &dense_docs)?;

        let layout = SidecarLayout::from_env();
        let qdrant = QdrantClient::new(&layout);
        let qdrant_count = qdrant
            .count_points_exact(&manifest.qdrant_collection)
            .context("qdrant count failed")?;
        if Some(qdrant_count as i64) != manifest.dense_projection_count {
            bail!(
                "qdrant_count_mismatch: manifest={:?} qdrant={qdrant_count}",
                manifest.dense_projection_count
            );
        }

        let (index, id_to_doc, build_ms) = build_index(&dense_docs)?;
        let artifact = args.artifact.unwrap_or_else(default_artifact_path);
        let write_started = Instant::now();
        index.write(&artifact).context("write turbovec artifact")?;
        let write_ms = elapsed_ms(write_started.elapsed());
        let artifact_bytes = artifact.metadata().map(|meta| meta.len()).unwrap_or(0);
        let load_started = Instant::now();
        let loaded = IdMapIndex::load(&artifact).context("load turbovec artifact")?;
        loaded.prepare();
        let load_ms = elapsed_ms(load_started.elapsed());

        let mut query_reports = Vec::new();
        for query in args.queries {
            query_reports.push(compare_query(
                &qdrant,
                &manifest.qdrant_collection,
                &loaded,
                &id_to_doc,
                &query,
                args.k,
                args.repeats,
            )?);
        }

        let report = Report {
            diagnostic_only: true,
            product_path_changed: false,
            project: project.display().to_string(),
            storage: storage_path.display().to_string(),
            project_id,
            qdrant_collection: manifest.qdrant_collection,
            manifest_embedding_backend: manifest.embedding_backend,
            query_embedding_backend: embedding_runtime_id(),
            embedding_dim: qdrant_vector_dim(),
            dense_anchor_count: dense_docs.len(),
            qdrant_count,
            artifact: artifact.display().to_string(),
            artifact_bytes,
            build_ms,
            write_ms,
            load_ms,
            queries: query_reports,
            recommendation: "diagnostic measurement only; Qdrant remains product path",
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
        Ok(())
    }

    fn dense_docs(storage: &Store) -> Result<Vec<LlmSymbolDoc>> {
        Ok(storage
            .get_all_llm_symbol_docs()
            .context("load stored symbol docs")?
            .into_iter()
            .filter(|doc| doc.dense_reason.is_some())
            .collect())
    }

    fn validate_identity(
        storage: &Store,
        manifest: &codestory_store::RetrievalIndexManifest,
        dense_docs: &[LlmSymbolDoc],
    ) -> Result<()> {
        let backend = embedding_runtime_id();
        let dim = qdrant_vector_dim();
        if manifest.embedding_backend.as_deref() != Some(backend.as_str()) {
            bail!(
                "manifest_backend_mismatch: manifest={:?} current={backend}",
                manifest.embedding_backend
            );
        }
        if manifest.embedding_dim != Some(dim as i32) {
            bail!(
                "manifest_dim_mismatch: manifest={:?} current={dim}",
                manifest.embedding_dim
            );
        }
        if manifest.dense_projection_count != Some(dense_docs.len() as i64) {
            bail!(
                "dense_anchor_count_mismatch: manifest={:?} stored={}",
                manifest.dense_projection_count,
                dense_docs.len()
            );
        }
        let stats = storage
            .get_llm_symbol_doc_stats()
            .context("load stored doc vector stats")?;
        if stats.mixed_embedding_backends || stats.mixed_dimensions {
            bail!("stored_doc_vector_contract_mixed");
        }
        if dense_docs
            .iter()
            .any(|doc| doc.embedding_dim as usize != dim)
        {
            bail!("dense_doc_dim_mismatch");
        }
        if dense_docs.iter().any(|doc| {
            !stored_doc_backend_matches_runtime(doc.embedding_backend.as_deref(), &backend)
        }) {
            bail!("dense_doc_backend_mismatch");
        }
        Ok(())
    }

    fn stored_doc_backend_matches_runtime(stored: Option<&str>, runtime: &str) -> bool {
        stored == Some(runtime)
            || matches!(
                (stored, runtime),
                (Some("llamacpp"), "llamacpp:bge-base-en-v1.5")
            )
    }

    fn build_index(
        docs: &[LlmSymbolDoc],
    ) -> Result<(IdMapIndex, HashMap<u64, LlmSymbolDoc>, u128)> {
        let dim = qdrant_vector_dim();
        let mut vectors = Vec::with_capacity(docs.len() * dim);
        let mut ids = Vec::with_capacity(docs.len());
        let mut id_to_doc = HashMap::with_capacity(docs.len());
        for (index, doc) in docs.iter().enumerate() {
            // ponytail: local ids avoid unsigned casts for virtual graph ids; use real ids if turbovec gains signed ids.
            let id = u64::try_from(index + 1).context("too many dense docs")?;
            if doc.embedding.len() != dim {
                bail!(
                    "dense_doc_vector_dim_mismatch: node_id={} vector_dim={} expected={dim}",
                    doc.node_id.0,
                    doc.embedding.len()
                );
            }
            vectors.extend_from_slice(&doc.embedding);
            ids.push(id);
            id_to_doc.insert(id, doc.clone());
        }
        let started = Instant::now();
        let mut index = IdMapIndex::new(dim, 4).context("construct turbovec index")?;
        index
            .add_with_ids(&vectors, &ids)
            .context("add dense anchors to turbovec")?;
        index.prepare();
        Ok((index, id_to_doc, elapsed_ms(started.elapsed())))
    }

    fn compare_query(
        qdrant: &QdrantClient,
        collection: &str,
        index: &IdMapIndex,
        id_to_doc: &HashMap<u64, LlmSymbolDoc>,
        query: &str,
        k: usize,
        repeats: usize,
    ) -> Result<QueryReport> {
        let query_vector = diagnostic_query_vector(query).context("embed query for turbovec")?;
        let qdrant_hits = qdrant
            .diagnostic_search_vector(collection, &query_vector, k)
            .context("qdrant vector search")?;
        let turbovec_hits = turbovec_search(index, id_to_doc, &query_vector, k);
        let repeats = repeats.max(1);
        let _ = qdrant.search(collection, query, k);
        let _ = qdrant.diagnostic_search_vector(collection, &query_vector, k);
        let _ = index.search(&query_vector, k);
        let embedding_ms = timed_repeats(repeats, || diagnostic_query_vector(query).map(drop))?;
        let qdrant_total_ms =
            timed_repeats(repeats, || qdrant.search(collection, query, k).map(drop))?;
        let qdrant_lookup_ms = timed_repeats(repeats, || {
            qdrant
                .diagnostic_search_vector(collection, &query_vector, k)
                .map(drop)
        })?;
        let turbovec_lookup_ms = timed_repeats(repeats, || {
            let _ = index.search(&query_vector, k);
            Ok(())
        })?;
        let qdrant_ids = qdrant_hits
            .iter()
            .filter_map(|hit| hit.node_id.as_deref())
            .collect::<Vec<_>>();
        let turbovec_ids = turbovec_hits
            .iter()
            .map(|hit| hit.node_id.as_str())
            .collect::<Vec<_>>();
        Ok(QueryReport {
            query: query.to_string(),
            overlap_at_k: overlap_ratio(&qdrant_ids, &turbovec_ids),
            mrr_delta: mrr(&qdrant_ids, &qdrant_ids) - mrr(&qdrant_ids, &turbovec_ids),
            query_embedding_p50_ms: percentile(&embedding_ms, 50.0),
            query_embedding_p95_ms: percentile(&embedding_ms, 95.0),
            query_embedding_p99_ms: percentile(&embedding_ms, 99.0),
            qdrant_total_p50_ms: percentile(&qdrant_total_ms, 50.0),
            qdrant_total_p95_ms: percentile(&qdrant_total_ms, 95.0),
            qdrant_total_p99_ms: percentile(&qdrant_total_ms, 99.0),
            qdrant_lookup_p50_ms: percentile(&qdrant_lookup_ms, 50.0),
            qdrant_lookup_p95_ms: percentile(&qdrant_lookup_ms, 95.0),
            qdrant_lookup_p99_ms: percentile(&qdrant_lookup_ms, 99.0),
            turbovec_lookup_p50_ms: percentile(&turbovec_lookup_ms, 50.0),
            turbovec_lookup_p95_ms: percentile(&turbovec_lookup_ms, 95.0),
            turbovec_lookup_p99_ms: percentile(&turbovec_lookup_ms, 99.0),
            qdrant_top_k: qdrant_ids.into_iter().map(str::to_string).collect(),
            turbovec_top_k: turbovec_ids.into_iter().map(str::to_string).collect(),
        })
    }

    fn turbovec_search(
        index: &IdMapIndex,
        id_to_doc: &HashMap<u64, LlmSymbolDoc>,
        query_vector: &[f32],
        k: usize,
    ) -> Vec<TurbovecHit> {
        let (scores, ids) = index.search(query_vector, k);
        ids.into_iter()
            .zip(scores)
            .filter_map(|(id, score)| {
                let doc = id_to_doc.get(&id)?;
                Some(TurbovecHit {
                    node_id: doc.node_id.0.to_string(),
                    score,
                })
            })
            .collect()
    }

    fn timed_repeats(mut repeats: usize, mut f: impl FnMut() -> Result<()>) -> Result<Vec<f64>> {
        let mut timings = Vec::with_capacity(repeats);
        while repeats > 0 {
            let started = Instant::now();
            f()?;
            timings.push(started.elapsed().as_secs_f64() * 1000.0);
            repeats -= 1;
        }
        Ok(timings)
    }

    fn overlap_ratio(left: &[&str], right: &[&str]) -> f64 {
        let left = left.iter().copied().collect::<HashSet<_>>();
        let overlap = right.iter().filter(|id| left.contains(**id)).count();
        overlap as f64 / left.len().max(1) as f64
    }

    fn mrr(expected: &[&str], actual: &[&str]) -> f64 {
        for (rank, id) in actual.iter().enumerate() {
            if expected.contains(id) {
                return 1.0 / (rank + 1) as f64;
            }
        }
        0.0
    }

    fn percentile(values: &[f64], percentile: f64) -> f64 {
        let mut values = values.to_vec();
        values.sort_by(f64::total_cmp);
        let index = ((values.len().saturating_sub(1)) as f64 * percentile / 100.0).ceil() as usize;
        values[index.min(values.len().saturating_sub(1))]
    }

    fn elapsed_ms(duration: Duration) -> u128 {
        duration.as_millis()
    }

    fn default_artifact_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "codestory-turbovec-dense-anchor-{}.tvim",
            std::process::id()
        ))
    }

    fn self_test() -> Result<()> {
        let dim = 8;
        let vectors = vec![
            1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ];
        let mut index = IdMapIndex::new(dim, 4)?;
        index.add_with_ids(&vectors, &[11, 22])?;
        let (_scores, ids) = index.search(&vectors[0..dim], 1);
        assert_eq!(ids[0], 11);
        assert_eq!(percentile(&[1.0, 2.0, 3.0], 95.0), 3.0);
        assert_eq!(percentile(&[1.0, 2.0, 3.0], 99.0), 3.0);
        assert_eq!(overlap_ratio(&["a", "b"], &["b", "c"]), 0.5);
        assert!(stored_doc_backend_matches_runtime(
            Some("llamacpp"),
            "llamacpp:bge-base-en-v1.5"
        ));
        assert!(!stored_doc_backend_matches_runtime(
            Some("onnx"),
            "llamacpp:bge-base-en-v1.5"
        ));
        let mut embedding = vec![0.0; qdrant_vector_dim()];
        embedding[0] = 1.0;
        let negative_doc = LlmSymbolDoc {
            node_id: NodeId(-42),
            file_node_id: None,
            kind: NodeKind::FUNCTION,
            display_name: "virtual_dense_anchor".into(),
            qualified_name: None,
            file_path: None,
            start_line: None,
            doc_text: "virtual dense anchor".into(),
            doc_version: 5,
            doc_hash: "virtual-dense-anchor".into(),
            embedding_profile: Some("bge-base-en-v1.5".into()),
            embedding_model: "BAAI/bge-base-en-v1.5-local".into(),
            embedding_backend: Some("llamacpp".into()),
            embedding_dim: qdrant_vector_dim() as u32,
            doc_shape: Some("self-test".into()),
            semantic_policy_version: Some("graph_first_v1".into()),
            dense_reason: Some("component_report".into()),
            embedding: embedding.clone(),
            updated_at_epoch_ms: 0,
        };
        let (index, id_to_doc, _) = build_index(&[negative_doc])?;
        let hits = turbovec_search(&index, &id_to_doc, &embedding, 1);
        assert_eq!(hits[0].node_id, "-42");
        Ok(())
    }

    #[derive(Debug)]
    struct Args {
        project: Option<PathBuf>,
        storage: Option<PathBuf>,
        artifact: Option<PathBuf>,
        queries: Vec<String>,
        k: usize,
        repeats: usize,
        self_test: bool,
    }

    impl Args {
        fn parse() -> Result<Self> {
            let mut args = std::env::args().skip(1);
            let mut parsed = Self {
                project: None,
                storage: None,
                artifact: None,
                queries: Vec::new(),
                k: 10,
                repeats: 5,
                self_test: false,
            };
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--project" => {
                        parsed.project = Some(PathBuf::from(next_value(&mut args, &arg)?))
                    }
                    "--storage" => {
                        parsed.storage = Some(PathBuf::from(next_value(&mut args, &arg)?))
                    }
                    "--artifact" => {
                        parsed.artifact = Some(PathBuf::from(next_value(&mut args, &arg)?))
                    }
                    "--query" => parsed.queries.push(next_value(&mut args, &arg)?),
                    "--k" => {
                        parsed.k = next_value(&mut args, &arg)?.parse().context("parse --k")?
                    }
                    "--repeats" => {
                        parsed.repeats = next_value(&mut args, &arg)?
                            .parse()
                            .context("parse --repeats")?
                    }
                    "--self-test" => parsed.self_test = true,
                    _ => bail!("unknown argument: {arg}"),
                }
            }
            if parsed.queries.is_empty() {
                parsed.queries = DEFAULT_QUERIES
                    .iter()
                    .map(|query| query.to_string())
                    .collect();
            }
            Ok(parsed)
        }
    }

    fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
        args.next()
            .with_context(|| format!("missing value for {flag}"))
    }

    #[derive(Serialize)]
    struct Report {
        diagnostic_only: bool,
        product_path_changed: bool,
        project: String,
        storage: String,
        project_id: String,
        qdrant_collection: String,
        manifest_embedding_backend: Option<String>,
        query_embedding_backend: String,
        embedding_dim: usize,
        dense_anchor_count: usize,
        qdrant_count: u64,
        artifact: String,
        artifact_bytes: u64,
        build_ms: u128,
        write_ms: u128,
        load_ms: u128,
        queries: Vec<QueryReport>,
        recommendation: &'static str,
    }

    #[derive(Serialize)]
    struct QueryReport {
        query: String,
        overlap_at_k: f64,
        mrr_delta: f64,
        query_embedding_p50_ms: f64,
        query_embedding_p95_ms: f64,
        query_embedding_p99_ms: f64,
        qdrant_total_p50_ms: f64,
        qdrant_total_p95_ms: f64,
        qdrant_total_p99_ms: f64,
        qdrant_lookup_p50_ms: f64,
        qdrant_lookup_p95_ms: f64,
        qdrant_lookup_p99_ms: f64,
        turbovec_lookup_p50_ms: f64,
        turbovec_lookup_p95_ms: f64,
        turbovec_lookup_p99_ms: f64,
        qdrant_top_k: Vec<String>,
        turbovec_top_k: Vec<String>,
    }

    struct TurbovecHit {
        node_id: String,
        #[allow(dead_code)]
        score: f32,
    }
}
