//! One registry of every environment variable and configuration-file key
//! CodeStory supports.
//!
//! Every setting is declared exactly once here with its owning module, value
//! shape, audience, and secrecy. Producers import the identity from this
//! registry instead of spelling it, the user-facing configuration reference is
//! generated from the same table, and an architecture contract rejects
//! production sources that spell a registered identity outside its declared
//! owner. That is what keeps a new knob from entering the product without a
//! catalog entry, documentation, or a stated owner the way an ad-hoc
//! `std::env::var` call did.
//!
//! Secrecy is a property of the setting, not of the call site: a secret value
//! is never rendered into documentation, warnings, or diagnostics, so a caller
//! cannot accidentally echo it by reaching for the wrong formatter.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::api::ApiError;

/// Environment identities, declared once. Producers import these constants.
pub const AGENT_PREFLIGHT_LOCAL_REFRESH_TIMEOUT_MS_ENV: &str =
    "CODESTORY_AGENT_PREFLIGHT_LOCAL_REFRESH_TIMEOUT_MS";
pub const ALLOW_PROJECT_NETWORK_CONFIG_ENV: &str = "CODESTORY_ALLOW_PROJECT_NETWORK_CONFIG";
pub const CACHE_ROOT_ENV: &str = "CODESTORY_CACHE_ROOT";
pub const CLI_ENV: &str = "CODESTORY_CLI";
pub const DISABLE_INSTALLED_CLI_PROBE_ENV: &str = "CODESTORY_DISABLE_INSTALLED_CLI_PROBE";
pub const DISABLE_RELEASE_PROBE_ENV: &str = "CODESTORY_DISABLE_RELEASE_PROBE";
pub const EMBED_ALLOW_CPU_ENV: &str = "CODESTORY_EMBED_ALLOW_CPU";
pub const EMBED_QUALIFICATION_DIR_ENV: &str = "CODESTORY_EMBED_QUALIFICATION_DIR";
pub const EMBED_QUALIFICATION_NONCE_ENV: &str = "CODESTORY_EMBED_QUALIFICATION_NONCE";
pub const EVAL_PROBES_ENV: &str = "CODESTORY_EVAL_PROBES";
pub const EVAL_PROBES_MANIFEST_ENV: &str = "CODESTORY_EVAL_PROBES_MANIFEST";
pub const GRAPH_INCLUDE_CALLSITE_IDENTITY_ENV: &str = "CODESTORY_GRAPH_INCLUDE_CALLSITE_IDENTITY";
pub const GRAPH_INCLUDE_CANDIDATE_TARGETS_ENV: &str = "CODESTORY_GRAPH_INCLUDE_CANDIDATE_TARGETS";
pub const GRAPH_INCLUDE_EDGE_CERTAINTY_ENV: &str = "CODESTORY_GRAPH_INCLUDE_EDGE_CERTAINTY";
pub const HYBRID_RETRIEVAL_ENABLED_ENV: &str = "CODESTORY_HYBRID_RETRIEVAL_ENABLED";
pub const INDEX_FRESHNESS_TTL_SECS_ENV: &str = "CODESTORY_INDEX_FRESHNESS_TTL_SECS";
pub const INDEX_GRAPH_LAZY_ENV: &str = "CODESTORY_INDEX_GRAPH_LAZY";
pub const INDEX_LEGACY_DEDUP_ENV: &str = "CODESTORY_INDEX_LEGACY_DEDUP";
pub const INDEX_LEGACY_EDGE_IDENTITY_ENV: &str = "CODESTORY_INDEX_LEGACY_EDGE_IDENTITY";
pub const INDEX_SOURCE_FILE_BYTE_CAP_ENV: &str = "CODESTORY_INDEX_SOURCE_FILE_BYTE_CAP";
pub const INTERNAL_EMBEDDING_SERVER_EXECUTABLE_SHA256_ENV: &str =
    "CODESTORY_INTERNAL_EMBEDDING_SERVER_EXECUTABLE_SHA256";
