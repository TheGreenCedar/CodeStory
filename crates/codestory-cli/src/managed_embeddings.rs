use anyhow::{Context, Result, anyhow, bail};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::args::{CliEmbeddingQuant, CliLlamaVariant};

const MANAGED_ONNX_BACKEND_LABEL: &str = "onnx";
const MANAGED_ONNX_PROVIDER: &str = if cfg!(target_os = "windows") {
    "directml"
} else {
    "cpu"
};
const MANAGED_DOC_EMBED_BATCH_SIZE: usize = 2048;
const MANAGED_SEMANTIC_DOC_MAX_TOKENS: usize = 512;
const MANAGED_ONNX_BATCH_TOKENS: usize = 32_768;
const MANAGED_STORED_VECTOR_ENCODING: &str = "int8";
const MANAGED_DIR_NAME: &str = "managed-embeddings";
const MANAGED_POOLED_ONNX_MODEL_NAME: &str = "model_optimized_cls_pool.onnx";
const ONNX_SOURCE_OUTPUT_NAME: &str = "last_hidden_state";
const ONNX_POOLED_OUTPUT_NAME: &str = "sentence_embedding";
const ONNX_CLS_INDEX_NAME: &str = "codestory_cls_index";
const ONNX_CLS_POOL_NODE_NAME: &str = "codestory_cls_pool";
const ENDPOINT_PROBE_TEXT: &str = "codestory managed embeddings health probe";
const ENDPOINT_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

