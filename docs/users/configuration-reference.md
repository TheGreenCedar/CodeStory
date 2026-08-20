# Configuration reference

<!-- Generated from codestory_contracts::config_registry. Do not edit by hand; run
`cargo run -p codestory-contracts --bin generate_config_docs`. -->

CodeStory reads configuration from two places: `.codestory.toml` files and the process environment. Environment values win over files, and the user home file loads before the project file.

## Configuration files

`.codestory.toml` is read from the user home directory and from the project root. Only the keys below are honoured.

| Key | Type | Where it may be set | Meaning |
| --- | --- | --- | --- |
| `cache_dir` | path | user home only | Cache root for every project this process opens. |
| `hybrid_retrieval_enabled` | boolean | user home or project | Enables hybrid lexical/semantic ranking. |
| `schema_version` | integer | user home or project | Configuration schema version this file is written against. |
| `semantic_doc_alias_mode` | enum | user home or project | Alias expansion mode for semantic documents. |
| `semantic_doc_scope` | enum | user home or project | Which symbols receive semantic documents. |
| `summary_endpoint` | text | user home; project needs the network opt-in | Symbol summary endpoint URL. |
| `summary_model` | text | user home; project needs the network opt-in | Model name sent to the symbol summary endpoint. |

## Schema versions

`schema_version` declares which configuration schema a file is written against. A file without the key is read as version 1.

| Declared version | Unknown keys | Result |
| --- | --- | --- |
| 1 (or absent) | warned about by name | the file loads and unknown keys are ignored |
| 2 | rejected | the command fails with `unknown_config_key` |
| above 2 | not interpreted | the command fails with `unsupported_config_schema` |

Warnings and errors name unknown keys. They never repeat a configured value, so a mistyped credential key cannot reach a log line.

## Environment variables

The owner column names the one source file that reads a variable. A build check rejects any other production file that reads it, so a setting has one meaning: no second module can clamp, default, or reject a value the owner accepted.

### Supported environment variables

Operator-facing settings. Environment values win over `.codestory.toml`.