pub const LATEST_RELEASE_VERSION_ENV: &str = "CODESTORY_LATEST_RELEASE_VERSION";
pub const LLM_DOC_EMBED_BATCH_SIZE_ENV: &str = "CODESTORY_LLM_DOC_EMBED_BATCH_SIZE";
pub const LOG_ENV: &str = "CODESTORY_LOG";
pub const LOG_CORRELATION_ID_ENV: &str = "CODESTORY_LOG_CORRELATION_ID";
pub const NO_TUI_ENV: &str = "CODESTORY_NO_TUI";
pub const PACKET_CANDIDATE_TRACE_ENV: &str = "CODESTORY_PACKET_CANDIDATE_TRACE";
pub const PACKET_STEP_TRACE_OUT_ENV: &str = "CODESTORY_PACKET_STEP_TRACE_OUT";
pub const PIPELINE_FLUSH_ENV: &str = "CODESTORY_PIPELINE_FLUSH";
pub const PLUGIN_ACTIVE_PROJECT_TTL_MS_ENV: &str = "CODESTORY_PLUGIN_ACTIVE_PROJECT_TTL_MS";
pub const PLUGIN_ACTIVE_STATE_PATH_ENV: &str = "CODESTORY_PLUGIN_ACTIVE_STATE_PATH";
pub const PLUGIN_CACHE_VERSION_ENV: &str = "CODESTORY_PLUGIN_CACHE_VERSION";
pub const PLUGIN_CLI_ARCHIVE_SHA256_ENV: &str = "CODESTORY_PLUGIN_CLI_ARCHIVE_SHA256";
pub const PLUGIN_CLI_ARCHIVE_URL_ENV: &str = "CODESTORY_PLUGIN_CLI_ARCHIVE_URL";
pub const PLUGIN_CLI_BUILD_SOURCE_ENV: &str = "CODESTORY_PLUGIN_CLI_BUILD_SOURCE";
pub const PLUGIN_CLI_MANIFEST_PATH_ENV: &str = "CODESTORY_PLUGIN_CLI_MANIFEST_PATH";
pub const PLUGIN_CLI_PATH_ENV: &str = "CODESTORY_PLUGIN_CLI_PATH";
pub const PLUGIN_CLI_PROVISIONED_AT_ENV: &str = "CODESTORY_PLUGIN_CLI_PROVISIONED_AT";
pub const PLUGIN_CLI_REPO_REF_ENV: &str = "CODESTORY_PLUGIN_CLI_REPO_REF";
pub const PLUGIN_CLI_RETENTION_ENV: &str = "CODESTORY_PLUGIN_CLI_RETENTION";
pub const PLUGIN_CLI_SHA256_ENV: &str = "CODESTORY_PLUGIN_CLI_SHA256";
pub const PLUGIN_CLI_SOURCE_ENV: &str = "CODESTORY_PLUGIN_CLI_SOURCE";
pub const PLUGIN_CLI_VERSION_ENV: &str = "CODESTORY_PLUGIN_CLI_VERSION";
pub const PLUGIN_CLI_WARNINGS_ENV: &str = "CODESTORY_PLUGIN_CLI_WARNINGS";
pub const PLUGIN_DATA_ENV: &str = "CODESTORY_PLUGIN_DATA";
pub const PLUGIN_DIRTY_MARKER_PATH_ENV: &str = "CODESTORY_PLUGIN_DIRTY_MARKER_PATH";
pub const PLUGIN_DIRTY_MARKER_PROJECT_ROOT_ENV: &str = "CODESTORY_PLUGIN_DIRTY_MARKER_PROJECT_ROOT";
pub const PLUGIN_LAUNCH_CWD_ENV: &str = "CODESTORY_PLUGIN_LAUNCH_CWD";
pub const PLUGIN_MULTI_PROJECT_ENV: &str = "CODESTORY_PLUGIN_MULTI_PROJECT";
pub const PLUGIN_ROOT_ENV: &str = "CODESTORY_PLUGIN_ROOT";
pub const PLUGIN_RUNTIME_CWD_ENV: &str = "CODESTORY_PLUGIN_RUNTIME_CWD";
pub const PLUGIN_VERSION_ENV: &str = "CODESTORY_PLUGIN_VERSION";
pub const PRECISE_SEMANTIC_SCIP_ARTIFACT_ENV: &str = "CODESTORY_PRECISE_SEMANTIC_SCIP_ARTIFACT";
pub const RESOLUTION_ENABLE_SEMANTIC_ENV: &str = "CODESTORY_RESOLUTION_ENABLE_SEMANTIC";
pub const RESOLUTION_LEGACY_ENV: &str = "CODESTORY_RESOLUTION_LEGACY";
pub const RESOLUTION_LEGACY_MODE_ENV: &str = "CODESTORY_RESOLUTION_LEGACY_MODE";
pub const RESOLUTION_PARALLEL_COMPUTE_ENV: &str = "CODESTORY_RESOLUTION_PARALLEL_COMPUTE";
pub const RESOLUTION_STORE_CANDIDATES_ENV: &str = "CODESTORY_RESOLUTION_STORE_CANDIDATES";
pub const RETRIEVAL_ENV: &str = "CODESTORY_RETRIEVAL";
pub const RETRIEVAL_PROFILE_ENV: &str = "CODESTORY_RETRIEVAL_PROFILE";
pub const RETRIEVAL_RUN_ID_ENV: &str = "CODESTORY_RETRIEVAL_RUN_ID";
pub const RETRIEVAL_SHADOW_ENV: &str = "CODESTORY_RETRIEVAL_SHADOW";
pub const SEMANTIC_DOC_ALIAS_MODE_ENV: &str = "CODESTORY_SEMANTIC_DOC_ALIAS_MODE";
pub const SEMANTIC_DOC_MAX_TOKENS_ENV: &str = "CODESTORY_SEMANTIC_DOC_MAX_TOKENS";
pub const SEMANTIC_DOC_SCOPE_ENV: &str = "CODESTORY_SEMANTIC_DOC_SCOPE";
pub const SEMANTIC_STREAM_PENDING_DOCS_ENV: &str = "CODESTORY_SEMANTIC_STREAM_PENDING_DOCS";
pub const SEMANTIC_STREAM_SORT_WINDOW_BATCHES_ENV: &str =
    "CODESTORY_SEMANTIC_STREAM_SORT_WINDOW_BATCHES";
pub const STDIO_CACHE_ROOT_ENV: &str = "CODESTORY_STDIO_CACHE_ROOT";
pub const STORED_VECTOR_ENCODING_ENV: &str = "CODESTORY_STORED_VECTOR_ENCODING";
pub const SUMMARY_API_KEY_ENV: &str = "CODESTORY_SUMMARY_API_KEY";
pub const SUMMARY_ENDPOINT_ENV: &str = "CODESTORY_SUMMARY_ENDPOINT";
pub const SUMMARY_MAX_TOKENS_ENV: &str = "CODESTORY_SUMMARY_MAX_TOKENS";
pub const SUMMARY_MODEL_ENV: &str = "CODESTORY_SUMMARY_MODEL";
pub const SUMMARY_TIMEOUT_SECS_ENV: &str = "CODESTORY_SUMMARY_TIMEOUT_SECS";
pub const SYMBOL_FULL_TEXT_INDEX_ENV: &str = "CODESTORY_SYMBOL_FULL_TEXT_INDEX";
pub const TEST_EMBED_ALLOW_CPU_ENV: &str = "CODESTORY_TEST_EMBED_ALLOW_CPU";
pub const TEST_PROMOTION_ABORT_SENTINEL_ENV: &str = "CODESTORY_TEST_PROMOTION_ABORT_SENTINEL";

/// Value shape a setting accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    Boolean,
    Integer,
    Path,
    Text,
    Enumerated,
}

impl SettingKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Boolean => "boolean",
            Self::Integer => "integer",
            Self::Path => "path",
            Self::Text => "text",
            Self::Enumerated => "enum",
        }
    }
}

/// Who is expected to set a setting, and therefore where it is documented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SettingAudience {
    /// Supported operator-facing configuration.
    Product,
    /// Set by the plugin launcher or an installed host, not by a user.
    Host,
    /// Diagnostic or rollout switch; unsupported for product use.
    Diagnostic,
    /// Exists only so the test suites can drive a failure shape.
    Testing,
}