type HttpHeaders = Vec<(String, String)>;
type RawHttpResponse = (u16, HttpHeaders, Vec<u8>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnnxAssetKind {
    Model,
    Tokenizer,
    Auxiliary,
}

#[derive(Debug, Clone, Copy)]
struct OnnxAsset {
    kind: OnnxAssetKind,
    name: &'static str,
    url: &'static str,
    sha256: &'static str,
    size_bytes: u64,
}

#[derive(Debug, Clone)]
struct HttpEndpoint {
    host: String,
    port: u16,
    path: String,
}

const ONNX_ASSETS: &[OnnxAsset] = &[
    OnnxAsset {
        kind: OnnxAssetKind::Model,
        name: "model_optimized.onnx",
        url: "https://huggingface.co/Qdrant/bge-base-en-v1.5-onnx-Q/resolve/main/model_optimized.onnx",
        sha256: "4e556722bc4f65716c544c8a931f1e90fb3f866e5741fd93a96f051d673339c7",
        size_bytes: 217_824_172,
    },
    OnnxAsset {
        kind: OnnxAssetKind::Tokenizer,
        name: "tokenizer.json",
        url: "https://huggingface.co/Qdrant/bge-base-en-v1.5-onnx-Q/resolve/main/tokenizer.json",
        sha256: "d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66",
        size_bytes: 711_396,
    },
    OnnxAsset {
        kind: OnnxAssetKind::Auxiliary,
        name: "tokenizer_config.json",
        url: "https://huggingface.co/Qdrant/bge-base-en-v1.5-onnx-Q/resolve/main/tokenizer_config.json",
        sha256: "0b29c7bfc889e53b36d9dd3e686dd4300f6525110eaa98c76a5dafceb2029f53",
        size_bytes: 1_242,
    },
    OnnxAsset {
        kind: OnnxAssetKind::Auxiliary,
        name: "special_tokens_map.json",
        url: "https://huggingface.co/Qdrant/bge-base-en-v1.5-onnx-Q/resolve/main/special_tokens_map.json",
        sha256: "5d5b662e421ea9fac075174bb0688ee0d9431699900b90662acd44b2a350503a",
        size_bytes: 695,
    },
    OnnxAsset {
        kind: OnnxAssetKind::Auxiliary,
        name: "vocab.txt",
        url: "https://huggingface.co/Qdrant/bge-base-en-v1.5-onnx-Q/resolve/main/vocab.txt",
        sha256: "07eced375cec144d27c900241f3e339478dec958f92fddbc551f295c992038a3",
        size_bytes: 231_508,
    },
    OnnxAsset {
        kind: OnnxAssetKind::Auxiliary,
        name: "config.json",
        url: "https://huggingface.co/Qdrant/bge-base-en-v1.5-onnx-Q/resolve/main/config.json",
        sha256: "86f84a5285de7f1ee673f712387219ef1e261ec27dcd870e793a80f9da1aaa3b",
        size_bytes: 740,
    },
];

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ManagedAssetOutput {
    pub(crate) name: String,
    pub(crate) url: String,
    pub(crate) sha256: String,
    pub(crate) size_bytes: u64,
    pub(crate) path: String,
    pub(crate) installed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ManagedEmbeddingsStatus {
    pub(crate) state: String,
    pub(crate) message: String,
    pub(crate) root: String,
    pub(crate) endpoint: String,
    pub(crate) model: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ManagedManifest {
    onnx_model_path: Option<String>,
    onnx_source_model_path: Option<String>,
    onnx_tokenizer_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ManagedEmbeddingsSetupOutput {
    pub(crate) dry_run: bool,
    pub(crate) root: String,
    pub(crate) backend: String,
    pub(crate) provider: String,
    pub(crate) model: ManagedAssetOutput,
    pub(crate) runtime_model: ManagedAssetOutput,
    pub(crate) tokenizer: ManagedAssetOutput,
    pub(crate) auxiliary_assets: Vec<ManagedAssetOutput>,
    pub(crate) status: ManagedEmbeddingsStatus,
    pub(crate) next_commands: Vec<String>,
}

pub(crate) fn managed_root(cache_override: Option<&Path>) -> Result<PathBuf> {
    if let Some(cache_override) = cache_override {
        return Ok(cache_override.join(MANAGED_DIR_NAME));
    }
    Ok(ProjectDirs::from("dev", "codestory", "codestory")
        .map(|dirs| dirs.cache_dir().join(MANAGED_DIR_NAME))
        .unwrap_or_else(|| {
            std::env::temp_dir()
                .join("codestory")
                .join(MANAGED_DIR_NAME)
        }))
}

pub(crate) fn setup_embeddings(
    root: &Path,
    _quant: CliEmbeddingQuant,
    _variant: CliLlamaVariant,
    dry_run: bool,
    _start_server: bool,
) -> Result<ManagedEmbeddingsSetupOutput> {
    let model = onnx_model_asset();
    let tokenizer = onnx_tokenizer_asset();
    let source_model_path = onnx_asset_path(root, model);
    let runtime_model_path = pooled_onnx_model_path(root);
    let tokenizer_path = onnx_asset_path(root, tokenizer);

    if !dry_run {
        fs::create_dir_all(onnx_models_dir(root))
            .with_context(|| format!("Failed to create {}", onnx_models_dir(root).display()))?;
        for asset in ONNX_ASSETS {
            install_asset(asset.url, &onnx_asset_path(root, asset), asset.sha256)?;
        }
        ensure_pooled_onnx_model(&source_model_path, &runtime_model_path)?;
        write_onnx_manifest(
            root,
            model,
            tokenizer,
            &runtime_model_path,
            &source_model_path,
            &tokenizer_path,
        )?;
    }

    Ok(ManagedEmbeddingsSetupOutput {
        dry_run,
        root: clean_path(root),
        backend: MANAGED_ONNX_BACKEND_LABEL.to_string(),
        provider: MANAGED_ONNX_PROVIDER.to_string(),
        model: managed_onnx_asset_output(root, model),
        runtime_model: managed_pooled_onnx_asset_output(root),
        tokenizer: managed_onnx_asset_output(root, tokenizer),
        auxiliary_assets: ONNX_ASSETS
            .iter()
            .filter(|asset| asset.kind == OnnxAssetKind::Auxiliary)
            .map(|asset| managed_onnx_asset_output(root, asset))
            .collect(),
        status: inspect_status(root),
        next_commands: vec![
            "codestory-cli doctor --project .".to_string(),
            "codestory-cli index --project . --refresh full".to_string(),
        ],
    })
}

pub(crate) fn inspect_status(root: &Path) -> ManagedEmbeddingsStatus {
    if legacy_llamacpp_backend_selected()
        && let Some(url) = explicit_llama_url()
    {
        let state = if embedding_endpoint_ready(&url, None) {
            "external_llama_configured"
        } else {
            "external_llama_unreachable"
        };
        let display_url = redact_url_for_display(&url);
        let message = if state == "external_llama_configured" {
            format!(
                "External llama.cpp endpoint is configured and accepted an embeddings probe at {display_url}."
            )
        } else {
            format!(
                "External llama.cpp endpoint is configured but did not accept an embeddings probe at {display_url}."
            )
        };
        return ManagedEmbeddingsStatus {
            state: state.to_string(),
            message,
            root: clean_path(root),
            endpoint: display_url,
            model: None,
        };
    }

    if legacy_llamacpp_backend_selected() {
        return ManagedEmbeddingsStatus {
            state: "disabled_by_config".to_string(),
            message:
                "Managed ONNX is skipped because embedding env config selects legacy llama.cpp."
                    .to_string(),
            root: clean_path(root),
            endpoint: MANAGED_ONNX_BACKEND_LABEL.to_string(),
            model: None,
        };
    }

    if disabled_by_embedding_env() {
        return ManagedEmbeddingsStatus {
            state: "disabled_by_config".to_string(),
            message:
                "Managed ONNX is skipped because embedding env config selects another backend."
                    .to_string(),
            root: clean_path(root),
            endpoint: MANAGED_ONNX_BACKEND_LABEL.to_string(),
            model: None,
        };
    }

    let model = default_onnx_model_path(root);
    let tokenizer = default_onnx_tokenizer_path(root);
    if model.is_none() || tokenizer.is_none() {
        return ManagedEmbeddingsStatus {
            state: "missing_managed_assets".to_string(),
            message: "Managed ONNX assets are not installed. Run `codestory-cli setup embeddings`."
                .to_string(),
            root: clean_path(root),
            endpoint: MANAGED_ONNX_BACKEND_LABEL.to_string(),
            model: model.as_ref().map(|path| clean_path(path)),
        };
    }

    let model = model.expect("checked model path");
    let tokenizer = tokenizer.expect("checked tokenizer path");
    if let Err(error) = codestory_runtime::probe_onnx_runtime_paths(&model, &tokenizer) {
        return ManagedEmbeddingsStatus {
            state: "managed_onnx_unusable".to_string(),
            message: format!(
                "Managed ONNX assets are installed but failed runtime verification: {error}"
            ),
            root: clean_path(root),
            endpoint: MANAGED_ONNX_BACKEND_LABEL.to_string(),
            model: Some(clean_path(&model)),
        };
    }

    ManagedEmbeddingsStatus {
        state: "managed_onnx_ready".to_string(),
        message: format!(
            "Managed ONNX embeddings are installed with model `{}` and tokenizer `{}`.",
            clean_path(&model),
            clean_path(&tokenizer)
        ),
        root: clean_path(root),
        endpoint: MANAGED_ONNX_BACKEND_LABEL.to_string(),
        model: Some(clean_path(&model)),
    }
}

pub(crate) fn prepare_runtime_if_installed(root: &Path) {
    if disabled_by_embedding_env() || legacy_llamacpp_backend_selected() {
        return;
    }
    if default_onnx_model_path(root).is_some() && default_onnx_tokenizer_path(root).is_some() {
        set_managed_endpoint_env(root);
    }
}

pub(crate) fn render_setup_embeddings_markdown(output: &ManagedEmbeddingsSetupOutput) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Managed Embeddings Setup");
    let _ = writeln!(markdown, "root: `{}`", output.root);
    let _ = writeln!(markdown, "backend: `{}`", output.backend);
    let _ = writeln!(markdown, "provider: `{}`", output.provider);
    let _ = writeln!(markdown, "dry_run: `{}`", output.dry_run);
    let _ = writeln!(markdown);
    let _ = writeln!(
        markdown,
        "- source_model: `{}` ({} bytes)",
        output.model.name, output.model.size_bytes
    );
    let _ = writeln!(markdown, "- source_model_path: `{}`", output.model.path);
    let _ = writeln!(
        markdown,
        "- runtime_model: `{}` ({} bytes)",
        output.runtime_model.name, output.runtime_model.size_bytes
    );
    let _ = writeln!(
        markdown,
        "- runtime_model_path: `{}`",
        output.runtime_model.path
    );
    let _ = writeln!(
        markdown,
        "- tokenizer: `{}` ({} bytes)",
        output.tokenizer.name, output.tokenizer.size_bytes
    );
    let _ = writeln!(markdown, "- tokenizer_path: `{}`", output.tokenizer.path);
    let _ = writeln!(markdown, "- status: `{}`", output.status.state);
    let _ = writeln!(markdown, "- message: {}", output.status.message);
    if !output.next_commands.is_empty() {
        let _ = writeln!(markdown);
        let _ = writeln!(markdown, "next_commands:");
        for command in &output.next_commands {
            let _ = writeln!(markdown, "- `{command}`");
        }
    }
    markdown
}

fn install_asset(url: &str, destination: &Path, expected_sha256: &str) -> Result<()> {
    if destination.exists() {
        verify_sha256(destination, expected_sha256)?;
        return Ok(());
    }
    let Some(parent) = destination.parent() else {
        bail!(
            "Managed asset destination has no parent: {}",
            destination.display()
        );
    };
    fs::create_dir_all(parent).with_context(|| format!("Failed to create {}", parent.display()))?;
    let partial = destination.with_extension("download");
    let curl = trusted_tool_path("curl", &[parent])?;
    let status = Command::new(&curl)
        .arg("--fail")
        .arg("--location")
        .arg("--retry")
        .arg("3")
        .arg("--output")
        .arg(&partial)
        .arg(url)
        .status()
        .with_context(|| {
            format!(
                "Failed to run trusted curl at {} while downloading {url}",
                curl.display()
            )
        })?;
    if !status.success() {
        bail!("curl failed while downloading {url}");
    }
    verify_sha256(&partial, expected_sha256)?;
    fs::rename(&partial, destination).with_context(|| {
        format!(
            "Failed to move downloaded asset {} to {}",
            partial.display(),
            destination.display()
        )
    })?;
    Ok(())
}

fn ensure_pooled_onnx_model(source: &Path, destination: &Path) -> Result<()> {
    if destination.exists()
        && destination
            .metadata()
            .is_ok_and(|metadata| metadata.len() > 0)
    {
        return Ok(());
    }
    let source_bytes =
        fs::read(source).with_context(|| format!("Failed to read {}", source.display()))?;
    let pooled_bytes = derive_cls_pooled_onnx_model(&source_bytes).with_context(|| {
        format!(
            "Failed to derive pooled ONNX model from {}",
            source.display()
        )
    })?;
    let Some(parent) = destination.parent() else {
        bail!(
            "Pooled ONNX destination has no parent: {}",
            destination.display()
        );
    };
    fs::create_dir_all(parent).with_context(|| format!("Failed to create {}", parent.display()))?;
    let partial = destination.with_extension("onnx.partial");
    fs::write(&partial, pooled_bytes)
        .with_context(|| format!("Failed to write {}", partial.display()))?;
    fs::rename(&partial, destination).with_context(|| {
        format!(
            "Failed to move pooled ONNX model {} to {}",
            partial.display(),
            destination.display()
        )
    })?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct ProtoField {
    number: u32,
    wire_type: u8,
    start: usize,
    value_start: usize,
    value_end: usize,
    end: usize,
}

fn derive_cls_pooled_onnx_model(source: &[u8]) -> Result<Vec<u8>> {
    let fields = parse_proto_fields(source)?;
    let mut out = Vec::with_capacity(source.len() + 512);
    let mut rewrote_graph = false;
    for field in fields {
        if field.number == 7 && field.wire_type == 2 {
            let graph = rewrite_graph_for_cls_pooling(&source[field.value_start..field.value_end])?;
            write_len_field(&mut out, 7, &graph);
            rewrote_graph = true;
        } else {
            out.extend_from_slice(&source[field.start..field.end]);
        }
    }
    if !rewrote_graph {
        bail!("ONNX ModelProto did not contain graph field 7");
    }
    Ok(out)
}

fn rewrite_graph_for_cls_pooling(graph: &[u8]) -> Result<Vec<u8>> {
    let fields = parse_proto_fields(graph)?;
    let mut source_output = None;
    let mut already_pooled = false;
    for field in &fields {
        if field.number == 12
            && field.wire_type == 2
            && let Some(name) = value_info_name(&graph[field.value_start..field.value_end])?
        {
            if name == ONNX_POOLED_OUTPUT_NAME {
                already_pooled = true;
            }
            if name == ONNX_SOURCE_OUTPUT_NAME || source_output.is_none() {
                source_output = Some(name);
            }
        }
    }
    if already_pooled && source_output.as_deref() != Some(ONNX_SOURCE_OUTPUT_NAME) {
        return Ok(graph.to_vec());
    }
    let source_output = source_output.ok_or_else(|| anyhow!("ONNX graph has no output"))?;

    let mut out = Vec::with_capacity(graph.len() + 512);
    for field in fields {
        if field.number != 12 {
            out.extend_from_slice(&graph[field.start..field.end]);
        }
    }
    write_len_field(&mut out, 5, &cls_index_initializer());
    write_len_field(&mut out, 1, &cls_pool_node(&source_output));
    write_len_field(&mut out, 12, &sentence_embedding_value_info());
    Ok(out)
}

fn value_info_name(value_info: &[u8]) -> Result<Option<String>> {
    for field in parse_proto_fields(value_info)? {
        if field.number == 1 && field.wire_type == 2 {
            return String::from_utf8(value_info[field.value_start..field.value_end].to_vec())
                .map(Some)
                .context("ONNX ValueInfoProto name was not valid UTF-8");
        }
    }
    Ok(None)
}

fn cls_pool_node(source_output: &str) -> Vec<u8> {
    let mut out = Vec::new();
    write_string_field(&mut out, 1, source_output);
    write_string_field(&mut out, 1, ONNX_CLS_INDEX_NAME);
    write_string_field(&mut out, 2, ONNX_POOLED_OUTPUT_NAME);
    write_string_field(&mut out, 3, ONNX_CLS_POOL_NODE_NAME);
    write_string_field(&mut out, 4, "Gather");
    write_len_field(&mut out, 5, &axis_attribute());
    out
}

fn axis_attribute() -> Vec<u8> {
    let mut out = Vec::new();
    write_string_field(&mut out, 1, "axis");
    write_varint_field(&mut out, 3, 1);
    write_varint_field(&mut out, 20, 2);
    out
}

fn cls_index_initializer() -> Vec<u8> {
    let mut out = Vec::new();
    write_varint_field(&mut out, 2, 7);
    write_string_field(&mut out, 8, ONNX_CLS_INDEX_NAME);
    write_len_field(&mut out, 9, &0_i64.to_le_bytes());
    out
}

fn sentence_embedding_value_info() -> Vec<u8> {
    let mut out = Vec::new();
    write_string_field(&mut out, 1, ONNX_POOLED_OUTPUT_NAME);
    write_len_field(&mut out, 2, &float_tensor_type_proto());
    out
}

fn float_tensor_type_proto() -> Vec<u8> {
    let mut tensor = Vec::new();
    write_varint_field(&mut tensor, 1, 1);
    write_len_field(&mut tensor, 2, &sentence_embedding_shape_proto());

    let mut out = Vec::new();
    write_len_field(&mut out, 1, &tensor);
    out
}

fn sentence_embedding_shape_proto() -> Vec<u8> {
    let mut out = Vec::new();
    write_len_field(&mut out, 1, &dim_param("batch_size"));
    write_len_field(&mut out, 1, &dim_value(768));
    out
}

fn dim_param(value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    write_string_field(&mut out, 2, value);
    out
}

fn dim_value(value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    write_varint_field(&mut out, 1, value);
    out
}

fn parse_proto_fields(bytes: &[u8]) -> Result<Vec<ProtoField>> {
    let mut fields = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let start = offset;
        let tag = read_varint(bytes, &mut offset)?;
        let wire_type = (tag & 0x07) as u8;
        let number = u32::try_from(tag >> 3).context("protobuf field number was too large")?;
        let mut value_start = offset;
        match wire_type {
            0 => {
                let _ = read_varint(bytes, &mut offset)?;
            }
            1 => {
                offset = offset
                    .checked_add(8)
                    .filter(|end| *end <= bytes.len())
                    .ok_or_else(|| anyhow!("truncated fixed64 protobuf field"))?;
            }
            2 => {
                let len = usize::try_from(read_varint(bytes, &mut offset)?)
                    .context("length-delimited protobuf field was too large")?;
                value_start = offset;
                offset = offset
                    .checked_add(len)
                    .filter(|end| *end <= bytes.len())
                    .ok_or_else(|| anyhow!("truncated length-delimited protobuf field"))?;
            }
            5 => {
                offset = offset
                    .checked_add(4)
                    .filter(|end| *end <= bytes.len())
                    .ok_or_else(|| anyhow!("truncated fixed32 protobuf field"))?;
            }
            other => bail!("unsupported protobuf wire type {other}"),
        }
        fields.push(ProtoField {
            number,
            wire_type,
            start,
            value_start,
            value_end: offset,
            end: offset,
        });
    }
    Ok(fields)
}

fn read_varint(bytes: &[u8], offset: &mut usize) -> Result<u64> {
    let mut value = 0_u64;
    let mut shift = 0_u32;
    while *offset < bytes.len() {
        let byte = bytes[*offset];
        *offset += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift >= 64 {
            bail!("protobuf varint was too long");
        }
    }
    bail!("truncated protobuf varint")
}

fn write_string_field(out: &mut Vec<u8>, number: u32, value: &str) {
    write_len_field(out, number, value.as_bytes());
}

fn write_varint_field(out: &mut Vec<u8>, number: u32, value: u64) {
    write_tag(out, number, 0);
    write_varint(out, value);
}

fn write_len_field(out: &mut Vec<u8>, number: u32, value: &[u8]) {
    write_tag(out, number, 2);
    write_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn write_tag(out: &mut Vec<u8>, number: u32, wire_type: u8) {
    write_varint(out, (u64::from(number) << 3) | u64::from(wire_type));
}

fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let actual = sha256_file(path)?;
    if !actual.eq_ignore_ascii_case(expected) {
        bail!(
            "Checksum mismatch for {}: expected {}, got {}",
            path.display(),
            expected,
            actual
        );
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn trusted_tool_path(tool: &str, disallowed_roots: &[&Path]) -> Result<PathBuf> {
    let Some(path_var) = std::env::var_os("PATH") else {
        bail!("PATH is not set; cannot locate trusted `{tool}`");
    };
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for candidate_name in command_candidate_names(tool) {
            let candidate = dir.join(&candidate_name);
            if !candidate.is_file() {
                continue;
            }
            let canonical = fs::canonicalize(&candidate)
                .with_context(|| format!("Failed to resolve {}", candidate.display()))?;
            if path_is_under_disallowed_root(&canonical, disallowed_roots) {
                continue;
            }
            return Ok(canonical);
        }
    }
    bail!("Could not find trusted `{tool}` on PATH outside the project/cache roots")
}

fn command_candidate_names(tool: &str) -> Vec<String> {
    if Path::new(tool).extension().is_some() {
        return vec![tool.to_string()];
    }
    #[cfg(target_os = "windows")]
    {
        let mut names = vec![tool.to_string()];
        let path_ext = std::env::var_os("PATHEXT")
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_string());
        for extension in path_ext.split(';') {
            if extension.trim().is_empty() {
                continue;
            }
            names.push(format!("{tool}{extension}"));
            names.push(format!("{tool}{}", extension.to_ascii_lowercase()));
        }
        names
    }
    #[cfg(not(target_os = "windows"))]
    {
        vec![tool.to_string()]
    }
}

fn path_is_under_disallowed_root(path: &Path, disallowed_roots: &[&Path]) -> bool {
    let current_dir = std::env::current_dir()
        .ok()
        .and_then(|path| fs::canonicalize(path).ok());
    if current_dir
        .as_ref()
        .is_some_and(|root| path.starts_with(root))
    {
        return true;
    }
    disallowed_roots
        .iter()
        .filter_map(|root| fs::canonicalize(root).ok())
        .any(|root| path.starts_with(root))
}

fn write_onnx_manifest(
    root: &Path,
    model: &OnnxAsset,
    tokenizer: &OnnxAsset,
    runtime_model_path: &Path,
    source_model_path: &Path,
    tokenizer_path: &Path,
) -> Result<()> {
    fs::create_dir_all(root).with_context(|| format!("Failed to create {}", root.display()))?;
    let manifest = serde_json::json!({
        "backend": MANAGED_ONNX_BACKEND_LABEL,
        "provider": MANAGED_ONNX_PROVIDER,
        "onnx_model_asset": model.name,
        "onnx_model_sha256": model.sha256,
        "onnx_model_path": clean_path(runtime_model_path),
        "onnx_source_model_path": clean_path(source_model_path),
        "onnx_pooled_output": ONNX_POOLED_OUTPUT_NAME,
        "onnx_tokenizer_asset": tokenizer.name,
        "onnx_tokenizer_sha256": tokenizer.sha256,
        "onnx_tokenizer_path": clean_path(tokenizer_path),
        "onnx_assets": ONNX_ASSETS.iter().map(|asset| asset.name).collect::<Vec<_>>(),
    });
    fs::write(
        root.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).expect("serialize managed embeddings manifest"),
    )
    .with_context(|| format!("Failed to write {}", root.join("manifest.json").display()))?;
    Ok(())
}

fn onnx_model_asset() -> &'static OnnxAsset {
    ONNX_ASSETS
        .iter()
        .find(|asset| asset.kind == OnnxAssetKind::Model)
        .expect("managed ONNX model asset is pinned")
}

fn onnx_tokenizer_asset() -> &'static OnnxAsset {
    ONNX_ASSETS
        .iter()
        .find(|asset| asset.kind == OnnxAssetKind::Tokenizer)
        .expect("managed ONNX tokenizer asset is pinned")
}

