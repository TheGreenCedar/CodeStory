//! A supported local project reaches full retrieval through embedded storage and native embeddings.

use codestory_contracts::graph::{Node, NodeId, NodeKind};
use codestory_retrieval::{
    CandidateSource, EmbeddingEndpointOrigin, QueryRequest, RETRIEVAL_EMBEDDING_DIM,
    RetrievalCache, SidecarProcessDefaults, SidecarProfile, SidecarRuntimeConfig,
    SidecarRuntimeDefaults, SidecarRuntimeOverrides,
    execute_retrieval_query_with_cache_for_runtime, finalize_index_for_runtime,
};
use codestory_store::{
    FileInfo, FileRole, IndexPublicationMode, IndexPublicationRecord, LlmSymbolDoc,
    SearchSymbolProjection, Store,
};
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

struct EmbeddingServer {
    endpoint: String,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl EmbeddingServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind embedding server");
        listener
            .set_nonblocking(true)
            .expect("make embedding listener nonblocking");
        let address = listener.local_addr().expect("embedding server address");
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !server_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => serve_embedding(&mut stream),
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("accept embedding request: {error}"),
                }
            }
        });
        Self {
            endpoint: format!("http://{address}/v1/embeddings"),
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for EmbeddingServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.join().expect("join embedding server");
        }
    }
}

fn serve_embedding(stream: &mut TcpStream) {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = stream.read(&mut buffer).expect("read embedding request");
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        let Some(header_end) = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|index| index + 4)
        else {
            continue;
        };
        let header = String::from_utf8_lossy(&request[..header_end]);
        let content_length = header
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        if request.len().saturating_sub(header_end) >= content_length {
            break;
        }
    }
    let mut embedding = vec![0.0_f32; RETRIEVAL_EMBEDDING_DIM];
    embedding[0] = 1.0;
    let body = serde_json::json!({"data": [{"index": 0, "embedding": embedding}]}).to_string();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .expect("write embedding response");
}

fn runtime(project_root: &Path, cache_root: &Path, endpoint: String) -> SidecarRuntimeConfig {
    let defaults =
        SidecarProcessDefaults::new(cache_root.to_path_buf(), SidecarRuntimeDefaults::default());
    let mut runtime = SidecarRuntimeConfig::for_project_profile_with_process_defaults(
        Some(project_root),
        SidecarProfile::Local,
        None,
        &defaults,
        &SidecarRuntimeOverrides::default(),
    );
    runtime.embedding.endpoint = endpoint;
    runtime.embedding.endpoint_origin = EmbeddingEndpointOrigin::ProcessEnvironment;
    runtime.embedding.device_policy = "allow_cpu".into();
    runtime.embedding.server_launch = Some("external_endpoint".into());
    runtime
}

fn seed_fixture_graph(storage: &mut Store, project_root: &Path) -> NodeId {
    let file_node_id = 1_001_i64;
    storage
        .insert_file(&FileInfo {
            id: file_node_id,
            path: project_root.join("lib.rs"),
            language: "rust".to_string(),
            modification_time: 1,
            indexed: true,
            complete: true,
            line_count: 3,
            file_role: FileRole::Entrypoint,
        })
        .expect("insert file");
    storage
        .insert_nodes_batch(&[Node {
            id: NodeId(file_node_id),
            kind: NodeKind::FILE,
            serialized_name: "lib.rs".to_string(),
            start_line: Some(1),
            start_col: Some(0),
            end_line: Some(3),
            end_col: Some(0),
            ..Default::default()
        }])
        .expect("file node");
    let function = Node {
        id: NodeId(2_001),
        kind: NodeKind::FUNCTION,
        serialized_name: "extension_service".to_string(),
        qualified_name: Some("extension_service".to_string()),
        file_node_id: Some(NodeId(file_node_id)),
        start_line: Some(1),
        start_col: Some(0),
        end_line: Some(1),
        end_col: Some(30),
        ..Default::default()
    };
    storage
        .insert_nodes_batch(std::slice::from_ref(&function))
        .expect("function node");
    storage
        .upsert_search_symbol_projection_batch(&[SearchSymbolProjection {
            node_id: function.id,
            display_name: function.serialized_name.clone(),
        }])
        .expect("search projection");
    let mut vector = vec![0.0; RETRIEVAL_EMBEDDING_DIM];
    vector[0] = 1.0;
    storage
        .upsert_llm_symbol_docs_batch(&[LlmSymbolDoc {
            node_id: function.id,
            file_node_id: Some(NodeId(file_node_id)),
            kind: NodeKind::FUNCTION,
            display_name: function.serialized_name.clone(),
            qualified_name: function.qualified_name.clone(),
            file_path: Some("lib.rs".into()),
            start_line: Some(1),
            doc_text: "semantic_doc_version: 4\nsymbol_kind: FUNCTION\nname: extension_service"
                .into(),
            doc_version: 4,
            doc_hash: "extension-service-doc".into(),
            embedding_profile: Some("bge-base-en-v1.5".into()),
            embedding_model: "llamacpp:bge-base-en-v1.5".into(),
            embedding_backend: Some("llamacpp".into()),
            embedding_dim: RETRIEVAL_EMBEDDING_DIM as u32,
            doc_shape: Some("semantic_doc_version=4;scope=durable_symbols".into()),
            semantic_policy_version: Some("graph_first_v1".into()),
            dense_reason: Some("public_api".into()),
            embedding: vector,
            updated_at_epoch_ms: 1,
        }])
        .expect("semantic document");
    storage
        .put_index_publication(&IndexPublicationRecord {
            generation: 1,
            generation_id: "core-generation-1".into(),
            run_id: "core-run-1".into(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        })
        .expect("core publication");
    function.id
}

#[test]
fn embedded_default_reaches_full_mode_and_returns_dense_hits() {
    let embedding = EmbeddingServer::start();
    let project = TempDir::new().expect("project");
    std::fs::write(
        project.path().join("lib.rs"),
        "pub fn extension_service() {}\n",
    )
    .expect("write source");
    let cache = TempDir::new().expect("cache");
    let database = TempDir::new().expect("database");
    let storage_path = database.path().join("codestory.db");
    {
        let mut storage = Store::open(&storage_path).expect("open store");
        seed_fixture_graph(&mut storage, project.path());
    }
    let runtime = runtime(project.path(), cache.path(), embedding.endpoint.clone());

    let _finalized = finalize_index_for_runtime(project.path(), &storage_path, &runtime)
        .expect("finalize embedded retrieval");

    let result = execute_retrieval_query_with_cache_for_runtime(
        QueryRequest {
            project_root: project.path(),
            storage_path: &storage_path,
            query: "how the extension service is implemented",
            budget_ms: Some(2_000),
            cancelled: None,
        },
        &mut RetrievalCache::new(),
        &runtime,
    )
    .expect("query embedded retrieval");

    assert_eq!(result.trace.retrieval_mode, "full");
    assert!(
        result.hits.iter().any(|hit| {
            hit.node_id.as_deref() == Some("2001")
                && hit.file_path == "lib.rs"
                && (hit.source == CandidateSource::Semantic
                    || hit.provenance.iter().any(|source| source == "dense_anchor"))
        }),
        "expected embedded dense hit, got {:?}",
        result.hits
    );
}