impl SettingAudience {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Product => "product",
            Self::Host => "host",
            Self::Diagnostic => "diagnostic",
            Self::Testing => "testing",
        }
    }

    const fn heading(self) -> &'static str {
        match self {
            Self::Product => "Supported environment variables",
            Self::Host => "Host-provided environment variables",
            Self::Diagnostic => "Diagnostic environment variables",
            Self::Testing => "Test-harness environment variables",
        }
    }

    const fn note(self) -> &'static str {
        match self {
            Self::Product => {
                "Operator-facing settings. Environment values win over `.codestory.toml`."
            }
            Self::Host => {
                "The plugin launcher and installed hosts set these. Setting them by hand \
                 misreports provisioning provenance."
            }
            Self::Diagnostic => {
                "Rollout and diagnostic switches. They are not product configuration and may \
                 change or disappear without a compatibility window."
            }
            Self::Testing => {
                "Only the test suites set these. They drive failure shapes that must never \
                 occur in a product run."
            }
        }
    }
}

/// Whether a value may ever be rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingSecrecy {
    Public,
    /// Values are credentials: never render them into docs, warnings, or
    /// diagnostics, only their presence.
    Secret,
}

/// One registered environment setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvSetting {
    /// Process environment variable name.
    pub name: &'static str,
    /// The one production source file allowed to spell this identity.
    pub owner: &'static str,
    pub kind: SettingKind,
    pub audience: SettingAudience,
    pub secrecy: SettingSecrecy,
    pub summary: &'static str,
}

const CLI_APP: &str = "crates/codestory-cli/src/app.rs";
const CLI_CONFIG: &str = "crates/codestory-cli/src/config.rs";
const CLI_DIAGNOSTICS: &str = "crates/codestory-cli/src/diagnostics.rs";
const CLI_EMBEDDING_SERVER_TRANSPORT: &str =
    "crates/codestory-cli/src/embedding_server_transport.rs";
const CLI_EXPLORE: &str = "crates/codestory-cli/src/explore.rs";
const CLI_PREFLIGHT: &str = "crates/codestory-cli/src/app/readiness_commands/preflight.rs";
const CLI_RUNTIME: &str = "crates/codestory-cli/src/runtime.rs";
const CLI_STDIO_TRANSPORT: &str = "crates/codestory-cli/src/stdio_transport.rs";
const INDEXER_LIB: &str = "crates/codestory-indexer/src/lib.rs";
const INDEXER_RESOLUTION: &str = "crates/codestory-indexer/src/resolution/mod.rs";
const RETRIEVAL_CONFIG: &str = "crates/codestory-retrieval/src/config.rs";
const RETRIEVAL_INDEX: &str = "crates/codestory-retrieval/src/index.rs";
const RETRIEVAL_QUALIFICATION: &str =
    "crates/codestory-retrieval/src/per_user_embedding/qualification_control.rs";
const RUNTIME_FRESHNESS: &str = "crates/codestory-runtime/src/index_freshness.rs";
const RUNTIME_GRAPH_DTO: &str = "crates/codestory-runtime/src/graph_dto.rs";
const RUNTIME_ORCHESTRATOR: &str = "crates/codestory-runtime/src/agent/orchestrator.rs";
const RUNTIME_RETRIEVAL_PRIMARY: &str = "crates/codestory-runtime/src/agent/retrieval_primary.rs";
const RUNTIME_SEARCH_ENGINE: &str = "crates/codestory-runtime/src/search/engine.rs";
const RUNTIME_TRACE_EXPORT: &str = "crates/codestory-runtime/src/agent/trace_export.rs";
const STORE_HELPERS: &str = "crates/codestory-store/src/storage_impl/helpers.rs";
const STORE_IMPL: &str = "crates/codestory-store/src/storage_impl/mod.rs";

const fn setting(
    name: &'static str,
    owner: &'static str,
    kind: SettingKind,
    audience: SettingAudience,
    summary: &'static str,
) -> EnvSetting {
    EnvSetting {
        name,
        owner,
        kind,
        audience,
        secrecy: SettingSecrecy::Public,
        summary,
    }
}

const fn secret(
    name: &'static str,
    owner: &'static str,
    kind: SettingKind,
    audience: SettingAudience,
    summary: &'static str,
) -> EnvSetting {
    EnvSetting {
        secrecy: SettingSecrecy::Secret,
        ..setting(name, owner, kind, audience, summary)
    }
}