fn managed_onnx_asset_output(root: &Path, asset: &OnnxAsset) -> ManagedAssetOutput {
    let path = onnx_asset_path(root, asset);
    ManagedAssetOutput {
        name: asset.name.to_string(),
        url: asset.url.to_string(),
        sha256: asset.sha256.to_string(),
        size_bytes: asset.size_bytes,
        path: clean_path(&path),
        installed: path.exists(),
    }
}

fn managed_pooled_onnx_asset_output(root: &Path) -> ManagedAssetOutput {
    let path = pooled_onnx_model_path(root);
    let size_bytes = fs::metadata(&path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let sha256 = if path.exists() {
        sha256_file(&path).unwrap_or_default()
    } else {
        String::new()
    };
    ManagedAssetOutput {
        name: MANAGED_POOLED_ONNX_MODEL_NAME.to_string(),
        url: format!(
            "derived:{}#{ONNX_CLS_POOL_NODE_NAME}",
            onnx_model_asset().name
        ),
        sha256,
        size_bytes,
        path: clean_path(&path),
        installed: path.exists(),
    }
}

fn onnx_models_dir(root: &Path) -> PathBuf {
    root.join("models").join("bge-base-en-v1.5-onnx-qdrant")
}

fn onnx_asset_path(root: &Path, asset: &OnnxAsset) -> PathBuf {
    onnx_models_dir(root).join(asset.name)
}

fn pooled_onnx_model_path(root: &Path) -> PathBuf {
    onnx_models_dir(root).join(MANAGED_POOLED_ONNX_MODEL_NAME)
}

fn manifest_child_path(root: &Path, raw_path: &str) -> Option<PathBuf> {
    let path = PathBuf::from(raw_path);
    let candidate = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    canonical_child_path(root, &candidate).ok()
}

fn canonical_child_path(root: &Path, path: &Path) -> Result<PathBuf> {
    let root = fs::canonicalize(root)
        .with_context(|| format!("Failed to resolve managed root {}", root.display()))?;
    let path = fs::canonicalize(path)
        .with_context(|| format!("Failed to resolve managed path {}", path.display()))?;
    if !path.starts_with(&root) {
        bail!(
            "Managed path {} is outside managed root {}",
            path.display(),
            root.display()
        );
    }
    Ok(path)
}

fn default_onnx_model_path(root: &Path) -> Option<PathBuf> {
    if let Some(manifest) = read_manifest(root)
        && let Some(model_path) = manifest.onnx_model_path
        && let Some(path) = manifest_child_path(root, &model_path)
        && path.exists()
    {
        return Some(path);
    }
    let pooled_path = pooled_onnx_model_path(root);
    if let Some(path) = canonical_child_path(root, &pooled_path)
        .ok()
        .filter(|path| path.exists())
    {
        return Some(path);
    }
    if let Some(manifest) = read_manifest(root)
        && let Some(source_model_path) = manifest.onnx_source_model_path
        && let Some(path) = manifest_child_path(root, &source_model_path)
        && path.exists()
    {
        return Some(path);
    }
    let path = onnx_asset_path(root, onnx_model_asset());
    canonical_child_path(root, &path)
        .ok()
        .filter(|path| path.exists())
}

fn default_onnx_tokenizer_path(root: &Path) -> Option<PathBuf> {
    if let Some(manifest) = read_manifest(root)
        && let Some(tokenizer_path) = manifest.onnx_tokenizer_path
        && let Some(path) = manifest_child_path(root, &tokenizer_path)
        && path.exists()
    {
        return Some(path);
    }
    let path = onnx_asset_path(root, onnx_tokenizer_asset());
    canonical_child_path(root, &path)
        .ok()
        .filter(|path| path.exists())
}

fn read_manifest(root: &Path) -> Option<ManagedManifest> {
    let path = root.join("manifest.json");
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn disabled_by_embedding_env() -> bool {
    env_value_is_hash("CODESTORY_EMBED_RUNTIME_MODE")
        || env_value_is_hash("CODESTORY_EMBED_BACKEND")
        || matches!(
            std::env::var("CODESTORY_HYBRID_RETRIEVAL_ENABLED")
                .ok()
                .map(|value| value.trim().to_ascii_lowercase()),
            Some(value) if value == "0" || value == "false" || value == "off"
        )
}

fn legacy_llamacpp_backend_selected() -> bool {
    env_value_is_llamacpp("CODESTORY_EMBED_RUNTIME_MODE")
        || env_value_is_llamacpp("CODESTORY_EMBED_BACKEND")
        || (explicit_llama_url().is_some()
            && env_value_is_unset("CODESTORY_EMBED_RUNTIME_MODE")
            && env_value_is_unset("CODESTORY_EMBED_BACKEND"))
}

fn env_value_is_hash(name: &str) -> bool {
    matches!(
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_ascii_lowercase()),
        Some(value) if value == "hash" || value == "hash_projection"
    )
}

fn env_value_is_llamacpp(name: &str) -> bool {
    matches!(
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_ascii_lowercase()),
        Some(value) if matches!(value.as_str(), "llamacpp" | "llama.cpp" | "llama-cpp" | "gguf")
    )
}