| Variable | Type | Owner | Meaning |
| --- | --- | --- | --- |
| `CODESTORY_AGENT_PREFLIGHT_LOCAL_REFRESH_TIMEOUT_MS` | integer | `crates/codestory-cli/src/app/readiness_commands/preflight.rs` | Milliseconds `agent preflight` waits for a local refresh before reporting. |
| `CODESTORY_ALLOW_PROJECT_NETWORK_CONFIG` | boolean | `crates/codestory-cli/src/config.rs` | Process-wide opt-in that lets project `.codestory.toml` files set summary network keys. |
| `CODESTORY_ALLOW_SENSITIVE_PROJECT_ROOT` | boolean | `crates/codestory-cli/src/config.rs` | Process-start opt-in that allows `$HOME`, `~/.ssh`, `~/.gnupg`, or a `secrets/` directory as a project root. Tool arguments cannot set this. |
| `CODESTORY_CACHE_ROOT` | path | `crates/codestory-retrieval/src/config.rs` | Overrides the per-user cache root that holds every project cache. |
| `CODESTORY_EMBED_ALLOW_CPU` | boolean | `crates/codestory-cli/src/app.rs` | Rejected at startup: CPU embedding is unsupported, so any non-empty value fails closed. |
| `CODESTORY_HYBRID_RETRIEVAL_ENABLED` | boolean | `crates/codestory-retrieval/src/config.rs` | Enables hybrid lexical/semantic ranking (default on). |
| `CODESTORY_INDEX_FRESHNESS_TTL_SECS` | integer | `crates/codestory-runtime/src/index_freshness.rs` | Seconds a content-verified freshness verdict is reused before rechecking. |
| `CODESTORY_INDEX_SOURCE_FILE_BYTE_CAP` | integer | `crates/codestory-cli/src/config.rs` | Admission headroom in bytes for parser-backed sources; structural formats keep their own smaller bound. Not a cost bound. Non-positive values keep the default. |
| `CODESTORY_LLM_DOC_EMBED_BATCH_SIZE` | integer | `crates/codestory-retrieval/src/config.rs` | Documents per embedding batch during semantic publication. |
| `CODESTORY_LOG` | enum | `crates/codestory-cli/src/diagnostics.rs` | Process log level: `error`, `warn`, `info`, `debug`, or `trace`. |
| `CODESTORY_LOG_CORRELATION_ID` | text | `crates/codestory-cli/src/diagnostics.rs` | Correlation id stamped on every diagnostic record of this process. |
| `CODESTORY_NO_TUI` | boolean | `crates/codestory-cli/src/explore.rs` | Forces `explore` to render plain output instead of the terminal UI. |
| `CODESTORY_RETRIEVAL_PROFILE` | enum | `crates/codestory-retrieval/src/config.rs` | Cache namespace profile: `local`/`dev` or `agent`/`ci`. |
| `CODESTORY_RETRIEVAL_RUN_ID` | text | `crates/codestory-retrieval/src/config.rs` | Run label that separates agent-profile cache namespaces. |
| `CODESTORY_SEMANTIC_DOC_ALIAS_MODE` | enum | `crates/codestory-retrieval/src/config.rs` | Alias expansion mode for semantic documents. |
| `CODESTORY_SEMANTIC_DOC_MAX_TOKENS` | integer | `crates/codestory-retrieval/src/config.rs` | Token budget per semantic document. |
| `CODESTORY_SEMANTIC_DOC_SCOPE` | enum | `crates/codestory-retrieval/src/config.rs` | Which symbols receive semantic documents. |
| `CODESTORY_SEMANTIC_STREAM_PENDING_DOCS` | boolean | `crates/codestory-retrieval/src/config.rs` | Streams pending semantic documents instead of materializing them first. |
| `CODESTORY_SEMANTIC_STREAM_SORT_WINDOW_BATCHES` | integer | `crates/codestory-retrieval/src/config.rs` | Batches held in the semantic streaming sort window. |
| `CODESTORY_STDIO_CACHE_ROOT` | path | `crates/codestory-cli/src/config.rs` | Cache root captured once for a multi-project stdio process. |
| `CODESTORY_STORED_VECTOR_ENCODING` | enum | `crates/codestory-store/src/storage_impl/helpers.rs` | Encoding used for vectors persisted in the core database. |
| `CODESTORY_SUMMARY_API_KEY` | text | `crates/codestory-retrieval/src/config.rs` | Credential for the symbol summary endpoint. Values are never logged or displayed. |
| `CODESTORY_SUMMARY_ENDPOINT` | text | `crates/codestory-retrieval/src/config.rs` | Symbol summary endpoint URL; unset disables summary generation. |
| `CODESTORY_SUMMARY_MAX_TOKENS` | integer | `crates/codestory-retrieval/src/config.rs` | Token ceiling for one symbol summary request. |
| `CODESTORY_SUMMARY_MODEL` | text | `crates/codestory-retrieval/src/config.rs` | Model name sent to the symbol summary endpoint. |
| `CODESTORY_SUMMARY_TIMEOUT_SECS` | integer | `crates/codestory-retrieval/src/config.rs` | Seconds a symbol summary request may take, clamped to 1-300. |

### Host-provided environment variables

The plugin launcher and installed hosts set these. Setting them by hand misreports provisioning provenance.