/// Every environment setting CodeStory reads, ordered by name.
pub const ENV_SETTINGS: &[EnvSetting] = &[
    setting(
        AGENT_PREFLIGHT_LOCAL_REFRESH_TIMEOUT_MS_ENV,
        CLI_PREFLIGHT,
        SettingKind::Integer,
        SettingAudience::Product,
        "Milliseconds `agent preflight` waits for a local refresh before reporting.",
    ),
    setting(
        ALLOW_PROJECT_NETWORK_CONFIG_ENV,
        CLI_CONFIG,
        SettingKind::Boolean,
        SettingAudience::Product,
        "Process-wide opt-in that lets project `.codestory.toml` files set summary network keys.",
    ),
    setting(
        CACHE_ROOT_ENV,
        RETRIEVAL_CONFIG,
        SettingKind::Path,
        SettingAudience::Product,
        "Overrides the per-user cache root that holds every project cache.",
    ),
    setting(
        CLI_ENV,
        CLI_RUNTIME,
        SettingKind::Path,
        SettingAudience::Host,
        "Executable an installed host launches instead of the provisioned CLI.",
    ),
    setting(
        DISABLE_INSTALLED_CLI_PROBE_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Boolean,
        SettingAudience::Diagnostic,
        "Suppresses the installed-CLI probe that status reports as provisioning evidence.",
    ),
    setting(
        DISABLE_RELEASE_PROBE_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Boolean,
        SettingAudience::Diagnostic,
        "Suppresses the latest-release probe and records the probe source as disabled.",
    ),
    setting(
        EMBED_ALLOW_CPU_ENV,
        CLI_APP,
        SettingKind::Boolean,
        SettingAudience::Product,
        "Rejected at startup: CPU embedding is unsupported, so any non-empty value fails closed.",
    ),
    setting(
        EMBED_QUALIFICATION_DIR_ENV,
        RETRIEVAL_QUALIFICATION,
        SettingKind::Path,
        SettingAudience::Diagnostic,
        "Directory the embedding qualification harness uses for its control handshake.",
    ),
    secret(
        EMBED_QUALIFICATION_NONCE_ENV,
        RETRIEVAL_QUALIFICATION,
        SettingKind::Text,
        SettingAudience::Diagnostic,
        "Single-run authentication nonce pairing a qualification worker with its control directory.",
    ),
    setting(
        GRAPH_INCLUDE_CALLSITE_IDENTITY_ENV,
        RUNTIME_GRAPH_DTO,
        SettingKind::Boolean,
        SettingAudience::Diagnostic,
        "Includes call-site identity in graph DTOs (default on).",
    ),
    setting(
        GRAPH_INCLUDE_CANDIDATE_TARGETS_ENV,
        RUNTIME_GRAPH_DTO,
        SettingKind::Boolean,
        SettingAudience::Diagnostic,
        "Includes unresolved candidate targets in graph DTOs (default on).",
    ),
    setting(
        GRAPH_INCLUDE_EDGE_CERTAINTY_ENV,
        RUNTIME_GRAPH_DTO,
        SettingKind::Boolean,
        SettingAudience::Diagnostic,
        "Includes edge certainty in graph DTOs (default on).",
    ),
    setting(
        HYBRID_RETRIEVAL_ENABLED_ENV,
        RETRIEVAL_CONFIG,
        SettingKind::Boolean,
        SettingAudience::Product,
        "Enables hybrid lexical/semantic ranking (default on).",
    ),
    setting(
        INDEX_FRESHNESS_TTL_SECS_ENV,
        RUNTIME_FRESHNESS,
        SettingKind::Integer,
        SettingAudience::Product,
        "Seconds a content-verified freshness verdict is reused before rechecking.",
    ),
    setting(
        INDEX_GRAPH_LAZY_ENV,
        INDEXER_LIB,
        SettingKind::Boolean,
        SettingAudience::Diagnostic,
        "Defers graph rule execution until a projection needs it (default on).",
    ),
    setting(
        INDEX_LEGACY_DEDUP_ENV,
        INDEXER_LIB,
        SettingKind::Boolean,
        SettingAudience::Diagnostic,
        "Restores the superseded edge de-duplication pass.",
    ),
    setting(
        INDEX_LEGACY_EDGE_IDENTITY_ENV,
        INDEXER_LIB,
        SettingKind::Boolean,
        SettingAudience::Diagnostic,
        "Restores the superseded edge identity derivation.",
    ),
    setting(
        INDEX_SOURCE_FILE_BYTE_CAP_ENV,
        CLI_CONFIG,
        SettingKind::Integer,
        SettingAudience::Product,
        "Largest source file, in bytes, the indexer will read; non-positive values keep the default.",
    ),
    setting(
        INTERNAL_EMBEDDING_SERVER_EXECUTABLE_SHA256_ENV,
        CLI_EMBEDDING_SERVER_TRANSPORT,
        SettingKind::Text,
        SettingAudience::Diagnostic,
        "Expected embedding-server executable digest the transport verifies before connecting.",
    ),
    setting(
        LATEST_RELEASE_VERSION_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Text,
        SettingAudience::Host,
        "Latest published version supplied by the host instead of a live release probe.",
    ),
    setting(
        LLM_DOC_EMBED_BATCH_SIZE_ENV,
        RETRIEVAL_CONFIG,
        SettingKind::Integer,
        SettingAudience::Product,
        "Documents per embedding batch during semantic publication.",
    ),
    setting(
        LOG_ENV,
        CLI_DIAGNOSTICS,
        SettingKind::Enumerated,
        SettingAudience::Product,
        "Process log level: `error`, `warn`, `info`, `debug`, or `trace`.",
    ),
    setting(
        LOG_CORRELATION_ID_ENV,
        CLI_DIAGNOSTICS,
        SettingKind::Text,
        SettingAudience::Product,
        "Correlation id stamped on every diagnostic record of this process.",
    ),
    setting(
        NO_TUI_ENV,
        CLI_EXPLORE,
        SettingKind::Boolean,
        SettingAudience::Product,
        "Forces `explore` to render plain output instead of the terminal UI.",
    ),
    setting(
        PACKET_CANDIDATE_TRACE_ENV,
        RUNTIME_ORCHESTRATOR,
        SettingKind::Text,
        SettingAudience::Diagnostic,
        "Symbol filter that turns on packet candidate tracing.",
    ),
    setting(
        PACKET_STEP_TRACE_OUT_ENV,
        RUNTIME_TRACE_EXPORT,
        SettingKind::Path,
        SettingAudience::Diagnostic,
        "File that receives the packet step trace.",
    ),
    setting(
        PIPELINE_FLUSH_ENV,
        INDEXER_LIB,
        SettingKind::Boolean,
        SettingAudience::Diagnostic,
        "Forces a storage flush after every indexing batch.",
    ),
    setting(
        PLUGIN_ACTIVE_PROJECT_TTL_MS_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Integer,
        SettingAudience::Host,
        "Milliseconds a host-recorded active project stays routable.",
    ),
    setting(
        PLUGIN_ACTIVE_STATE_PATH_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Path,
        SettingAudience::Host,
        "File where the host records the active project for stdio routing.",
    ),
    setting(
        PLUGIN_CACHE_VERSION_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Text,
        SettingAudience::Host,
        "Plugin cache layout version reported with provisioning status.",
    ),
    setting(
        PLUGIN_CLI_ARCHIVE_SHA256_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Text,
        SettingAudience::Host,
        "Digest of the archive the host expanded to provision the CLI.",
    ),
    setting(
        PLUGIN_CLI_ARCHIVE_URL_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Text,
        SettingAudience::Host,
        "Archive URL the host downloaded to provision the CLI.",
    ),
    setting(
        PLUGIN_CLI_BUILD_SOURCE_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Text,
        SettingAudience::Host,
        "Build source the host used when it provisioned the CLI from source.",
    ),
    setting(
        PLUGIN_CLI_MANIFEST_PATH_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Path,
        SettingAudience::Host,
        "Provisioning manifest the host wrote beside the installed CLI.",
    ),
    setting(
        PLUGIN_CLI_PATH_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Path,
        SettingAudience::Host,
        "Installed CLI executable the host provisioned.",
    ),
    setting(
        PLUGIN_CLI_PROVISIONED_AT_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Text,
        SettingAudience::Host,
        "Timestamp recorded when the host provisioned the CLI.",
    ),
    setting(
        PLUGIN_CLI_REPO_REF_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Text,
        SettingAudience::Host,
        "Repository ref the host built the CLI from.",
    ),
    setting(
        PLUGIN_CLI_RETENTION_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Text,
        SettingAudience::Host,
        "Retention policy the host applied to previously provisioned CLIs.",
    ),
    setting(
        PLUGIN_CLI_SHA256_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Text,
        SettingAudience::Host,
        "Digest of the installed CLI executable.",
    ),
    setting(
        PLUGIN_CLI_SOURCE_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Text,
        SettingAudience::Host,
        "How the host obtained the CLI: release archive, local build, or explicit path.",
    ),
    setting(
        PLUGIN_CLI_VERSION_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Text,
        SettingAudience::Host,
        "Version of the CLI the host provisioned.",
    ),
    setting(
        PLUGIN_CLI_WARNINGS_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Text,
        SettingAudience::Host,
        "Warnings the host recorded while provisioning the CLI.",
    ),
    setting(
        PLUGIN_DATA_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Path,
        SettingAudience::Host,
        "Host-owned data directory that anchors plugin state files.",
    ),
    setting(
        PLUGIN_DIRTY_MARKER_PATH_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Path,
        SettingAudience::Host,
        "File the host touches when a repository changed outside CodeStory.",
    ),
    setting(
        PLUGIN_DIRTY_MARKER_PROJECT_ROOT_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Path,
        SettingAudience::Host,
        "Project root the dirty marker belongs to.",
    ),
    setting(
        PLUGIN_LAUNCH_CWD_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Path,
        SettingAudience::Host,
        "Working directory the host launched the plugin from.",
    ),
    setting(
        PLUGIN_MULTI_PROJECT_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Boolean,
        SettingAudience::Host,
        "Declares that the host drives one stdio process across several repositories.",
    ),
    setting(
        PLUGIN_ROOT_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Path,
        SettingAudience::Host,
        "Installed plugin root the host launched.",
    ),
    setting(
        PLUGIN_RUNTIME_CWD_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Path,
        SettingAudience::Host,
        "Working directory the runtime binary should adopt.",
    ),
    setting(
        PLUGIN_VERSION_ENV,
        CLI_STDIO_TRANSPORT,
        SettingKind::Text,
        SettingAudience::Host,
        "Version of the installed plugin package.",
    ),
    setting(
        PRECISE_SEMANTIC_SCIP_ARTIFACT_ENV,
        RETRIEVAL_INDEX,
        SettingKind::Path,
        SettingAudience::Diagnostic,
        "Externally produced SCIP artifact used instead of the built one.",
    ),
    setting(
        RESOLUTION_ENABLE_SEMANTIC_ENV,
        INDEXER_RESOLUTION,
        SettingKind::Boolean,
        SettingAudience::Diagnostic,
        "Enables semantic resolution passes.",
    ),
    setting(
        RESOLUTION_LEGACY_ENV,
        INDEXER_RESOLUTION,
        SettingKind::Boolean,
        SettingAudience::Diagnostic,
        "Restores the superseded resolution pipeline.",
    ),
    setting(
        RESOLUTION_LEGACY_MODE_ENV,
        INDEXER_RESOLUTION,
        SettingKind::Boolean,
        SettingAudience::Diagnostic,
        "Selects the superseded resolution mode inside the legacy pipeline.",
    ),
    setting(
        RESOLUTION_PARALLEL_COMPUTE_ENV,
        INDEXER_RESOLUTION,
        SettingKind::Boolean,
        SettingAudience::Diagnostic,
        "Computes resolution candidates in parallel.",
    ),
    setting(
        RESOLUTION_STORE_CANDIDATES_ENV,
        INDEXER_RESOLUTION,
        SettingKind::Boolean,
        SettingAudience::Diagnostic,
        "Persists unresolved resolution candidates for inspection.",
    ),
    setting(
        RETRIEVAL_ENV,
        RUNTIME_RETRIEVAL_PRIMARY,
        SettingKind::Boolean,
        SettingAudience::Diagnostic,
        "`1` requires published retrieval for packet and search; `0` is unsupported and fails closed.",
    ),
    setting(
        RETRIEVAL_PROFILE_ENV,
        RETRIEVAL_CONFIG,
        SettingKind::Enumerated,
        SettingAudience::Product,
        "Cache namespace profile: `local`/`dev` or `agent`/`ci`.",
    ),
    setting(
        RETRIEVAL_RUN_ID_ENV,
        RETRIEVAL_CONFIG,
        SettingKind::Text,
        SettingAudience::Product,
        "Run label that separates agent-profile cache namespaces.",
    ),
    setting(
        RETRIEVAL_SHADOW_ENV,
        RUNTIME_RETRIEVAL_PRIMARY,
        SettingKind::Boolean,
        SettingAudience::Diagnostic,
        "Runs published retrieval beside the incumbent path for comparison.",
    ),
    setting(
        SEMANTIC_DOC_ALIAS_MODE_ENV,
        RETRIEVAL_CONFIG,
        SettingKind::Enumerated,
        SettingAudience::Product,
        "Alias expansion mode for semantic documents.",
    ),
    setting(
        SEMANTIC_DOC_MAX_TOKENS_ENV,
        RETRIEVAL_CONFIG,
        SettingKind::Integer,
        SettingAudience::Product,
        "Token budget per semantic document.",
    ),
    setting(
        SEMANTIC_DOC_SCOPE_ENV,
        RETRIEVAL_CONFIG,
        SettingKind::Enumerated,
        SettingAudience::Product,
        "Which symbols receive semantic documents.",
    ),
    setting(
        SEMANTIC_STREAM_PENDING_DOCS_ENV,
        RETRIEVAL_CONFIG,
        SettingKind::Boolean,
        SettingAudience::Product,
        "Streams pending semantic documents instead of materializing them first.",
    ),
    setting(
        SEMANTIC_STREAM_SORT_WINDOW_BATCHES_ENV,
        RETRIEVAL_CONFIG,
        SettingKind::Integer,
        SettingAudience::Product,
        "Batches held in the semantic streaming sort window.",
    ),
    setting(
        STDIO_CACHE_ROOT_ENV,
        CLI_CONFIG,
        SettingKind::Path,
        SettingAudience::Product,
        "Cache root captured once for a multi-project stdio process.",
    ),
    setting(
        STORED_VECTOR_ENCODING_ENV,
        STORE_HELPERS,
        SettingKind::Enumerated,
        SettingAudience::Product,
        "Encoding used for vectors persisted in the core database.",
    ),
    secret(
        SUMMARY_API_KEY_ENV,
        RETRIEVAL_CONFIG,
        SettingKind::Text,
        SettingAudience::Product,
        "Credential for the symbol summary endpoint.",
    ),
    setting(
        SUMMARY_ENDPOINT_ENV,
        RETRIEVAL_CONFIG,
        SettingKind::Text,
        SettingAudience::Product,
        "Symbol summary endpoint URL; unset disables summary generation.",
    ),
    setting(
        SUMMARY_MAX_TOKENS_ENV,
        RETRIEVAL_CONFIG,
        SettingKind::Integer,
        SettingAudience::Product,
        "Token ceiling for one symbol summary request.",
    ),
    setting(
        SUMMARY_MODEL_ENV,
        RETRIEVAL_CONFIG,
        SettingKind::Text,
        SettingAudience::Product,
        "Model name sent to the symbol summary endpoint.",
    ),
    setting(
        SUMMARY_TIMEOUT_SECS_ENV,
        RETRIEVAL_CONFIG,
        SettingKind::Integer,
        SettingAudience::Product,
        "Seconds a symbol summary request may take, clamped to 1-300.",
    ),
    setting(
        SYMBOL_FULL_TEXT_INDEX_ENV,
        RUNTIME_SEARCH_ENGINE,
        SettingKind::Boolean,
        SettingAudience::Diagnostic,
        "Builds the symbol full-text index (default on).",
    ),
    setting(
        TEST_EMBED_ALLOW_CPU_ENV,
        RETRIEVAL_CONFIG,
        SettingKind::Boolean,
        SettingAudience::Testing,
        "Test-support builds only: exercises CPU-shaped embedding failures.",
    ),
    setting(
        TEST_PROMOTION_ABORT_SENTINEL_ENV,
        STORE_IMPL,
        SettingKind::Path,
        SettingAudience::Testing,
        "Sentinel path that aborts a promotion mid-flight to prove crash recovery.",
    ),
];

