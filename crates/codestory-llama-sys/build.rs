mod model_staging;
mod native_staging;

use model_staging::{ExpectedModel, stage_model, verify_model};
use native_staging::stage_linux_shared_libraries;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::PathBuf;

const MODEL_CONTRACT_FILE: &str = "model-contract.json";
const NATIVE_RUNTIME_FILE_LIST: &str = "codestory-native-runtime-files-v1.txt";
const EMBEDDING_SERVER_PROTOCOL_FILE: &str =
    "../../docs/testing/per-user-embedding-server-protocol.json";
const EMBEDDING_SERVER_CONSTANT_SET_FILE: &str =
    "../../docs/testing/per-user-embedding-server-constant-set.json";
const EMBEDDING_SERVER_MEASUREMENT_PROTOCOL_FILE: &str =
    "../../docs/testing/per-user-embedding-server-measurement-protocol.json";

struct ModelContract {
    file_name: String,
    size: u64,
    sha256: String,
    embedding_family: String,
    llama_cpp_crate_version: String,
    llama_cpp_source_commit: String,
    dimension: u64,
    query_prefix: String,
    document_prefix: String,
    pooling: String,
    normalization: String,
    element_type: String,
    vector_schema_version: u64,
    tokenizer_sha256: String,
    config_sha256: String,
    producer_name: String,
    producer_version: String,
    license_spdx_id: String,
    license_source_url: String,
}

struct EmbeddingServerConstants {
    frozen: bool,
    connect_timeout_ms: u64,
    spawn_convergence_timeout_ms: u64,
    retry_after_ms: u64,
    query_request_deadline_ms: u64,
    bulk_replay_success_budget_ms: u64,
    bulk_request_deadline_ms: u64,
    hard_native_no_progress_ms: u64,
    watchdog_cadence_ms: u64,
    election_initial_backoff_ms: Option<u64>,
    election_maximum_backoff_ms: Option<u64>,
}

