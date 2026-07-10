use crate::config::{QDRANT_HEALTH_BUDGET, SidecarLayout};
use crate::embeddings::{self, qdrant_vector_dim as active_vector_dim};
use crate::outbound_http::{OutboundHttpError, read_text, truncate_http_body};
use anyhow::{Context, Result, bail};
use codestory_store::FileRole;
use std::path::Path;
use std::time::{Duration, Instant};

/// Back-compat smoke width when semantic vectors are explicitly downgraded.
pub const QDRANT_VECTOR_DIM: usize = 8;

/// Batch size for Qdrant point upserts. This is not a coverage cap.
pub const QDRANT_INDEX_UPSERT_BATCH_SIZE: usize = 512;

const QDRANT_MUTATION_BUDGET: Duration = Duration::from_secs(5);
const QDRANT_UPSERT_BUDGET: Duration = Duration::from_secs(60);
const QDRANT_CREATE_POSTCONDITION_ATTEMPTS: usize = 10;
const QDRANT_CREATE_POSTCONDITION_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct QdrantUpsertPoint {
    pub id: u64,
    pub display_name: String,
    pub node_id: String,
    pub file_path: Option<String>,
    pub file_role: Option<FileRole>,
    pub dense_reason: Option<String>,
    pub vector: Option<Vec<f32>>,
}

#[derive(Debug, Clone)]
pub struct QdrantHealthProbe {
    pub reachable: bool,
    pub latency_ms: u64,
    pub collection_exists: bool,
    pub point_count: Option<u64>,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QdrantDeleteOutcome {
    Deleted,
    NotFound,
}

#[derive(Debug, Clone)]
pub struct QdrantClient {
    base_url: String,
    timeout: Duration,
}

impl QdrantClient {
    pub fn new(layout: &SidecarLayout) -> Self {
        Self {
            base_url: layout.qdrant_base_url(),
            timeout: QDRANT_HEALTH_BUDGET,
        }
    }

    pub fn collection_name(project_id: &str) -> String {
        format!("codestory_{project_id}")
    }

    pub fn collection_name_for_generation(project_id: &str, sidecar_input_hash: &str) -> String {
        crate::generation::sidecar_qdrant_collection(project_id, sidecar_input_hash)
    }

    /// List collection names from `GET /collections`.
    pub fn list_collection_names(&self) -> Result<Vec<String>> {
        let url = format!("{}/collections", self.base_url);
        let response = read_text(ureq::get(&url).timeout(self.timeout).call())
            .context("list qdrant collections")?;
        if !(200..300).contains(&response.status) {
            bail!(
                "list collections http {}: {}",
                response.status,
                truncate_http_body(&response.body)
            );
        }
        parse_collection_names(&response.body)
    }

    /// Reachability probe that does not require a project collection.
    pub fn list_collections_probe(&self) -> QdrantHealthProbe {
        let started = Instant::now();
        let url = format!("{}/collections", self.base_url);
        match read_text(ureq::get(&url).timeout(self.timeout).call()) {
            Ok(response) => {
                let status = response.status;
                QdrantHealthProbe {
                    reachable: (200..500).contains(&status),
                    latency_ms: started.elapsed().as_millis() as u64,
                    collection_exists: false,
                    point_count: None,
                    detail: format!("http {status}"),
                }
            }
            Err(error) => QdrantHealthProbe {
                reachable: false,
                latency_ms: started.elapsed().as_millis() as u64,
                collection_exists: false,
                point_count: None,
                detail: error.to_string(),
            },
        }
    }

    pub fn health_probe(&self, collection: &str) -> QdrantHealthProbe {
        let started = Instant::now();
        let url = format!("{}/collections/{collection}", self.base_url);
        let latency_ms = || started.elapsed().as_millis() as u64;
        match read_text(ureq::get(&url).timeout(self.timeout).call()) {
            Ok(response) => {
                let status = response.status;
                let point_count = parse_collection_point_count(&response.body);
                let detail = match point_count {
                    Some(count) => format!("http {status} points_count={count}"),
                    None => format!("http {status}"),
                };
                QdrantHealthProbe {
                    reachable: true,
                    latency_ms: latency_ms(),
                    collection_exists: status == 200,
                    point_count,
                    detail,
                }
            }
            Err(error) => match collection_probe_from_http_error(&error) {
                Some((reachable, collection_exists, detail)) => QdrantHealthProbe {
                    reachable,
                    latency_ms: latency_ms(),
                    collection_exists,
                    point_count: None,
                    detail,
                },
                None => QdrantHealthProbe {
                    reachable: false,
                    latency_ms: latency_ms(),
                    collection_exists: false,
                    point_count: None,
                    detail: error.to_string(),
                },
            },
        }
    }