/// Ordered audiences as the generated reference renders them.
const AUDIENCES: [SettingAudience; 4] = [
    SettingAudience::Product,
    SettingAudience::Host,
    SettingAudience::Diagnostic,
    SettingAudience::Testing,
];

/// Registered setting for one environment variable name.
pub fn env_setting(name: &str) -> Option<&'static EnvSetting> {
    ENV_SETTINGS.iter().find(|setting| setting.name == name)
}

/// Production source file allowed to spell one environment identity.
pub fn env_setting_owner(name: &str) -> Option<&'static str> {
    env_setting(name).map(|setting| setting.owner)
}

/// Presence of a registered setting in the process environment, with the value
/// withheld whenever the registry marks it secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservedSetting {
    /// The setting is not present in the environment.
    Unset,
    /// The setting is present and safe to display.
    Set(String),
    /// The setting is present and its value is a credential.
    SetSecret,
}

/// Observe one registered setting without ever exposing a secret value.
///
/// This is the only read path a non-owner may use, so a diagnostic surface
/// cannot echo a credential by reaching for `std::env::var` directly.
pub fn observe_env_setting(name: &str) -> ObservedSetting {
    let Some(setting) = env_setting(name) else {
        return ObservedSetting::Unset;
    };
    let Ok(value) = std::env::var(setting.name) else {
        return ObservedSetting::Unset;
    };
    if value.trim().is_empty() {
        return ObservedSetting::Unset;
    }
    match setting.secrecy {
        SettingSecrecy::Secret => ObservedSetting::SetSecret,
        SettingSecrecy::Public => ObservedSetting::Set(value),
    }
}

