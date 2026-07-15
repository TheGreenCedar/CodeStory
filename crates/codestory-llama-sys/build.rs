use sha2::{Digest, Sha256};
use std::env;
use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

const MODEL_FILE_NAME: &str = "coderankembed.Q8_0.gguf";
const MODEL_SIZE: u64 = 146_029_792;
const MODEL_SHA256: &str = "666db8df27c88570cdc07adca28646260038b8ca65354911d57b936ebf56efaa";
const PRODUCT_EMBEDDING_RUNTIME_FAMILY: &str = "inprocess:coderank-embed:q8_0";
const LLAMA_CPP_CRATE_VERSION: &str = "0.1.151";
const LLAMA_CPP_SOURCE_COMMIT: &str = "9e3b928fd8c9d14dbf15a8768b9fdd7e5c721d66";

fn main() {
    println!("cargo:rerun-if-env-changed=CODESTORY_EMBED_MODEL_SOURCE");
    let target = env::var("TARGET").expect("Cargo sets TARGET");
    let backend = match env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("macos") => "metal",
        Ok("windows" | "linux") => "vulkan",
        _ => "cpu",
    };
    let ggml_build_identity = format!(
        "llama-cpp-sys-2@{LLAMA_CPP_CRATE_VERSION}+llama.cpp@{LLAMA_CPP_SOURCE_COMMIT}+{backend}+{target}"
    );
    let product_embedding_runtime_id = format!(
        "{PRODUCT_EMBEDDING_RUNTIME_FAMILY}:sha256-{MODEL_SHA256}:llama.cpp-{LLAMA_CPP_SOURCE_COMMIT}"
    );

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    fs::write(
        out_dir.join("model_contract.rs"),
        format!(
            "pub const MODEL_FILE_NAME: &str = {MODEL_FILE_NAME:?};\n\
             pub const MODEL_SIZE: u64 = {MODEL_SIZE};\n\
             pub const MODEL_SHA256: &str = {MODEL_SHA256:?};\n\
             pub const LLAMA_CPP_CRATE_VERSION: &str = {LLAMA_CPP_CRATE_VERSION:?};\n\
             pub const LLAMA_CPP_SOURCE_COMMIT: &str = {LLAMA_CPP_SOURCE_COMMIT:?};\n\
             pub const GGML_BUILD_IDENTITY: &str = {ggml_build_identity:?};\n\
             pub const PRODUCT_EMBEDDING_RUNTIME_ID: &str = {product_embedding_runtime_id:?};\n"
        ),
    )
    .expect("write embedding model contract");
    let generated = out_dir.join("embedded_model.rs");
    let source = resolve_model_source();

    match source {
        Some(source) => {
            println!("cargo:rerun-if-changed={}", source.display());
            verify_model(&source).unwrap_or_else(|error| {
                panic!(
                    "invalid embedded model source {}: {error}",
                    source.display()
                )
            });

            let destination = out_dir.join(MODEL_FILE_NAME);
            fs::copy(&source, &destination).unwrap_or_else(|error| {
                panic!(
                    "failed to stage embedded model {}: {error}",
                    source.display()
                )
            });
            fs::write(
                &generated,
                format!(
                    "pub static EMBEDDED_MODEL_BYTES: &[u8] = include_bytes!(\"{MODEL_FILE_NAME}\");\n\
                     pub const EMBEDDED_MODEL_COMPILED: bool = true;\n"
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

fn resolve_model_source() -> Option<PathBuf> {
    if let Some(source) = env::var_os("CODESTORY_EMBED_MODEL_SOURCE") {
        return Some(PathBuf::from(source));
    }
    if env::var("DEBUG").as_deref() != Ok("false") {
        return None;
    }

    Some(prepare_release_model())
}

fn prepare_release_model() -> PathBuf {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("codestory-llama-sys lives under the workspace crates directory");
    let script = workspace_root.join("scripts/prepare-embedded-model.mjs");
    println!("cargo:rerun-if-changed={}", script.display());
    let output = Command::new("node")
        .arg(&script)
        .current_dir(workspace_root)
        .output()
        .unwrap_or_else(|error| {
            panic!("failed to start automatic embedded-model preparation with Node.js: {error}")
        });
    if !output.status.success() {
        panic!(
            "automatic embedded-model preparation failed (status {}):\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let source = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    verify_model(&source).unwrap_or_else(|error| {
        panic!(
            "automatic embedded-model preparation produced invalid {}: {error}",
            source.display()
        )
    });
    source
}

fn verify_model(path: &Path) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("path is not a regular file".into());
    }
    if metadata.len() != MODEL_SIZE {
        return Err(format!(
            "size mismatch: expected {MODEL_SIZE} bytes, found {}",
            metadata.len()
        ));
    }

    let digest = sha256_file(path).map_err(|error| error.to_string())?;
    if digest != MODEL_SHA256 {
        return Err(format!(
            "SHA-256 mismatch: expected {MODEL_SHA256}, found {digest}"
        ));
    }
    Ok(())
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