    /// Exact count of indexed points in a generated collection.
    pub fn count_points_exact(&self, collection: &str) -> Result<u64> {
        let url = format!("{}/collections/{collection}/points/count", self.base_url);
        let body = serde_json::json!({ "exact": true });
        let payload = serde_json::to_string(&body).context("serialize qdrant count body")?;
        match read_text(
            ureq::post(&url)
                .timeout(self.timeout)
                .set("Content-Type", "application/json")
                .send_string(&payload),
        ) {
            Ok(response) => {
                let status = response.status;
                if !(200..300).contains(&status) {
                    bail!("qdrant count http {status}");
                }
                parse_count_points_response(&response.body)
            }
            Err(error) => Err(anyhow::anyhow!("qdrant count request failed: {error}")),
        }
    }

    /// Semantic search against Qdrant's Query API (parses `result.points[]` on HTTP 2xx).
    pub fn search(
        &self,
        collection: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<super::CandidateHit>> {
        let vector = query_vector(query)?;
        self.search_vector(collection, &vector, limit)
    }

    /// Diagnostic-only vector lookup against Qdrant without query embedding.
    ///
    /// Product retrieval must keep using [`QdrantClient::search`] so query
    /// embedding and Qdrant lookup stay in the mandatory sidecar path.
    pub fn diagnostic_search_vector(
        &self,
        collection: &str,
        vector: &[f32],
        limit: usize,
    ) -> Result<Vec<super::CandidateHit>> {
        self.search_vector(collection, vector, limit)
    }

    fn search_vector(
        &self,
        collection: &str,
        vector: &[f32],
        limit: usize,
    ) -> Result<Vec<super::CandidateHit>> {
        let expected_dim = active_vector_dim();
        if vector.len() != expected_dim {
            bail!(
                "qdrant query vector dim mismatch: vector={} expected={expected_dim}",
                vector.len()
            );
        }
        let url = format!("{}/collections/{collection}/points/query", self.base_url);
        let body = serde_json::json!({
            "query": vector,
            "limit": limit,
            "with_payload": true,
        });
        let payload = serde_json::to_string(&body).context("serialize qdrant search body")?;
        match read_text(
            ureq::post(&url)
                .timeout(self.timeout)
                .set("Content-Type", "application/json")
                .send_string(&payload),
        ) {
            Ok(response) => {
                let status = response.status;
                if (200..300).contains(&status) {
                    return parse_search_response(&response.body, limit);
                }
                bail!("qdrant search http {status}")
            }
            Err(error) => Err(anyhow::anyhow!("qdrant search request failed: {error}")),
        }
    }

    /// Drop collection when embedding backend/dim changes (idempotent).
    pub fn delete_collection(&self, collection: &str) -> Result<()> {
        self.delete_collection_with_outcome(collection).map(|_| ())
    }

    pub(crate) fn delete_collection_with_outcome(
        &self,
        collection: &str,
    ) -> Result<QdrantDeleteOutcome> {
        let url = format!("{}/collections/{collection}", self.base_url);
        match read_text(ureq::delete(&url).timeout(QDRANT_MUTATION_BUDGET).call()) {
            Ok(response) => {
                let status = response.status;
                if status == 200 {
                    self.clear_collection_stub_marker(collection);
                    return Ok(QdrantDeleteOutcome::Deleted);
                }
                if status == 404 {
                    self.clear_collection_stub_marker(collection);
                    return Ok(QdrantDeleteOutcome::NotFound);
                }
                bail!("delete collection http {status}");
            }
            Err(error) => {
                if error.is_status(404) {
                    self.clear_collection_stub_marker(collection);
                    return Ok(QdrantDeleteOutcome::NotFound);
                }
                bail!("delete collection request failed: {error}")
            }
        }
    }

    /// Create collection when missing (idempotent for 409 already exists).
    pub fn ensure_collection(&self, collection: &str) -> Result<()> {
        let probe = self.health_probe(collection);
        if !probe.reachable {
            bail!("qdrant unreachable: {}", probe.detail);
        }
        if probe.collection_exists {
            return Ok(());
        }
        let url = format!("{}/collections/{collection}", self.base_url);
        let body = serde_json::json!({
            "vectors": {
                "size": active_vector_dim(),
                "distance": "Cosine"
            }
        });
        let payload = serde_json::to_string(&body).context("serialize create collection body")?;
        match read_text(
            ureq::put(&url)
                .timeout(QDRANT_MUTATION_BUDGET)
                .set("Content-Type", "application/json")
                .send_string(&payload),
        ) {
            Ok(response) => {
                create_collection_status_result(response.status)?;
                Ok(())
            }
            Err(error) if error.status().is_some() => {
                create_collection_status_result(error.status().expect("checked status"))
            }
            Err(error) => wait_for_collection_create_postcondition(
                collection,
                &error.to_string(),
                QDRANT_CREATE_POSTCONDITION_ATTEMPTS,
                QDRANT_CREATE_POSTCONDITION_DELAY,
                || self.health_probe(collection),
                std::thread::sleep,
            ),
        }
    }

    /// Upsert semantic points (replaces same ids on conflict).
    ///
    /// Product indexing supplies stored local semantic-document vectors. If no vectors are supplied,
    /// this method still supports embedding labels for focused diagnostics; mixed batches fail.
    pub fn upsert_points(&self, collection: &str, points: &[QdrantUpsertPoint]) -> Result<usize> {
        if points.is_empty() {
            return Ok(0);
        }
        self.ensure_collection(collection)?;
        let url = format!(
            "{}/collections/{collection}/points?wait=true",
            self.base_url
        );
        let mut written = 0usize;
        for chunk in points.chunks(QDRANT_INDEX_UPSERT_BATCH_SIZE) {
            let vectors = vectors_for_points(chunk)?;
            if vectors.len() != chunk.len() {
                bail!(
                    "embedding batch returned {} vector(s) for {} qdrant point(s)",
                    vectors.len(),
                    chunk.len()
                );
            }
            let mut qdrant_points = Vec::with_capacity(chunk.len());
            for (point, vector) in chunk.iter().zip(vectors) {
                if vector.len() != active_vector_dim() {
                    bail!(
                        "qdrant point vector dim {} != collection dim {} for node {}",
                        vector.len(),
                        active_vector_dim(),
                        point.node_id
                    );
                }
                qdrant_points.push(serde_json::json!({
                        "id": point.id,
                        "vector": vector,
                        "payload": {
                            "node_id": point.node_id,
                            "display_name": point.display_name,
                            "path": point.file_path,
                            "file_role": point.file_role.map(FileRole::as_str),
                            "symbol": point.display_name,
                            "dense_reason": point.dense_reason,
                        }
                }));
            }
            let body = serde_json::json!({ "points": qdrant_points });
            let payload = serde_json::to_string(&body).context("serialize upsert body")?;
            match read_text(
                ureq::put(&url)
                    .timeout(QDRANT_UPSERT_BUDGET)
                    .set("Content-Type", "application/json")
                    .send_string(&payload),
            ) {
                Ok(response) => {
                    let status = response.status;
                    if (200..300).contains(&status) {
                        written += chunk.len();
                    } else {
                        bail!(
                            "upsert points http {status}: {}",
                            truncate_http_body(&response.body)
                        );
                    }
                }
                Err(error) if error.status().is_some() => {
                    let status = error.status().expect("checked status");
                    let body = error.body().unwrap_or_default();
                    bail!("upsert points http {status}: {}", truncate_http_body(body));
                }
                Err(error) => bail!("upsert points request failed: {error}"),
            }
        }
        self.clear_collection_stub_marker(collection);
        Ok(written)
    }

    /// Smoke semantic search used by health probes (requires indexed collection).
    pub fn semantic_search_smoke(&self, collection: &str) -> bool {
        self.semantic_search_smoke_result(collection).is_ok()
    }

    /// Smoke semantic search used by finalize paths when failure detail matters.
    pub fn semantic_search_smoke_result(&self, collection: &str) -> Result<()> {
        match self.search(collection, "function", 3) {
            Ok(hits) => {
                if hits.iter().any(hit_has_repo_relative_payload_path) {
                    Ok(())
                } else {
                    bail!(
                        "qdrant semantic smoke returned {} hit(s) without a repo-relative payload path",
                        hits.len()
                    );
                }
            }
            Err(error) => Err(error).context("qdrant semantic smoke search failed"),
        }
    }

    pub fn clear_collection_stub_marker(&self, collection: &str) {
        let layout = SidecarLayout::from_env();
        Self::clear_stub_marker_files(&layout.qdrant_data_dir, collection);
    }

    pub fn clear_stub_marker_files(qdrant_data_dir: &Path, collection: &str) {
        let marker = Self::stub_marker_path(qdrant_data_dir, collection);
        if marker.is_file() {
            let _ = std::fs::remove_file(marker);
        }
        let legacy = Self::legacy_stub_marker_path(qdrant_data_dir, collection);
        if legacy.is_file() {
            let _ = std::fs::remove_file(legacy);
        }
    }

    pub fn stub_marker_path(qdrant_data_dir: &Path, collection: &str) -> std::path::PathBuf {
        qdrant_data_dir
            .join("codestory-stub-markers")
            .join(format!("{collection}.qdrant-stub"))
    }

    pub fn legacy_stub_marker_path(qdrant_data_dir: &Path, collection: &str) -> std::path::PathBuf {
        qdrant_data_dir
            .join("collections")
            .join(collection)
            .join(".qdrant-stub")
    }

    pub fn is_collection_stubbed(qdrant_data_dir: &Path, collection: &str) -> bool {
        Self::stub_marker_path(qdrant_data_dir, collection).is_file()
            || Self::legacy_stub_marker_path(qdrant_data_dir, collection).is_file()
    }
}

fn hit_has_repo_relative_payload_path(hit: &super::CandidateHit) -> bool {
    let path = hit.file_path.trim();
    !path.is_empty()
        && !path.contains(':')
        && (path.contains('/') || path.contains('\\') || path.contains('.'))
}

fn parse_collection_names(body: &str) -> Result<Vec<String>> {
    let parsed: serde_json::Value =
        serde_json::from_str(body).context("parse qdrant collections response json")?;
    let Some(collections) = parsed
        .get("result")
        .and_then(|value| value.get("collections"))
        .and_then(|value| value.as_array())
    else {
        return Ok(Vec::new());
    };
    let mut names = Vec::new();
    for entry in collections {
        if let Some(name) = entry.get("name").and_then(|value| value.as_str()) {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

fn parse_collection_point_count(body: &str) -> Option<u64> {
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;
    parsed
        .get("result")
        .and_then(|value| value.get("points_count"))
        .or_else(|| {
            parsed
                .get("result")
                .and_then(|value| value.get("indexed_vectors_count"))
        })
        .and_then(|value| value.as_u64())
}

fn parse_count_points_response(body: &str) -> Result<u64> {
    let parsed: serde_json::Value =
        serde_json::from_str(body).context("parse qdrant count response json")?;
    parsed
        .get("result")
        .and_then(|value| value.get("count"))
        .and_then(|value| value.as_u64())
        .ok_or_else(|| anyhow::anyhow!("qdrant count response missing result.count"))
}

/// ureq treats 4xx as status errors; map collection GET 404 to reachable + missing.
fn collection_probe_from_http_error(error: &OutboundHttpError) -> Option<(bool, bool, String)> {
    let code = error.status()?;
    if code == 404 {
        return Some((true, false, "http 404".into()));
    }
    if code < 500 {
        return Some((true, false, format!("http {code}")));
    }
    None
}

fn create_collection_status_result(status: u16) -> Result<()> {
    if status == 200 || status == 201 || status == 409 {
        return Ok(());
    }
    bail!("create collection http {status}")
}

fn wait_for_collection_create_postcondition<F, S>(
    collection: &str,
    create_error: &str,
    attempts: usize,
    delay: Duration,
    mut probe: F,
    mut sleep: S,
) -> Result<()>
where
    F: FnMut() -> QdrantHealthProbe,
    S: FnMut(Duration),
{
    let mut last_probe_detail = String::from("not_probed");
    for attempt in 1..=attempts {
        let status = probe();
        last_probe_detail = status.detail;
        if status.collection_exists {
            return Ok(());
        }
        if attempt < attempts {
            sleep(delay);
        }
    }
    let reason = collection_create_failure_reason(create_error);
    bail!(
        "qdrant {reason} for collection {collection}: create request did not reach a confirmed collection after {attempts} postcondition probe(s); last_probe={last_probe_detail}; create_error={create_error}"
    )
}

fn collection_create_failure_reason(create_error: &str) -> &'static str {
    let error = create_error.to_ascii_lowercase();
    if error.contains("timeout") || error.contains("timed out") || error.contains("10060") {
        "collection_create_timeout"
    } else {
        "collection_create_failed"
    }
}

/// Parse Qdrant Query API JSON (`result.points[]` with payload paths / symbols).
pub fn parse_search_response(body: &str, limit: usize) -> Result<Vec<super::CandidateHit>> {
    use super::candidate::{CandidateHit, CandidateSource};
    let parsed: serde_json::Value =
        serde_json::from_str(body).context("parse qdrant search response json")?;
    let points = parsed
        .get("result")
        .and_then(|value| value.get("points"))
        .and_then(|value| value.as_array())
        .ok_or_else(|| anyhow::anyhow!("qdrant query response missing result.points"))?;
    let mut hits = Vec::new();
    for point in points {
        if hits.len() >= limit {
            break;
        }
        let score = point
            .get("score")
            .and_then(|value| value.as_f64())
            .map(|value| value as f32)
            .unwrap_or(0.5);
        let payload = point.get("payload").and_then(|value| value.as_object());
        let file_path = payload
            .and_then(|map| map.get("path"))
            .or_else(|| payload.and_then(|map| map.get("file_path")))
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .or_else(|| {
                payload
                    .and_then(|map| map.get("display_name"))
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            });
        let Some(file_path) = file_path else {
            continue;
        };
        let symbol_name = payload
            .and_then(|map| map.get("symbol"))
            .or_else(|| payload.and_then(|map| map.get("display_name")))
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let node_id = payload
            .and_then(|map| map.get("node_id"))
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let file_role = payload
            .and_then(|map| map.get("file_role"))
            .and_then(|value| value.as_str())
            .map(FileRole::from_db_value);
        let dense_reason = payload
            .and_then(|map| map.get("dense_reason"))
            .and_then(|value| value.as_str());
        let mut hit =
            CandidateHit::with_source(file_path, symbol_name, score, CandidateSource::Qdrant);
        hit.node_id = node_id;
        hit.file_role = file_role;
        if dense_reason == Some("component_report") {
            hit.add_provenance("component_report");
        } else {
            hit.add_provenance("dense_anchor");
        }
        hits.push(hit);
    }
    Ok(hits)
}

#[allow(dead_code)]
pub fn label_to_vector(label: &str) -> Vec<f32> {
    embeddings::label_to_vector(label)
}

fn query_vector(query: &str) -> Result<Vec<f32>> {
    embeddings::embed_query(query)
}

/// Diagnostic-only helper for offline vector-index parity checks.
///
/// Product retrieval must keep using [`QdrantClient::search`], which embeds and
/// queries the mandatory Qdrant sidecar collection in one fail-closed path.
pub fn diagnostic_query_vector(query: &str) -> Result<Vec<f32>> {
    query_vector(query)
}

fn document_vectors(labels: &[String]) -> Result<Vec<Vec<f32>>> {
    embeddings::embed_documents(labels)
}

fn vectors_for_points(points: &[QdrantUpsertPoint]) -> Result<Vec<Vec<f32>>> {
    let provided = points.iter().filter(|point| point.vector.is_some()).count();
    if provided == points.len() {
        return Ok(points
            .iter()
            .map(|point| point.vector.clone().expect("count verified vectors"))
            .collect());
    }
    if provided != 0 {
        bail!(
            "qdrant upsert received mixed stored and generated vectors; this would hide embedding contract drift"
        );
    }

    let labels = points
        .iter()
        .map(|point| point.display_name.clone())
        .collect::<Vec<_>>();
    document_vectors(&labels).with_context(|| {
        format!(
            "embed qdrant document batch size={} first={}",
            labels.len(),
            labels.first().map(String::as_str).unwrap_or("<empty>")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::CandidateSource;

    #[test]
    fn label_to_vector_has_fixed_dim() {
        let vector = label_to_vector("handler");
        assert_eq!(vector.len(), QDRANT_VECTOR_DIM);
        assert!(vector.iter().all(|value| (0.0..=1.0).contains(value)));
    }

    #[test]
    fn parse_search_response_maps_query_points() {
        let body = r#"{
            "result": {
                "points": [
                    {
                        "score": 0.91,
                        "payload": {
                            "node_id": "42",
                            "path": "src/handler.rs",
                            "symbol": "handle_request",
                            "file_role": "source",
                            "dense_reason": "public_api"
                        }
                    }
                ]
            }
        }"#;
        let hits = parse_search_response(body, 8).expect("parse");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].file_path, "src/handler.rs");
        assert_eq!(hits[0].node_id.as_deref(), Some("42"));
        assert_eq!(hits[0].symbol_name.as_deref(), Some("handle_request"));
        assert_eq!(hits[0].source, CandidateSource::Qdrant);
        assert_eq!(hits[0].file_role, Some(FileRole::Source));
        assert_eq!(hits[0].provenance, vec!["dense_anchor".to_string()]);
    }

    #[test]
    fn parse_search_response_empty_result_is_empty() {
        let hits = parse_search_response(r#"{ "result": { "points": [] } }"#, 8).expect("parse");
        assert!(hits.is_empty());
    }

    #[test]
    fn parse_search_response_missing_result_points_is_error() {
        let error = parse_search_response(r#"{ "status": "ok" }"#, 8)
            .expect_err("query response must contain result.points");
        assert!(error.to_string().contains("result.points"));
    }

    #[test]
    fn parse_collection_names_reads_result_array() {
        let body = r#"{
            "result": {
                "collections": [
                    { "name": "codestory_a" },
                    { "name": "other" }
                ]
            }
        }"#;
        let names = parse_collection_names(body).expect("parse");
        assert_eq!(names, vec!["codestory_a", "other"]);
    }

    #[test]
    fn parse_collection_point_count_reads_collection_info() {
        let body = r#"{
            "result": {
                "status": "green",
                "points_count": 42,
                "indexed_vectors_count": 40
            }
        }"#;

        assert_eq!(parse_collection_point_count(body), Some(42));
    }

    #[test]
    fn parse_count_points_response_reads_exact_count() {
        let body = r#"{ "result": { "count": 1234 }, "status": "ok" }"#;

        assert_eq!(parse_count_points_response(body).expect("parse"), 1234);
    }

    #[test]
    fn create_collection_status_accepts_already_exists() {
        create_collection_status_result(409).expect("already exists is idempotent");
    }

    #[test]
    fn create_collection_status_rejects_non_409_client_error() {
        let error = create_collection_status_result(403)
            .expect_err("definite create HTTP errors should not wait for postcondition");

        assert_eq!(error.to_string(), "create collection http 403");
    }

    #[test]
    fn create_collection_timeout_accepts_late_collection_postcondition() {
        let mut probes = vec![
            QdrantHealthProbe {
                reachable: true,
                latency_ms: 1,
                collection_exists: false,
                point_count: None,
                detail: "http 404".into(),
            },
            QdrantHealthProbe {
                reachable: true,
                latency_ms: 1,
                collection_exists: true,
                point_count: Some(0),
                detail: "http 200 points_count=0".into(),
            },
        ]
        .into_iter();
        let mut sleeps = 0;

        wait_for_collection_create_postcondition(
            "codestory_project_hash",
            "operation timed out",
            3,
            Duration::from_millis(1),
            || probes.next().expect("probe"),
            |_| sleeps += 1,
        )
        .expect("late collection should satisfy create postcondition");

        assert_eq!(sleeps, 1);
    }

    #[test]
    fn create_collection_timeout_names_collection_and_last_probe() {
        let error = wait_for_collection_create_postcondition(
            "codestory_project_hash",
            "os error 10060",
            2,
            Duration::from_millis(1),
            || QdrantHealthProbe {
                reachable: true,
                latency_ms: 1,
                collection_exists: false,
                point_count: Some(0),
                detail: "http 200 points_count=0".into(),
            },
            |_| {},
        )
        .expect_err("missing postcondition should fail closed");
        let message = error.to_string();

        assert!(message.contains("collection_create_timeout"));
        assert!(message.contains("codestory_project_hash"));
        assert!(message.contains("last_probe=http 200 points_count=0"));
    }

    #[test]
    fn create_collection_transport_failure_waits_then_reports_not_ready() {
        let error = wait_for_collection_create_postcondition(
            "codestory_project_hash",
            "connection closed before message completed",
            1,
            Duration::from_millis(1),
            || QdrantHealthProbe {
                reachable: false,
                latency_ms: 1,
                collection_exists: false,
                point_count: None,
                detail: "transport closed".into(),
            },
            |_| {},
        )
        .expect_err("uncertain transport failure should still fail closed when not ready");
        let message = error.to_string();

        assert!(message.contains("collection_create_failed"));
        assert!(message.contains("codestory_project_hash"));
        assert!(message.contains("last_probe=transport closed"));
    }
}