fn main() {
    println!("cargo:rerun-if-env-changed=CODESTORY_EMBED_MODEL_SOURCE");
    println!("cargo:rerun-if-changed={MODEL_CONTRACT_FILE}");
    println!("cargo:rerun-if-changed=model_staging.rs");
    println!("cargo:rerun-if-changed=native_staging.rs");
    println!("cargo:rerun-if-changed={EMBEDDING_SERVER_PROTOCOL_FILE}");
    println!("cargo:rerun-if-changed={EMBEDDING_SERVER_CONSTANT_SET_FILE}");
    println!("cargo:rerun-if-changed={EMBEDDING_SERVER_MEASUREMENT_PROTOCOL_FILE}");

    let contract = load_model_contract();
    let embedding_server_protocol_sha256 = file_sha256(EMBEDDING_SERVER_PROTOCOL_FILE);
    let embedding_server_constant_set_sha256 = file_sha256(EMBEDDING_SERVER_CONSTANT_SET_FILE);
    let embedding_server_constants =
        load_embedding_server_constants(EMBEDDING_SERVER_CONSTANT_SET_FILE);
    let embedding_server_measurement_protocol_sha256 =
        file_sha256(EMBEDDING_SERVER_MEASUREMENT_PROTOCOL_FILE);
    let target = env::var("TARGET").expect("Cargo sets TARGET");
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("Cargo sets CARGO_CFG_TARGET_OS");
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").expect("Cargo sets CARGO_CFG_TARGET_ARCH");
    let compiled_backends: &[&str] = match target_os.as_str() {
        "macos" => &["cpu", "metal"],
        "windows" | "linux" => &["cpu", "vulkan"],
        _ => &["cpu"],
    };
    let (engine_linkage, backend_loading) = match target_os.as_str() {
        "windows" | "linux" => ("dynamic", "runtime-modules"),
        _ => ("static", "builtin"),
    };
    let model_source = resolve_model_source();
    let model_embedded = model_source.is_some();
    let embedding_contract_sha256 = embedding_contract_digest(&contract);
    let ggml_build_identity = format!(
        "codestory-native-engine-v1|target={target}|os={target_os}|arch={target_arch}|linkage={engine_linkage}|backend_loading={backend_loading}|backends={}|llama_cpp_crate={}|llama_cpp_commit={}|model_sha256={}|embedding_contract_sha256={embedding_contract_sha256}|model_embedded={model_embedded}|producer={}@{}|end",
        compiled_backends.join(","),
        contract.llama_cpp_crate_version,
        contract.llama_cpp_source_commit,
        contract.sha256,
        contract.producer_name,
        contract.producer_version,
    );
    let product_embedding_runtime_id = format!(
        "{}:sha256-{}:llama.cpp-{}:producer-{}@{}",
        contract.embedding_family,
        contract.sha256,
        contract.llama_cpp_source_commit,
        contract.producer_name,
        contract.producer_version
    );

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    stage_dynamic_runtime(&target_os, &out_dir);
    fs::write(
        out_dir.join("model_contract.rs"),
        format!(
            "pub const MODEL_FILE_NAME: &str = {:?};\n\
             pub const MODEL_SIZE: u64 = {};\n\
             pub const MODEL_SHA256: &str = {:?};\n\
             pub const LLAMA_CPP_CRATE_VERSION: &str = {:?};\n\
             pub const LLAMA_CPP_SOURCE_COMMIT: &str = {:?};\n\
             const EMBEDDING_DIMENSION: usize = {};\n\
             pub const MODEL_TOKENIZER_SHA256: &str = {:?};\n\
             pub const MODEL_CONFIG_SHA256: &str = {:?};\n\
             pub const MODEL_PRODUCER_NAME: &str = {:?};\n\
             pub const MODEL_PRODUCER_VERSION: &str = {:?};\n\
             pub const MODEL_LICENSE_SPDX_ID: &str = {:?};\n\
             pub const MODEL_LICENSE_SOURCE_URL: &str = {:?};\n\
             pub const NATIVE_ENGINE_BUILD_CONTRACT_SCHEMA_VERSION: u32 = 2;\n\
             pub const NATIVE_ENGINE_TARGET_TRIPLE: &str = {target:?};\n\
             pub const NATIVE_ENGINE_TARGET_OS: &str = {target_os:?};\n\
             pub const NATIVE_ENGINE_TARGET_ARCH: &str = {target_arch:?};\n\
             pub const NATIVE_ENGINE_LINKAGE: &str = {engine_linkage:?};\n\
             pub const NATIVE_ENGINE_BACKEND_LOADING: &str = {backend_loading:?};\n\
             pub const NATIVE_ENGINE_COMPILED_BACKENDS: &[&str] = &{compiled_backends:?};\n\
             pub const NATIVE_ENGINE_EMBEDDING_CONTRACT_SHA256: &str = {embedding_contract_sha256:?};\n\
             pub const GGML_BUILD_IDENTITY: &str = {ggml_build_identity:?};\n\
             pub const PRODUCT_EMBEDDING_RUNTIME_ID: &str = {product_embedding_runtime_id:?};\n",
            contract.file_name,
            contract.size,
            contract.sha256,
            contract.llama_cpp_crate_version,
            contract.llama_cpp_source_commit,
            contract.dimension,
            contract.tokenizer_sha256,
            contract.config_sha256,
            contract.producer_name,
            contract.producer_version,
            contract.license_spdx_id,
            contract.license_source_url,
        ),
    )
    .expect("write embedding model contract");
    let embedding_server_proof_marker = format!(
        "codestory-embedding-server-proof-v1|bootstrap=1|protocol_schema=1|protocol_sha256={embedding_server_protocol_sha256}|constant_set_sha256={embedding_server_constant_set_sha256}|measurement_protocol_sha256={embedding_server_measurement_protocol_sha256}|clock_policy=awake_monotonic|query_capacity=64|bulk_capacity=64|idle_timeout_ms=60000|end"
    );
    fs::write(
        out_dir.join("embedding_server_contract.rs"),
        format!(
            "pub const PER_USER_EMBEDDING_PROTOCOL_SHA256: &str = {embedding_server_protocol_sha256:?};\n\
             pub const PER_USER_EMBEDDING_CONSTANT_SET_SHA256: &str = {embedding_server_constant_set_sha256:?};\n\
             pub const PER_USER_EMBEDDING_MEASUREMENT_PROTOCOL_SHA256: &str = {embedding_server_measurement_protocol_sha256:?};\n\
             pub const PER_USER_EMBEDDING_CONSTANT_SET_FROZEN: bool = {constant_set_frozen};\n\
             pub const PER_USER_EMBEDDING_CONNECT_TIMEOUT_MS: u64 = {connect_timeout_ms};\n\
             pub const PER_USER_EMBEDDING_SPAWN_CONVERGENCE_TIMEOUT_MS: u64 = {spawn_convergence_timeout_ms};\n\
             pub const PER_USER_EMBEDDING_RETRY_AFTER_MS: u64 = {retry_after_ms};\n\
             pub const PER_USER_EMBEDDING_QUERY_REQUEST_DEADLINE_MS: u64 = {query_request_deadline_ms};\n\
             pub const PER_USER_EMBEDDING_BULK_REPLAY_SUCCESS_BUDGET_MS: u64 = {bulk_replay_success_budget_ms};\n\
             pub const PER_USER_EMBEDDING_BULK_REQUEST_DEADLINE_MS: u64 = {bulk_request_deadline_ms};\n\
             pub const PER_USER_EMBEDDING_HARD_NATIVE_NO_PROGRESS_MS: u64 = {hard_native_no_progress_ms};\n\
             pub const PER_USER_EMBEDDING_WATCHDOG_CADENCE_MS: u64 = {watchdog_cadence_ms};\n\
             pub const PER_USER_EMBEDDING_ELECTION_INITIAL_BACKOFF_MS: Option<u64> = {election_initial_backoff_ms:?};\n\
             pub const PER_USER_EMBEDDING_ELECTION_MAXIMUM_BACKOFF_MS: Option<u64> = {election_maximum_backoff_ms:?};\n\
             #[used]\n\
             pub static EMBEDDING_SERVER_PROOF_MARKER: &[u8] = {embedding_server_proof_marker:?}.as_bytes();\n",
            constant_set_frozen = embedding_server_constants.frozen,
            connect_timeout_ms = embedding_server_constants.connect_timeout_ms,
            spawn_convergence_timeout_ms =
                embedding_server_constants.spawn_convergence_timeout_ms,
            retry_after_ms = embedding_server_constants.retry_after_ms,
            query_request_deadline_ms =
                embedding_server_constants.query_request_deadline_ms,
            bulk_replay_success_budget_ms =
                embedding_server_constants.bulk_replay_success_budget_ms,
            bulk_request_deadline_ms =
                embedding_server_constants.bulk_request_deadline_ms,
            hard_native_no_progress_ms =
                embedding_server_constants.hard_native_no_progress_ms,
            watchdog_cadence_ms = embedding_server_constants.watchdog_cadence_ms,
            election_initial_backoff_ms =
                embedding_server_constants.election_initial_backoff_ms,
            election_maximum_backoff_ms =
                embedding_server_constants.election_maximum_backoff_ms,
        ),
    )
    .expect("write embedding server proof contract");

    let generated = out_dir.join("embedded_model.rs");
    match model_source {
        Some(source) => {
            println!("cargo:rerun-if-changed={}", source.display());
            let expected_model = ExpectedModel {
                size: contract.size,
                sha256: &contract.sha256,
            };
            verify_model(&source, expected_model).unwrap_or_else(|error| {
                panic!(
                    "invalid embedded model source {}: {error}",
                    source.display()
                )
            });

            let destination = out_dir.join(&contract.file_name);
            stage_model(&source, &destination, expected_model).unwrap_or_else(|error| {
                panic!(
                    "failed to stage embedded model {}: {error}",
                    source.display()
                )
            });
            fs::write(
                &generated,
                format!(
                    "pub static EMBEDDED_MODEL_BYTES: &[u8] = include_bytes!({:?});\n\
                     pub const EMBEDDED_MODEL_COMPILED: bool = true;\n",
                    contract.file_name
                ),
            )
            .expect("write embedded model bindings");
        }
        None => {
            fs::write(
                &generated,
                "pub static EMBEDDED_MODEL_BYTES: &[u8] = &[];\n\
                 pub const EMBEDDED_MODEL_COMPILED: bool = false;\n",
            )
            .expect("write development embedded model bindings");
            println!(
                "cargo:warning=codestory-llama-sys development build has no embedded model; set CODESTORY_EMBED_MODEL_SOURCE to exercise it"
            );
        }
    }
}