/// Trust required before a configuration-file key is honoured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigKeyTrust {
    /// Any `.codestory.toml`, including one committed to a repository.
    AnyFile,
    /// User home file only; a project file naming it fails closed.
    UserHomeOnly,
    /// Project files need the process-wide network opt-in.
    ProjectNeedsNetworkOptIn,
}

impl ConfigKeyTrust {
    const fn as_doc(self) -> &'static str {
        match self {
            Self::AnyFile => "user home or project",
            Self::UserHomeOnly => "user home only",
            Self::ProjectNeedsNetworkOptIn => "user home; project needs the network opt-in",
        }
    }
}

/// One registered `.codestory.toml` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigFileKey {
    pub key: &'static str,
    pub kind: SettingKind,
    pub trust: ConfigKeyTrust,
    pub summary: &'static str,
}

/// Key carrying the configuration schema version. Absent means version 1.
pub const CONFIG_SCHEMA_VERSION_KEY: &str = "schema_version";

/// Schema version assumed when a file does not declare one.
pub const CONFIG_SCHEMA_CURRENT_VERSION: u32 = 1;

/// Highest schema version this build understands.
pub const CONFIG_SCHEMA_MAX_SUPPORTED_VERSION: u32 = 2;

/// Typed error code for a configuration schema this build cannot interpret.
pub const UNSUPPORTED_CONFIG_SCHEMA_CODE: &str = "unsupported_config_schema";

/// Typed error code for an unknown key under a schema that rejects them.
pub const UNKNOWN_CONFIG_KEY_CODE: &str = "unknown_config_key";

/// Repository-relative path of the generated configuration reference.
pub const CONFIGURATION_REFERENCE_PATH: &str = "docs/users/configuration-reference.md";