fn env_value_is_unset(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().is_empty())
        .unwrap_or(true)
}

fn explicit_llama_url() -> Option<String> {
    std::env::var("CODESTORY_EMBED_LLAMACPP_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn set_managed_endpoint_env(root: &Path) {
    let Some(model_path) = default_onnx_model_path(root) else {
        return;
    };
    let Some(tokenizer_path) = default_onnx_tokenizer_path(root) else {
        return;
    };
    unsafe {
        set_env_default_str("CODESTORY_EMBED_BACKEND", MANAGED_ONNX_BACKEND_LABEL);
        set_env_default_str("CODESTORY_EMBED_ONNX_MODEL", &clean_path(&model_path));
        set_env_default_str(
            "CODESTORY_EMBED_ONNX_TOKENIZER",
            &clean_path(&tokenizer_path),
        );
        set_env_default_str("CODESTORY_EMBED_ONNX_PROVIDER", MANAGED_ONNX_PROVIDER);
        set_env_default(
            "CODESTORY_EMBED_ONNX_BATCH_TOKENS",
            MANAGED_ONNX_BATCH_TOKENS,
        );
        set_env_default(
            "CODESTORY_LLM_DOC_EMBED_BATCH_SIZE",
            MANAGED_DOC_EMBED_BATCH_SIZE,
        );
        set_env_default(
            "CODESTORY_SEMANTIC_DOC_MAX_TOKENS",
            MANAGED_SEMANTIC_DOC_MAX_TOKENS,
        );
        set_env_default_str(
            "CODESTORY_STORED_VECTOR_ENCODING",
            MANAGED_STORED_VECTOR_ENCODING,
        );
    }
}

unsafe fn set_env_default(key: &str, value: usize) {
    if std::env::var_os(key).is_none() {
        unsafe {
            std::env::set_var(key, value.to_string());
        }
    }
}

unsafe fn set_env_default_str(key: &str, value: &str) {
    if std::env::var_os(key).is_none() {
        unsafe {
            std::env::set_var(key, value);
        }
    }
}

pub(crate) fn embedding_endpoint_ready(url: &str, expected_dimension: Option<usize>) -> bool {
    probe_embedding_endpoint(url, expected_dimension).is_ok()
}

fn probe_embedding_endpoint(url: &str, expected_dimension: Option<usize>) -> Result<usize> {
    let endpoint = parse_http_endpoint(url)
        .ok_or_else(|| anyhow!("Managed embedding endpoint must be an http:// URL"))?;
    let request = serde_json::json!({
        "input": [ENDPOINT_PROBE_TEXT],
        "model": "codestory-local-embedding",
    });
    let response = post_json_to_endpoint(&endpoint, &request)?;
    parse_embedding_probe_response(response, expected_dimension)
}

fn post_json_to_endpoint(endpoint: &HttpEndpoint, request: &JsonValue) -> Result<JsonValue> {
    let body =
        serde_json::to_vec(request).context("failed to serialize embedding probe request")?;
    let mut addrs = (endpoint.host.as_str(), endpoint.port)
        .to_socket_addrs()
        .with_context(|| format!("failed to resolve embedding endpoint {}", endpoint.url()))?;
    let mut stream = addrs
        .find_map(|addr| TcpStream::connect_timeout(&addr, ENDPOINT_PROBE_TIMEOUT).ok())
        .ok_or_else(|| anyhow!("failed to connect to embedding endpoint {}", endpoint.url()))?;
    stream.set_read_timeout(Some(ENDPOINT_PROBE_TIMEOUT))?;
    stream.set_write_timeout(Some(ENDPOINT_PROBE_TIMEOUT))?;
    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nAccept: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        endpoint.path,
        endpoint.host,
        endpoint.port,
        body.len()
    );
    stream.write_all(request.as_bytes())?;
    stream.write_all(&body)?;
    stream.flush()?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    let (status_code, headers, body) = split_http_response(&response)?;
    if !(200..300).contains(&status_code) {
        bail!(
            "embedding endpoint {} returned HTTP {status_code}: {}",
            endpoint.url(),
            String::from_utf8_lossy(&body)
        );
    }
    let body = if headers
        .iter()
        .any(|(key, value)| key == "transfer-encoding" && value.contains("chunked"))
    {
        decode_chunked_http_body(&body)?
    } else {
        body
    };
    serde_json::from_slice(&body)
        .with_context(|| format!("failed to parse JSON response from {}", endpoint.url()))
}