fn load_embedding_server_constants(path: &str) -> EmbeddingServerConstants {
    let bytes =
        fs::read(path).unwrap_or_else(|error| panic!("failed to read contract {path}: {error}"));
    let value: Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("failed to parse contract {path}: {error}"));
    assert_eq!(
        value.get("schema_version").and_then(Value::as_u64),
        Some(1),
        "embedding server constant-set schema is unsupported"
    );
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .expect("embedding server constant set omits status");
    assert!(
        matches!(status, "unfrozen" | "frozen"),
        "embedding server constant-set status is invalid"
    );
    let frozen = status == "frozen";
    let selected = if frozen {
        value
            .get("calibration_required_values")
            .and_then(Value::as_object)
            .expect("frozen embedding server constant set omits calibrated values")
    } else {
        value
            .get("draft_values")
            .and_then(Value::as_object)
            .expect("unfrozen embedding server constant set omits draft values")
    };
    let positive = |object: &serde_json::Map<String, Value>, field: &str| {
        let selected = object
            .get(field)
            .and_then(Value::as_u64)
            .unwrap_or_else(|| {
                panic!("embedding server constant {field} is not a positive integer")
            });
        assert!(
            selected > 0,
            "embedding server constant {field} must be positive"
        );
        selected
    };
    let (
        connect_timeout_ms,
        spawn_convergence_timeout_ms,
        retry_after_ms,
        query_request_deadline_ms,
        bulk_replay_success_budget_ms,
        bulk_request_deadline_ms,
        hard_native_no_progress_ms,
        watchdog_cadence_ms,
        election_initial_backoff_ms,
        election_maximum_backoff_ms,
    ) = if frozen {
        let capacity = selected
            .get("capacity_retry_policy")
            .and_then(Value::as_object)
            .expect("frozen capacity_retry_policy is malformed");
        assert_eq!(
            capacity.get("retry_class").and_then(Value::as_str),
            Some("after_capacity_change"),
            "frozen capacity retry class changed"
        );
        let deadlines = selected
            .get("request_deadlines_ms")
            .and_then(Value::as_object)
            .expect("frozen request_deadlines_ms is malformed");
        let election = selected
            .get("election_backoff_policy")
            .and_then(Value::as_object)
            .expect("frozen election_backoff_policy is malformed");
        (
            positive(selected, "connect_timeout_ms"),
            positive(selected, "spawn_convergence_timeout_ms"),
            positive(capacity, "retry_after_ms"),
            positive(deadlines, "query_request_deadline_ms"),
            positive(deadlines, "bulk_replay_success_budget_ms"),
            positive(deadlines, "bulk_request_deadline_ms"),
            positive(selected, "hard_native_no_progress_ms"),
            positive(selected, "watchdog_cadence_ms"),
            Some(positive(election, "initial_backoff_ms")),
            Some(positive(election, "maximum_backoff_ms")),
        )
    } else {
        (
            positive(selected, "connect_timeout_ms"),
            positive(selected, "spawn_convergence_timeout_ms"),
            positive(selected, "retry_after_ms"),
            positive(selected, "query_request_deadline_ms"),
            positive(selected, "bulk_replay_success_budget_ms"),
            positive(selected, "bulk_request_deadline_ms"),
            positive(selected, "hard_native_no_progress_ms"),
            positive(selected, "watchdog_cadence_ms"),
            None,
            None,
        )
    };
    assert!(
        query_request_deadline_ms <= bulk_request_deadline_ms,
        "query request deadline must not exceed the bulk deadline"
    );
    let required_bulk_deadline_ms = hard_native_no_progress_ms
        .checked_add(watchdog_cadence_ms)
        .and_then(|value| value.checked_add(spawn_convergence_timeout_ms))
        .and_then(|value| value.checked_add(bulk_replay_success_budget_ms))
        .expect("embedding server bulk replay deadline calculation overflowed");
    assert!(
        bulk_request_deadline_ms >= required_bulk_deadline_ms,
        "bulk request deadline must cover watchdog detection, replacement convergence, and one successful replay"
    );
    assert!(
        watchdog_cadence_ms < hard_native_no_progress_ms,
        "watchdog cadence must be shorter than the hard no-progress bound"
    );
    if let (Some(initial), Some(maximum)) =
        (election_initial_backoff_ms, election_maximum_backoff_ms)
    {
        assert!(
            initial <= maximum,
            "election initial backoff must not exceed its maximum"
        );
    }
    EmbeddingServerConstants {
        frozen,
        connect_timeout_ms,
        spawn_convergence_timeout_ms,
        retry_after_ms,
        query_request_deadline_ms,
        bulk_replay_success_budget_ms,
        bulk_request_deadline_ms,
        hard_native_no_progress_ms,
        watchdog_cadence_ms,
        election_initial_backoff_ms,
        election_maximum_backoff_ms,
    }
}