/// Every `.codestory.toml` key CodeStory honours, ordered by key.
pub const CONFIG_FILE_KEYS: &[ConfigFileKey] = &[
    ConfigFileKey {
        key: "cache_dir",
        kind: SettingKind::Path,
        trust: ConfigKeyTrust::UserHomeOnly,
        summary: "Cache root for every project this process opens.",
    },
    ConfigFileKey {
        key: "hybrid_retrieval_enabled",
        kind: SettingKind::Boolean,
        trust: ConfigKeyTrust::AnyFile,
        summary: "Enables hybrid lexical/semantic ranking.",
    },
    ConfigFileKey {
        key: CONFIG_SCHEMA_VERSION_KEY,
        kind: SettingKind::Integer,
        trust: ConfigKeyTrust::AnyFile,
        summary: "Configuration schema version this file is written against.",
    },
    ConfigFileKey {
        key: "semantic_doc_alias_mode",
        kind: SettingKind::Enumerated,
        trust: ConfigKeyTrust::AnyFile,
        summary: "Alias expansion mode for semantic documents.",
    },
    ConfigFileKey {
        key: "semantic_doc_scope",
        kind: SettingKind::Enumerated,
        trust: ConfigKeyTrust::AnyFile,
        summary: "Which symbols receive semantic documents.",
    },
    ConfigFileKey {
        key: "summary_endpoint",
        kind: SettingKind::Text,
        trust: ConfigKeyTrust::ProjectNeedsNetworkOptIn,
        summary: "Symbol summary endpoint URL.",
    },
    ConfigFileKey {
        key: "summary_model",
        kind: SettingKind::Text,
        trust: ConfigKeyTrust::ProjectNeedsNetworkOptIn,
        summary: "Model name sent to the symbol summary endpoint.",
    },
];

/// Whether a key is registered for `.codestory.toml`.
pub fn is_registered_config_key(key: &str) -> bool {
    CONFIG_FILE_KEYS.iter().any(|entry| entry.key == key)
}

/// What a schema version says to do with keys the build does not know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownKeyPolicy {
    /// Warn, naming the keys only, and continue.
    Warn,
    /// Fail closed.
    Reject,
}

/// A configuration file declaring a schema this build cannot interpret.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "configuration schema version {requested} is newer than the supported maximum {max_supported}"
)]
pub struct UnsupportedConfigSchema {
    pub requested: u64,
    pub max_supported: u32,
}

impl UnsupportedConfigSchema {
    /// Typed API error carrying the `unsupported_config_schema` code.
    pub fn to_api_error(&self, source_display: &str) -> ApiError {
        ApiError::new(
            UNSUPPORTED_CONFIG_SCHEMA_CODE,
            format!(
                "{source_display} declares {CONFIG_SCHEMA_VERSION_KEY} {} but this build supports \
                 at most {}; upgrade CodeStory or lower the declared version",
                self.requested, self.max_supported
            ),
        )
    }
}

/// Unknown-key behaviour for one declared schema version.
///
/// Version 1 warns so existing files keep loading, version 2 rejects, and any
/// higher version is unreadable rather than guessed at.
pub fn unknown_key_policy(version: u64) -> Result<UnknownKeyPolicy, UnsupportedConfigSchema> {
    match version {
        0 | 1 => Ok(UnknownKeyPolicy::Warn),
        2 => Ok(UnknownKeyPolicy::Reject),
        requested => Err(UnsupportedConfigSchema {
            requested,
            max_supported: CONFIG_SCHEMA_MAX_SUPPORTED_VERSION,
        }),
    }
}

/// Unregistered keys among the top-level keys of one configuration file.
pub fn unknown_config_keys<'a>(keys: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut unknown = keys
        .into_iter()
        .filter(|key| !is_registered_config_key(key))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    unknown.sort();
    unknown
}

/// Warning text for unknown keys under a schema version that tolerates them.
///
/// The signature takes key names only, so no call site can widen this into a
/// value echo: a key named `summary_api_key` in a repository-controlled file
/// must never put its value in a log line.
pub fn unknown_key_warning(source_display: &str, unknown_keys: &[String]) -> String {
    format!(
        "{source_display} declares unknown configuration {}: {}. Values were ignored and are not \
         reported. See docs/users/configuration-reference.md.",
        plural_keys(unknown_keys.len()),
        unknown_keys.join(", ")
    )
}

/// Typed API error for unknown keys under a schema version that rejects them.
pub fn unknown_key_error(source_display: &str, unknown_keys: &[String]) -> ApiError {
    ApiError::new(
        UNKNOWN_CONFIG_KEY_CODE,
        format!(
            "{source_display} declares unknown configuration {}: {}. Schema version 2 rejects \
             unknown keys; remove them or declare {CONFIG_SCHEMA_VERSION_KEY} = 1.",
            plural_keys(unknown_keys.len()),
            unknown_keys.join(", ")
        ),
    )
}

fn plural_keys(count: usize) -> &'static str {
    if count == 1 { "key" } else { "keys" }
}