| Variable | Type | Owner | Meaning |
| --- | --- | --- | --- |
| `CODESTORY_CLI` | path | `crates/codestory-cli/src/runtime.rs` | Executable an installed host launches instead of the provisioned CLI. |
| `CODESTORY_LATEST_RELEASE_VERSION` | text | `crates/codestory-cli/src/stdio_transport.rs` | Latest published version supplied by the host instead of a live release probe. |
| `CODESTORY_PLUGIN_ACTIVE_PROJECT_TTL_MS` | integer | `crates/codestory-cli/src/stdio_transport.rs` | Milliseconds a host-recorded active project stays routable. |
| `CODESTORY_PLUGIN_ACTIVE_STATE_PATH` | path | `crates/codestory-cli/src/stdio_transport.rs` | File where the host records the active project for stdio routing. |
| `CODESTORY_PLUGIN_CACHE_VERSION` | text | `crates/codestory-cli/src/stdio_transport.rs` | Plugin cache layout version reported with provisioning status. |
| `CODESTORY_PLUGIN_CLI_ARCHIVE_SHA256` | text | `crates/codestory-cli/src/stdio_transport.rs` | Digest of the archive the host expanded to provision the CLI. |
| `CODESTORY_PLUGIN_CLI_ARCHIVE_URL` | text | `crates/codestory-cli/src/stdio_transport.rs` | Archive URL the host downloaded to provision the CLI. |
| `CODESTORY_PLUGIN_CLI_BUILD_SOURCE` | text | `crates/codestory-cli/src/stdio_transport.rs` | Build source the host used when it provisioned the CLI from source. |
| `CODESTORY_PLUGIN_CLI_MANIFEST_PATH` | path | `crates/codestory-cli/src/stdio_transport.rs` | Provisioning manifest the host wrote beside the installed CLI. |
| `CODESTORY_PLUGIN_CLI_PATH` | path | `crates/codestory-cli/src/stdio_transport.rs` | Installed CLI executable the host provisioned. |
| `CODESTORY_PLUGIN_CLI_PROVISIONED_AT` | text | `crates/codestory-cli/src/stdio_transport.rs` | Timestamp recorded when the host provisioned the CLI. |
| `CODESTORY_PLUGIN_CLI_REPO_REF` | text | `crates/codestory-cli/src/stdio_transport.rs` | Repository ref the host built the CLI from. |
| `CODESTORY_PLUGIN_CLI_RETENTION` | text | `crates/codestory-cli/src/stdio_transport.rs` | Retention policy the host applied to previously provisioned CLIs. |
| `CODESTORY_PLUGIN_CLI_SHA256` | text | `crates/codestory-cli/src/stdio_transport.rs` | Digest of the installed CLI executable. |
| `CODESTORY_PLUGIN_CLI_SOURCE` | text | `crates/codestory-cli/src/stdio_transport.rs` | How the host obtained the CLI: release archive, local build, or explicit path. |
| `CODESTORY_PLUGIN_CLI_VERSION` | text | `crates/codestory-cli/src/stdio_transport.rs` | Version of the CLI the host provisioned. |
| `CODESTORY_PLUGIN_CLI_WARNINGS` | text | `crates/codestory-cli/src/stdio_transport.rs` | Warnings the host recorded while provisioning the CLI. |
| `CODESTORY_PLUGIN_DATA` | path | `crates/codestory-cli/src/stdio_transport.rs` | Host-owned data directory that anchors plugin state files. |
| `CODESTORY_PLUGIN_DIRTY_MARKER_PATH` | path | `crates/codestory-cli/src/stdio_transport.rs` | File the host touches when a repository changed outside CodeStory. |
| `CODESTORY_PLUGIN_DIRTY_MARKER_PROJECT_ROOT` | path | `crates/codestory-cli/src/stdio_transport.rs` | Project root the dirty marker belongs to. |
| `CODESTORY_PLUGIN_LAUNCH_CWD` | path | `crates/codestory-cli/src/stdio_transport.rs` | Working directory the host launched the plugin from. |
| `CODESTORY_PLUGIN_MULTI_PROJECT` | boolean | `crates/codestory-cli/src/stdio_transport.rs` | Declares that the host drives one stdio process across several repositories. |
| `CODESTORY_PLUGIN_ROOT` | path | `crates/codestory-cli/src/stdio_transport.rs` | Installed plugin root the host launched. |
| `CODESTORY_PLUGIN_RUNTIME_CWD` | path | `crates/codestory-cli/src/stdio_transport.rs` | Working directory the runtime binary should adopt. |
| `CODESTORY_PLUGIN_VERSION` | text | `crates/codestory-cli/src/stdio_transport.rs` | Version of the installed plugin package. |

### Diagnostic environment variables

Rollout and diagnostic switches. They are not product configuration and may change or disappear without a compatibility window.