fn file_sha256(path: &str) -> String {
    let bytes =
        fs::read(path).unwrap_or_else(|error| panic!("failed to read contract {path}: {error}"));
    format!("{:x}", Sha256::digest(bytes))
}

fn stage_dynamic_runtime(target_os: &str, out_dir: &std::path::Path) {
    if !matches!(target_os, "windows" | "linux") {
        return;
    }

    let backend_dir = PathBuf::from(
        env::var_os("DEP_LLAMA_BACKENDS_DIR")
            .expect("dynamic llama.cpp build must export DEP_LLAMA_BACKENDS_DIR"),
    );
    let native_out = backend_dir
        .parent()
        .expect("llama.cpp backend directory must have an output parent");
    let core_dir = native_out.join(if target_os == "windows" { "bin" } else { "lib" });
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("Cargo OUT_DIR must be nested under a profile directory");
    fs::create_dir_all(profile_dir).expect("create Cargo profile directory");

    let mut runtime_files: Vec<(String, PathBuf, &'static str)> = Vec::new();
    for directory in [&core_dir, &backend_dir] {
        let entries = fs::read_dir(directory).unwrap_or_else(|error| {
            panic!(
                "failed to inspect native runtime directory {}: {error}",
                directory.display()
            )
        });
        for entry in entries {
            let path = entry.expect("read native runtime entry").path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(role) = native_runtime_role(name, target_os) else {
                continue;
            };
            assert!(
                path.is_file(),
                "native runtime artifact must be a file: {}",
                path.display()
            );
            assert!(
                !runtime_files
                    .iter()
                    .any(|(existing, _, _)| existing.eq_ignore_ascii_case(name)),
                "duplicate native runtime artifact: {name}"
            );
            runtime_files.push((name.to_owned(), path, role));
        }
    }

    for required in ["llama_core", "ggml_core", "ggml_base", "cpu", "vulkan"] {
        assert!(
            runtime_files.iter().any(|(_, _, role)| *role == required),
            "dynamic native runtime is missing required {required} artifact"
        );
    }
    runtime_files.sort_by_key(|entry| entry.0.to_lowercase());
    if target_os == "windows" {
        for (name, source, _) in &runtime_files {
            fs::copy(source, profile_dir.join(name)).unwrap_or_else(|error| {
                panic!(
                    "failed to stage native runtime artifact {}: {error}",
                    source.display()
                )
            });
        }
    } else {
        let runtime_sources = runtime_files
            .iter()
            .map(|(_, source, _)| source.as_path())
            .collect::<Vec<_>>();
        let build_support_source = core_dir.join("libllama-common.so");
        stage_linux_shared_libraries(
            &runtime_sources,
            &[build_support_source.as_path()],
            profile_dir,
        )
        .unwrap_or_else(|error| {
            panic!(
                "failed to stage Linux shared runtime from {}: {error}",
                native_out.display()
            )
        });
    }
    let mut file_list = runtime_files
        .iter()
        .map(|(name, _, _)| name.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    file_list.push('\n');
    fs::write(profile_dir.join(NATIVE_RUNTIME_FILE_LIST), file_list)
        .expect("write deterministic native runtime file list");
}

fn native_runtime_role(name: &str, target_os: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let stem = if target_os == "windows" {
        lower.strip_suffix(".dll")?
    } else {
        lower.strip_prefix("lib")?.split_once(".so")?.0
    };
    match stem {
        "llama" => Some("llama_core"),
        "ggml" => Some("ggml_core"),
        "ggml-base" => Some("ggml_base"),
        value if value.starts_with("ggml-cpu") => Some("cpu"),
        value if value.starts_with("ggml-vulkan") => Some("vulkan"),
        _ => None,
    }
}

fn load_model_contract() -> ModelContract {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR"));
    let path = manifest_dir.join(MODEL_CONTRACT_FILE);
    let bytes = fs::read(&path).unwrap_or_else(|error| {
        panic!("failed to read model contract {}: {error}", path.display())
    });
    let value: Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("invalid model contract {}: {error}", path.display()));

    let schema_version = required_u64(&value, "schema_version");
    assert_eq!(
        schema_version, 1,
        "unsupported model contract schema_version"
    );
    let model = required_object(&value, "model");
    let runtime = required_object(&value, "runtime");
    let embedding = required_object(&value, "embedding");
    let tokenizer_config = required_object(&value, "tokenizer_config");
    let producer = required_object(&value, "producer");
    let license = required_object(&value, "license");
    let file_name = required_string(model, "file_name");
    assert!(
        !file_name.is_empty()
            && file_name != "."
            && file_name != ".."
            && !file_name.contains(['/', '\\'])
            && file_name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)),
        "model.file_name must be a safe file name"
    );
    let sha256 = required_string(model, "sha256");
    assert_sha256(&sha256, "model.sha256");
    let tokenizer_sha256 = required_string(tokenizer_config, "tokenizer_sha256");
    assert_sha256(&tokenizer_sha256, "tokenizer_config.tokenizer_sha256");
    let config_sha256 = required_string(tokenizer_config, "config_sha256");
    assert_sha256(&config_sha256, "tokenizer_config.config_sha256");
    let dimension = required_u64(embedding, "dimension");
    assert!(
        usize::try_from(dimension).is_ok(),
        "embedding.dimension exceeds usize"
    );
    let pooling = required_string(embedding, "pooling");
    assert_eq!(pooling, "cls", "unsupported embedding.pooling");
    let normalization = required_string(embedding, "normalization");
    assert_eq!(normalization, "l2", "unsupported embedding.normalization");
    let element_type = required_string(embedding, "element_type");
    assert_eq!(element_type, "f32_le", "unsupported embedding.element_type");
    let vector_schema_version = required_u64(embedding, "vector_schema_version");
    assert!(
        u32::try_from(vector_schema_version).is_ok(),
        "embedding.vector_schema_version exceeds u32"
    );
    assert_eq!(
        tokenizer_sha256,
        contract_digest("tokenizer", &sha256),
        "tokenizer_config.tokenizer_sha256 does not match the model identity"
    );
    assert_eq!(
        config_sha256,
        contract_digest(
            "config",
            &format!("{sha256}:{dimension}:{pooling}:{normalization}")
        ),
        "tokenizer_config.config_sha256 does not match the embedding semantics"
    );
    let container = required_string(tokenizer_config, "container");
    assert_eq!(container, "gguf", "unsupported tokenizer_config.container");
    let producer_name = required_string(producer, "name");
    assert_eq!(
        producer_name,
        env::var("CARGO_PKG_NAME").expect("Cargo sets CARGO_PKG_NAME"),
        "producer.name must match the crate name"
    );
    let producer_version = required_string(producer, "version");
    assert_eq!(
        producer_version,
        env::var("CARGO_PKG_VERSION").expect("Cargo sets CARGO_PKG_VERSION"),
        "producer.version must match the crate version"
    );
    let license_spdx_id = required_string(license, "spdx_id");
    assert_eq!(license_spdx_id, "MIT", "unsupported model license");
    let license_source_url = required_string(license, "source_url");
    assert!(
        license_source_url.starts_with("https://"),
        "license.source_url must use HTTPS"
    );

    ModelContract {
        file_name,
        size: required_u64(model, "size_bytes"),
        sha256,
        embedding_family: required_string(runtime, "embedding_family"),
        llama_cpp_crate_version: required_string(runtime, "llama_cpp_crate_version"),
        llama_cpp_source_commit: required_string(runtime, "llama_cpp_source_commit"),
        dimension,
        query_prefix: required_string(embedding, "query_prefix"),
        document_prefix: required_string_allow_empty(embedding, "document_prefix"),
        pooling,
        normalization,
        element_type,
        vector_schema_version,
        tokenizer_sha256,
        config_sha256,
        producer_name,
        producer_version,
        license_spdx_id,
        license_source_url,
    }
}