fn parse_http_endpoint(url: &str) -> Option<HttpEndpoint> {
    let rest = url.trim().strip_prefix("http://")?;
    let (authority, path) = rest
        .split_once('/')
        .map(|(authority, path)| (authority, format!("/{path}")))
        .unwrap_or((rest, "/v1/embeddings".to_string()));
    let (host, port) = if let Some((host, raw_port)) = authority.rsplit_once(':') {
        (host.to_string(), raw_port.parse::<u16>().ok()?)
    } else {
        (authority.to_string(), 80)
    };
    if host.trim().is_empty() {
        return None;
    }
    Some(HttpEndpoint { host, port, path })
}

impl HttpEndpoint {
    fn url(&self) -> String {
        format!("http://{}:{}{}", self.host, self.port, self.path)
    }
}

fn split_http_response(response: &[u8]) -> Result<RawHttpResponse> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("invalid HTTP response from embedding endpoint"))?;
    let header_text = String::from_utf8_lossy(&response[..header_end]);
    let mut lines = header_text.lines();
    let status_line = lines
        .next()
        .ok_or_else(|| anyhow!("missing HTTP status line from embedding endpoint"))?;
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow!("missing HTTP status code from embedding endpoint"))?
        .parse::<u16>()
        .context("invalid HTTP status code from embedding endpoint")?;
    let headers = lines
        .filter_map(|line| {
            line.split_once(':').map(|(key, value)| {
                (
                    key.trim().to_ascii_lowercase(),
                    value.trim().to_ascii_lowercase(),
                )
            })
        })
        .collect::<Vec<_>>();
    Ok((status_code, headers, response[header_end + 4..].to_vec()))
}

