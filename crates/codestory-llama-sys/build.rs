mod model_staging;

use model_staging::{ExpectedModel, stage_model, verify_model};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::PathBuf;

const MODEL_CONTRACT_FILE: &str = "model-contract.json";

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

fn main() {
    println!("cargo:rerun-if-env-changed=CODESTORY_EMBED_MODEL_SOURCE");
    println!("cargo:rerun-if-changed={MODEL_CONTRACT_FILE}");
    println!("cargo:rerun-if-changed=model_staging.rs");

    let contract = load_model_contract();
    let target = env::var("TARGET").expect("Cargo sets TARGET");
    let backend = match env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("macos") => "metal",
        Ok("windows" | "linux") => "vulkan",
        _ => "cpu",
    };
    let ggml_build_identity = format!(
        "llama-cpp-sys-2@{}+llama.cpp@{}+{backend}+{target}",
        contract.llama_cpp_crate_version, contract.llama_cpp_source_commit
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
    fs::write(
        out_dir.join("model_contract.rs"),
        format!(
            "pub const MODEL_FILE_NAME: &str = {:?};\n\
             pub const MODEL_SIZE: u64 = {};\n\
             pub const MODEL_SHA256: &str = {:?};\n\
             pub const LLAMA_CPP_CRATE_VERSION: &str = {:?};\n\
             pub const LLAMA_CPP_SOURCE_COMMIT: &str = {:?};\n\
             pub const EMBEDDING_DIMENSION: usize = {};\n\
             pub const EMBEDDING_QUERY_PREFIX: &str = {:?};\n\
             pub const EMBEDDING_DOCUMENT_PREFIX: &str = {:?};\n\
             pub const EMBEDDING_POOLING_ID: &str = {:?};\n\
             pub const EMBEDDING_NORMALIZATION_ID: &str = {:?};\n\
             pub const EMBEDDING_ELEMENT_TYPE: &str = {:?};\n\
             pub const EMBEDDING_VECTOR_SCHEMA_VERSION: u32 = {};\n\
             pub const MODEL_TOKENIZER_SHA256: &str = {:?};\n\
             pub const MODEL_CONFIG_SHA256: &str = {:?};\n\
             pub const MODEL_PRODUCER_NAME: &str = {:?};\n\
             pub const MODEL_PRODUCER_VERSION: &str = {:?};\n\
             pub const MODEL_LICENSE_SPDX_ID: &str = {:?};\n\
             pub const MODEL_LICENSE_SOURCE_URL: &str = {:?};\n\
             pub const GGML_BUILD_IDENTITY: &str = {ggml_build_identity:?};\n\
             pub const PRODUCT_EMBEDDING_RUNTIME_ID: &str = {product_embedding_runtime_id:?};\n",
            contract.file_name,
            contract.size,
            contract.sha256,
            contract.llama_cpp_crate_version,
            contract.llama_cpp_source_commit,
            contract.dimension,
            contract.query_prefix,
            contract.document_prefix,
            contract.pooling,
            contract.normalization,
            contract.element_type,
            contract.vector_schema_version,
            contract.tokenizer_sha256,
            contract.config_sha256,
            contract.producer_name,
            contract.producer_version,
            contract.license_spdx_id,
            contract.license_source_url,
        ),
    )
    .expect("write embedding model contract");

    let generated = out_dir.join("embedded_model.rs");
    match resolve_model_source() {
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
    let mut hasher = Sha256::new();
    for bytes in [domain.as_bytes(), value.as_bytes()] {
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