fn required_object<'a>(value: &'a Value, name: &str) -> &'a Value {
    value
        .get(name)
        .filter(|value| value.is_object())
        .unwrap_or_else(|| panic!("model contract field {name} must be an object"))
}

fn required_string(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| panic!("model contract field {name} must be a non-empty string"))
        .to_owned()
}

fn required_string_allow_empty(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("model contract field {name} must be a string"))
        .to_owned()
}

fn assert_sha256(value: &str, name: &str) {
    assert!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{name} must be a lowercase SHA-256 digest"
    );
}

fn required_u64(value: &Value, name: &str) -> u64 {
    value
        .get(name)
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .unwrap_or_else(|| panic!("model contract field {name} must be a positive integer"))
}

fn contract_digest(domain: &str, value: &str) -> String {
    ordered_contract_digest(domain, &[value])
}

fn embedding_contract_digest(contract: &ModelContract) -> String {
    let model_size = contract.size.to_string();
    let dimension = contract.dimension.to_string();
    let vector_schema_version = contract.vector_schema_version.to_string();
    ordered_contract_digest(
        "codestory-native-embedding-contract-v1",
        &[
            &contract.file_name,
            &model_size,
            &contract.sha256,
            &contract.embedding_family,
            &dimension,
            &contract.query_prefix,
            &contract.document_prefix,
            &contract.pooling,
            &contract.normalization,
            &contract.element_type,
            &vector_schema_version,
            "gguf",
            &contract.tokenizer_sha256,
            &contract.config_sha256,
        ],
    )
}

fn ordered_contract_digest(domain: &str, values: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for bytes in std::iter::once(domain).chain(values.iter().copied()) {
        let bytes = bytes.as_bytes();
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    format!("{:x}", hasher.finalize())
}

fn resolve_model_source() -> Option<PathBuf> {
    if let Some(source) = env::var_os("CODESTORY_EMBED_MODEL_SOURCE") {
        return Some(PathBuf::from(source));
    }
    if env::var("DEBUG").as_deref() != Ok("false") {
        return None;
    }

    panic!(
        "release builds require CODESTORY_EMBED_MODEL_SOURCE; run scripts/prepare-embedded-model.mjs before Cargo"
    );
}