fn decode_chunked_http_body(body: &[u8]) -> Result<Vec<u8>> {
    let mut offset = 0;
    let mut decoded = Vec::new();
    while offset < body.len() {
        let line_end = body[offset..]
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| anyhow!("invalid chunked response from embedding endpoint"))?
            + offset;
        let size_text = String::from_utf8_lossy(&body[offset..line_end]);
        let size_hex = size_text.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_hex, 16)
            .context("invalid chunk size from embedding endpoint")?;
        offset = line_end + 2;
        if size == 0 {
            break;
        }
        if offset + size > body.len() {
            bail!("truncated chunked response from embedding endpoint");
        }
        decoded.extend_from_slice(&body[offset..offset + size]);
        offset += size + 2;
    }
    Ok(decoded)
}

fn parse_embedding_probe_response(
    response: JsonValue,
    expected_dimension: Option<usize>,
) -> Result<usize> {
    let data = response
        .get("data")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| anyhow!("embedding probe response missing `data` array"))?;
    let first = data
        .first()
        .ok_or_else(|| anyhow!("embedding probe response returned no vectors"))?;
    let embedding = first
        .get("embedding")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| anyhow!("embedding probe response item missing `embedding`"))?;
    let dimension = embedding.len();
    if let Some(expected_dimension) = expected_dimension
        && dimension != expected_dimension
    {
        bail!("embedding probe returned dimension {dimension}, expected {expected_dimension}");
    }
    Ok(dimension)
}