| Variable | Type | Owner | Meaning |
| --- | --- | --- | --- |
| `CODESTORY_DISABLE_INSTALLED_CLI_PROBE` | boolean | `crates/codestory-cli/src/stdio_transport.rs` | Suppresses the installed-CLI probe that status reports as provisioning evidence. |
| `CODESTORY_DISABLE_RELEASE_PROBE` | boolean | `crates/codestory-cli/src/stdio_transport.rs` | Suppresses the latest-release probe and records the probe source as disabled. |
| `CODESTORY_EMBED_QUALIFICATION_DIR` | path | `crates/codestory-retrieval/src/per_user_embedding/qualification_control.rs` | Directory the embedding qualification harness uses for its control handshake. |
| `CODESTORY_EMBED_QUALIFICATION_NONCE` | text | `crates/codestory-retrieval/src/per_user_embedding/qualification_control.rs` | Single-run authentication nonce pairing a qualification worker with its control directory. Values are never logged or displayed. |
| `CODESTORY_GRAPH_INCLUDE_CALLSITE_IDENTITY` | boolean | `crates/codestory-runtime/src/graph_dto.rs` | Includes call-site identity in graph DTOs (default on). |
| `CODESTORY_GRAPH_INCLUDE_CANDIDATE_TARGETS` | boolean | `crates/codestory-runtime/src/graph_dto.rs` | Includes unresolved candidate targets in graph DTOs (default on). |
| `CODESTORY_GRAPH_INCLUDE_EDGE_CERTAINTY` | boolean | `crates/codestory-runtime/src/graph_dto.rs` | Includes edge certainty in graph DTOs (default on). |
| `CODESTORY_INDEX_GRAPH_LAZY` | boolean | `crates/codestory-indexer/src/lib.rs` | Defers graph rule execution until a projection needs it (default on). |
| `CODESTORY_INDEX_LEGACY_DEDUP` | boolean | `crates/codestory-indexer/src/lib.rs` | Restores the superseded edge de-duplication pass. |
| `CODESTORY_INDEX_LEGACY_EDGE_IDENTITY` | boolean | `crates/codestory-indexer/src/lib.rs` | Restores the superseded edge identity derivation. |
| `CODESTORY_INTERNAL_EMBEDDING_SERVER_EXECUTABLE_SHA256` | text | `crates/codestory-cli/src/embedding_server_transport.rs` | Expected embedding-server executable digest the transport verifies before connecting. |
| `CODESTORY_PACKET_CANDIDATE_TRACE` | text | `crates/codestory-runtime/src/agent/orchestrator.rs` | Symbol filter that turns on packet candidate tracing. |
| `CODESTORY_PACKET_STEP_TRACE_OUT` | path | `crates/codestory-runtime/src/agent/trace_export.rs` | File that receives the packet step trace. |
| `CODESTORY_PIPELINE_FLUSH` | boolean | `crates/codestory-indexer/src/lib.rs` | Forces a storage flush after every indexing batch. |
| `CODESTORY_PRECISE_SEMANTIC_SCIP_ARTIFACT` | path | `crates/codestory-retrieval/src/index.rs` | Externally produced SCIP artifact used instead of the built one. |
| `CODESTORY_RESOLUTION_ENABLE_SEMANTIC` | boolean | `crates/codestory-indexer/src/resolution/mod.rs` | Enables semantic resolution passes. |
| `CODESTORY_RESOLUTION_LEGACY` | boolean | `crates/codestory-indexer/src/resolution/mod.rs` | Restores the superseded resolution pipeline. |
| `CODESTORY_RESOLUTION_LEGACY_MODE` | boolean | `crates/codestory-indexer/src/resolution/mod.rs` | Selects the superseded resolution mode inside the legacy pipeline. |
| `CODESTORY_RESOLUTION_PARALLEL_COMPUTE` | boolean | `crates/codestory-indexer/src/resolution/mod.rs` | Computes resolution candidates in parallel. |
| `CODESTORY_RESOLUTION_STORE_CANDIDATES` | boolean | `crates/codestory-indexer/src/resolution/mod.rs` | Persists unresolved resolution candidates for inspection. |
| `CODESTORY_RETRIEVAL` | boolean | `crates/codestory-runtime/src/agent/retrieval_primary.rs` | `1` requires published retrieval for packet and search; `0` is unsupported and fails closed. |
| `CODESTORY_RETRIEVAL_SHADOW` | boolean | `crates/codestory-runtime/src/agent/retrieval_primary.rs` | Runs published retrieval beside the incumbent path for comparison. |
| `CODESTORY_SEMANTIC_CALIBRATION_QUERY_VECTOR_DIR` | path | `crates/codestory-retrieval/src/semantic_calibration_support.rs` | Directory where the development calibration harness captures query vectors. |
| `CODESTORY_SYMBOL_FULL_TEXT_INDEX` | boolean | `crates/codestory-runtime/src/search/engine.rs` | Builds the symbol full-text index (default on). |

### Test-harness environment variables

Only the test suites set these. They drive failure shapes that must never occur in a product run.

| Variable | Type | Owner | Meaning |
| --- | --- | --- | --- |
| `CODESTORY_TEST_EMBED_ALLOW_CPU` | boolean | `crates/codestory-retrieval/src/config.rs` | Test-support builds only: exercises CPU-shaped embedding failures. |
| `CODESTORY_TEST_PROMOTION_ABORT_SENTINEL` | path | `crates/codestory-store/src/storage_impl/mod.rs` | Sentinel path that aborts a promotion mid-flight to prove crash recovery. |