/// Render the user-facing configuration reference from the registry.
///
/// `crates/codestory-contracts/src/bin/generate_config_docs.rs` writes the
/// result to `docs/users/configuration-reference.md`, and a drift test fails
/// when the checked-in page and this rendering disagree.
pub fn render_configuration_reference() -> String {
    let mut page = String::new();
    page.push_str("# Configuration reference\n\n");
    page.push_str(
        "<!-- Generated from codestory_contracts::config_registry. Do not edit by hand; run\n\
         `cargo run -p codestory-contracts --bin generate_config_docs`. -->\n\n",
    );
    page.push_str(
        "CodeStory reads configuration from two places: `.codestory.toml` files and the process \
         environment. Environment values win over files, and the user home file loads before the \
         project file.\n\n",
    );

    page.push_str("## Configuration files\n\n");
    page.push_str(
        "`.codestory.toml` is read from the user home directory and from the project root. Only \
         the keys below are honoured.\n\n",
    );
    page.push_str("| Key | Type | Where it may be set | Meaning |\n");
    page.push_str("| --- | --- | --- | --- |\n");
    for entry in CONFIG_FILE_KEYS {
        let _ = writeln!(
            page,
            "| `{}` | {} | {} | {} |",
            entry.key,
            entry.kind.as_str(),
            entry.trust.as_doc(),
            entry.summary
        );
    }

    let _ = write!(
        page,
        "\n## Schema versions\n\n\
         `{CONFIG_SCHEMA_VERSION_KEY}` declares which configuration schema a file is written \
         against. A file without the key is read as version {CONFIG_SCHEMA_CURRENT_VERSION}.\n\n\
         | Declared version | Unknown keys | Result |\n\
         | --- | --- | --- |\n\
         | 1 (or absent) | warned about by name | the file loads and unknown keys are ignored |\n\
         | 2 | rejected | the command fails with `{UNKNOWN_CONFIG_KEY_CODE}` |\n\
         | above {CONFIG_SCHEMA_MAX_SUPPORTED_VERSION} | not interpreted | the command fails with \
         `{UNSUPPORTED_CONFIG_SCHEMA_CODE}` |\n\n\
         Warnings and errors name unknown keys. They never repeat a configured value, so a \
         mistyped credential key cannot reach a log line.\n\n"
    );

    for audience in AUDIENCES {
        let settings = ENV_SETTINGS
            .iter()
            .filter(|setting| setting.audience == audience)
            .collect::<Vec<_>>();
        if settings.is_empty() {
            continue;
        }
        let _ = write!(
            page,
            "## {}\n\n{}\n\n| Variable | Type | Owner | Meaning |\n| --- | --- | --- | --- |\n",
            audience.heading(),
            audience.note()
        );
        for entry in settings {
            let summary = match entry.secrecy {
                SettingSecrecy::Secret => {
                    format!("{} Values are never logged or displayed.", entry.summary)
                }
                SettingSecrecy::Public => entry.summary.to_string(),
            };
            let _ = writeln!(
                page,
                "| `{}` | {} | `{}` | {summary} |",
                entry.name,
                entry.kind.as_str(),
                entry.owner
            );
        }
        page.push('\n');
    }

    page
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_settings_are_unique_and_sorted() {
        let names = ENV_SETTINGS
            .iter()
            .map(|setting| setting.name)
            .collect::<Vec<_>>();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            names, sorted,
            "registry must stay sorted and duplicate-free"
        );
        for setting in ENV_SETTINGS {
            assert!(
                setting.name.starts_with("CODESTORY_"),
                "{} is not a CodeStory identity",
                setting.name
            );
            assert!(
                setting.owner.starts_with("crates/") && setting.owner.ends_with(".rs"),
                "{} must name a production source file as owner",
                setting.name
            );
            assert!(
                setting.summary.ends_with('.'),
                "{} needs a sentence summary",
                setting.name
            );
        }
    }

    #[test]
    fn config_keys_are_unique_and_sorted() {
        let keys = CONFIG_FILE_KEYS
            .iter()
            .map(|entry| entry.key)
            .collect::<Vec<_>>();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(keys, sorted);
        assert!(is_registered_config_key(CONFIG_SCHEMA_VERSION_KEY));
        assert!(!is_registered_config_key("embedding_query_prefix"));
    }

    #[test]
    fn schema_version_selects_unknown_key_behaviour() {
        assert_eq!(unknown_key_policy(0), Ok(UnknownKeyPolicy::Warn));
        assert_eq!(unknown_key_policy(1), Ok(UnknownKeyPolicy::Warn));
        assert_eq!(unknown_key_policy(2), Ok(UnknownKeyPolicy::Reject));
        let failure = unknown_key_policy(3).expect_err("version 3 is unreadable");
        assert_eq!(failure.requested, 3);
        assert_eq!(failure.max_supported, CONFIG_SCHEMA_MAX_SUPPORTED_VERSION);
        let error = failure.to_api_error("/tmp/.codestory.toml");
        assert_eq!(error.code, UNSUPPORTED_CONFIG_SCHEMA_CODE);
        assert!(error.message.contains("/tmp/.codestory.toml"));
    }

    #[test]
    fn unknown_keys_are_reported_by_name_without_values() {
        let unknown = unknown_config_keys(["cache_dir", "summary_api_key", "typo", "typo"]);
        assert_eq!(unknown, vec!["summary_api_key".to_string(), "typo".into()]);

        let warning = unknown_key_warning("/repo/.codestory.toml", &unknown);
        assert!(warning.contains("summary_api_key"));
        assert!(warning.contains("keys"));
        assert!(
            !warning.contains("sk-"),
            "warning must not carry configured values"
        );

        let single = unknown_config_keys(["typo"]);
        assert!(unknown_key_warning("/repo/.codestory.toml", &single).contains(" key: typo"));

        let error = unknown_key_error("/repo/.codestory.toml", &unknown);
        assert_eq!(error.code, UNKNOWN_CONFIG_KEY_CODE);
        assert!(error.message.contains("typo"));
    }

    #[test]
    fn secret_settings_never_render_their_value() {
        let secret_names = ENV_SETTINGS
            .iter()
            .filter(|setting| setting.secrecy == SettingSecrecy::Secret)
            .map(|setting| setting.name)
            .collect::<Vec<_>>();
        assert!(secret_names.contains(&SUMMARY_API_KEY_ENV));

        let reference = render_configuration_reference();
        for name in secret_names {
            let row = reference
                .lines()
                .find(|line| line.contains(name))
                .unwrap_or_else(|| panic!("{name} must appear in the generated reference"));
            assert!(
                row.contains("never logged or displayed"),
                "{name} row must state that values stay hidden"
            );
        }
    }

    #[test]
    fn owner_lookup_answers_for_registered_identities_only() {
        assert_eq!(
            env_setting_owner(SUMMARY_API_KEY_ENV),
            Some("crates/codestory-retrieval/src/config.rs")
        );
        assert_eq!(env_setting_owner("CODESTORY_NOT_A_SETTING"), None);
    }

    #[test]
    fn generated_reference_covers_every_registered_identity() {
        let reference = render_configuration_reference();
        for setting in ENV_SETTINGS {
            assert!(
                reference.contains(&format!("| `{}` |", setting.name)),
                "{} is missing from the generated reference",
                setting.name
            );
        }
        for entry in CONFIG_FILE_KEYS {
            assert!(
                reference.contains(&format!("| `{}` |", entry.key)),
                "{} is missing from the generated reference",
                entry.key
            );
        }
        assert_eq!(
            reference,
            render_configuration_reference(),
            "rendering must be deterministic"
        );
    }
}