pub(crate) fn redact_url_for_display(value: &str) -> String {
    let Some((scheme, rest)) = value.split_once("://") else {
        return value.to_string();
    };
    let rest = rest
        .split_once('#')
        .map(|(before, _)| before)
        .unwrap_or(rest);
    let rest = rest
        .split_once('?')
        .map(|(before, _)| before)
        .unwrap_or(rest);
    let (authority, suffix) = rest.split_once('/').unwrap_or((rest, ""));
    let host_port = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    if suffix.is_empty() {
        format!("{scheme}://{host_port}")
    } else {
        format!("{scheme}://{host_port}/{suffix}")
    }
}

fn clean_path(path: &Path) -> String {
    crate::display::clean_path_string(&path.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn clean_test_path(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    #[test]
    fn setup_dry_run_reports_pinned_onnx_assets_without_writing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("managed");
        let output = setup_embeddings(
            &root,
            CliEmbeddingQuant::Q8_0,
            CliLlamaVariant::Cpu,
            true,
            false,
        )
        .expect("dry run");

        assert_eq!(output.backend, "onnx");
        assert!(output.model.url.contains("Qdrant/bge-base-en-v1.5-onnx-Q"));
        assert_eq!(output.model.name, "model_optimized.onnx");
        assert_eq!(output.runtime_model.name, MANAGED_POOLED_ONNX_MODEL_NAME);
        assert_eq!(
            output.runtime_model.url,
            "derived:model_optimized.onnx#codestory_cls_pool"
        );
        assert!(!output.runtime_model.installed);
        assert_eq!(output.tokenizer.name, "tokenizer.json");
        assert!(!root.exists(), "dry run should not write managed files");
    }

    #[test]
    fn manifest_paths_are_under_managed_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = managed_root(Some(temp.path())).expect("managed root");
        let model_dir = onnx_models_dir(&root);
        fs::create_dir_all(&model_dir).expect("model dir");
        let model = model_dir.join("model_optimized.onnx");
        let tokenizer = model_dir.join("tokenizer.json");
        fs::write(&model, b"model").expect("model");
        fs::write(&tokenizer, b"tokenizer").expect("tokenizer");
        fs::write(
            root.join("manifest.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "onnx_model_path": clean_test_path(&model),
                "onnx_tokenizer_path": clean_test_path(&tokenizer),
            }))
            .expect("manifest"),
        )
        .expect("manifest write");

        assert_eq!(
            default_onnx_model_path(&root).expect("model"),
            fs::canonicalize(&model).expect("canonical model")
        );
        assert_eq!(
            default_onnx_tokenizer_path(&root).expect("tokenizer"),
            fs::canonicalize(&tokenizer).expect("canonical tokenizer")
        );
    }

    #[test]
    fn manifest_paths_outside_managed_root_are_ignored() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = managed_root(Some(temp.path())).expect("managed root");
        let outside = temp.path().join("outside.onnx");
        fs::create_dir_all(&root).expect("root");
        fs::write(&outside, b"outside").expect("outside");
        fs::write(
            root.join("manifest.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "onnx_model_path": clean_test_path(&outside),
                "onnx_tokenizer_path": clean_test_path(&outside),
            }))
            .expect("manifest"),
        )
        .expect("manifest write");

        assert!(default_onnx_model_path(&root).is_none());
        assert!(default_onnx_tokenizer_path(&root).is_none());
    }

    #[test]
    fn endpoint_probe_rejects_wrong_dimension() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let addr = listener.local_addr().expect("addr");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer).expect("read");
            let body = r#"{"data":[{"embedding":[1.0,2.0]}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).expect("write");
        });
        let url = format!("http://{addr}/v1/embeddings");
        assert!(!embedding_endpoint_ready(&url, Some(768)));
        handle.join().expect("server");
    }

    #[test]
    fn redacts_basic_auth_in_urls() {
        assert_eq!(
            redact_url_for_display("http://user:secret@127.0.0.1:8080/v1/embeddings"),
            "http://127.0.0.1:8080/v1/embeddings"
        );
    }

    #[test]
    fn redacts_query_and_fragment_in_urls() {
        assert_eq!(
            redact_url_for_display("https://user:secret@example.test/v1/embeddings?token=abc#frag"),
            "https://example.test/v1/embeddings"
        );
    }

    #[test]
    fn derives_cls_pooled_onnx_graph_output() {
        let graph = minimal_last_hidden_state_graph();
        let mut model = Vec::new();
        write_len_field(&mut model, 7, &graph);

        let derived = derive_cls_pooled_onnx_model(&model).expect("derive pooled model");
        let model_fields = parse_proto_fields(&derived).expect("model fields");
        let graph_field = model_fields
            .iter()
            .find(|field| field.number == 7)
            .expect("graph field");
        let graph_bytes = &derived[graph_field.value_start..graph_field.value_end];
        let graph_fields = parse_proto_fields(graph_bytes).expect("graph fields");

        let outputs = graph_fields
            .iter()
            .filter(|field| field.number == 12)
            .map(|field| {
                value_info_name(&graph_bytes[field.value_start..field.value_end])
                    .expect("value info")
                    .expect("name")
            })
            .collect::<Vec<_>>();
        let node_names = graph_fields
            .iter()
            .filter(|field| field.number == 1)
            .filter_map(|field| {
                node_name(&graph_bytes[field.value_start..field.value_end]).expect("node")
            })
            .collect::<Vec<_>>();
        let initializer_names = graph_fields
            .iter()
            .filter(|field| field.number == 5)
            .filter_map(|field| {
                tensor_name(&graph_bytes[field.value_start..field.value_end]).expect("tensor")
            })
            .collect::<Vec<_>>();

        assert_eq!(outputs, vec![ONNX_POOLED_OUTPUT_NAME]);
        assert!(node_names.contains(&ONNX_CLS_POOL_NODE_NAME.to_string()));
        assert!(initializer_names.contains(&ONNX_CLS_INDEX_NAME.to_string()));
    }

    fn minimal_last_hidden_state_graph() -> Vec<u8> {
        let mut graph = Vec::new();
        write_string_field(&mut graph, 2, "test_graph");
        write_len_field(&mut graph, 12, &last_hidden_state_value_info());
        graph
    }

    fn last_hidden_state_value_info() -> Vec<u8> {
        let mut value_info = Vec::new();
        write_string_field(&mut value_info, 1, ONNX_SOURCE_OUTPUT_NAME);
        value_info
    }

    fn node_name(bytes: &[u8]) -> Result<Option<String>> {
        string_field(bytes, 3)
    }

    fn tensor_name(bytes: &[u8]) -> Result<Option<String>> {
        string_field(bytes, 8)
    }

    fn string_field(bytes: &[u8], number: u32) -> Result<Option<String>> {
        for field in parse_proto_fields(bytes)? {
            if field.number == number && field.wire_type == 2 {
                return String::from_utf8(bytes[field.value_start..field.value_end].to_vec())
                    .map(Some)
                    .context("protobuf string field was not valid UTF-8");
            }
        }
        Ok(None)
    }
}
