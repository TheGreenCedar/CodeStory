//! Language support and claim-tier contracts.
//!
//! These tables separate three claims that are easy to overstate:
//! extension/file routing, parser-backed graph extraction, and answer-quality
//! proof. A language profile is product evidence for what the runtime may index
//! today; it is not proof that every framework pattern, semantic edge, or packet
//! answer is supported.

use crate::api::{PacketEvidenceResolutionDto, PacketEvidenceTierDto};
use crate::graph::NodeKind;

/// How CodeStory obtains product evidence for a language profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageSupportMode {
    /// Tree-sitter-backed extraction can produce graph nodes and edges.
    ParserBackedGraph,
    /// Path- or syntax-specific collectors produce exact source anchors only.
    StructuralCollector,
}

impl LanguageSupportMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ParserBackedGraph => "parser_backed_graph",
            Self::StructuralCollector => "structural_collector",
        }
    }
}

/// Evidence tier advertised by a language support profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageEvidenceTier {
    /// Parser-backed graph evidence covered by fidelity checks.
    GraphFidelity,
    /// Exact source anchors without parser-backed graph parity.
    StructuralOnly,
}

impl LanguageEvidenceTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GraphFidelity => "graph_fidelity",
            Self::StructuralOnly => "structural_only",
        }
    }
}

/// Specific claim a proof artifact is allowed to support.
///
/// Higher tiers are intentionally not implied by lower tiers. For example,
/// filename routing does not prove parser support, and parser extraction does
/// not prove packet answer quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageClaimTier {
    FilenameRoute,
    GrammarParse,
    SourceGraphExtraction,
    StructuralSourceProof,
    TypedSemanticEdges,
    PacketSufficientAnswerQuality,
}

impl LanguageClaimTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FilenameRoute => "filename_route",
            Self::GrammarParse => "grammar_parse",
            Self::SourceGraphExtraction => "source_graph_extraction",
            Self::StructuralSourceProof => "structural_source_proof",
            Self::TypedSemanticEdges => "typed_semantic_edges",
            Self::PacketSufficientAnswerQuality => "packet_sufficient_answer_quality",
        }
    }
}

/// Kind of artifact accepted as proof for a language claim tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageProofRole {
    ExtensionRouting,
    ParserSmoke,
    GraphFixture,
    StructuralCollectorFixture,
    SemanticResolverFixture,
    PacketRuntimeArtifact,
}

impl LanguageProofRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExtensionRouting => "extension_routing",
            Self::ParserSmoke => "parser_smoke",
            Self::GraphFixture => "graph_fixture",
            Self::StructuralCollectorFixture => "structural_collector_fixture",
            Self::SemanticResolverFixture => "semantic_resolver_fixture",
            Self::PacketRuntimeArtifact => "packet_runtime_artifact",
        }
    }
}

/// Allowed proof roles and provenance expectations for one claim tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanguageClaimTierContract {
    pub tier: LanguageClaimTier,
    pub allowed_proof_roles: &'static [LanguageProofRole],
    pub provenance_expectations: &'static [&'static str],
}

pub const LANGUAGE_CLAIM_TIER_CONTRACTS: &[LanguageClaimTierContract] = &[
    LanguageClaimTierContract {
        tier: LanguageClaimTier::FilenameRoute,
        allowed_proof_roles: &[LanguageProofRole::ExtensionRouting],
        provenance_expectations: &["LANGUAGE_SUPPORT_PROFILES extension registry"],
    },
    LanguageClaimTierContract {
        tier: LanguageClaimTier::GrammarParse,
        allowed_proof_roles: &[LanguageProofRole::ParserSmoke],
        provenance_expectations: &["live tree-sitter parser config and parse smoke"],
    },
    LanguageClaimTierContract {
        tier: LanguageClaimTier::SourceGraphExtraction,
        allowed_proof_roles: &[LanguageProofRole::GraphFixture],
        provenance_expectations: &["fidelity or tictactoe graph fixture"],
    },
    LanguageClaimTierContract {
        tier: LanguageClaimTier::StructuralSourceProof,
        allowed_proof_roles: &[LanguageProofRole::StructuralCollectorFixture],
        provenance_expectations: &["structural collector fixture with exact source spans"],
    },
    LanguageClaimTierContract {
        tier: LanguageClaimTier::TypedSemanticEdges,
        allowed_proof_roles: &[LanguageProofRole::SemanticResolverFixture],
        provenance_expectations: &["targeted resolver regression"],
    },
    LanguageClaimTierContract {
        tier: LanguageClaimTier::PacketSufficientAnswerQuality,
        allowed_proof_roles: &[LanguageProofRole::PacketRuntimeArtifact],
        provenance_expectations: &["publishable packet-runtime artifact"],
    },
];

/// Structural collector contract for structural-text, non-semantic proof.
///
/// Each row is a product evidence boundary. `semantic_proof_allowed = false`
/// means the collector may support navigation or diagnostics, but must not be
/// promoted as typed semantic evidence without a separate proof tier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StructuralSourceProofContract {
    pub collector_name: &'static str,
    pub path_pattern: &'static str,
    pub emitted_node_kinds: &'static [NodeKind],
    pub source_span: &'static str,
    pub evidence_tier: PacketEvidenceTierDto,
    pub resolution: PacketEvidenceResolutionDto,
    pub confidence: f32,
    pub unsupported_shape_notes: &'static [&'static str],
    pub claim_boundary: &'static str,
    pub semantic_proof_allowed: bool,
}

const GITHUB_ACTIONS_WORKFLOW_NODE_KINDS: &[NodeKind] =
    &[NodeKind::MODULE, NodeKind::FUNCTION, NodeKind::ANNOTATION];
const GITHUB_ACTIONS_WORKFLOW_UNSUPPORTED_SHAPES: &[&str] = &[
    "YAML anchors, aliases, and merge keys are not interpreted.",
    "Matrix expansion, expressions, reusable-workflow calls, and shell bodies are not semantically resolved.",
    "The collector records exact source anchors; it does not validate GitHub Actions execution semantics.",
];
const DOCKER_COMPOSE_NODE_KINDS: &[NodeKind] =
    &[NodeKind::MODULE, NodeKind::FUNCTION, NodeKind::ANNOTATION];
const DOCKER_COMPOSE_UNSUPPORTED_SHAPES: &[&str] = &[
    "Variable interpolation and env-file expansion are not resolved.",
    "Profiles, extends, health checks, dependency order, and runtime container behavior are not interpreted.",
    "The collector records exact source anchors; it does not validate Docker Compose execution semantics.",
];
const OPENAPI_ENDPOINT_NODE_KINDS: &[NodeKind] = &[NodeKind::FUNCTION];
const OPENAPI_ENDPOINT_UNSUPPORTED_SHAPES: &[&str] = &[
    "Handler implementation, auth behavior, request validation, response semantics, and runtime route behavior are not proven.",
    "Generated-client correctness is not proven.",
    "The dedicated OpenAPI indexer records exact schema endpoint anchors; it is not generic YAML structural routing.",
];
const CARGO_MANIFEST_NODE_KINDS: &[NodeKind] =
    &[NodeKind::MODULE, NodeKind::PACKAGE, NodeKind::ANNOTATION];
const CARGO_MANIFEST_UNSUPPORTED_SHAPES: &[&str] = &[
    "Dependency resolution, feature activation, workspace inheritance, build-script behavior, and lockfile proof are not interpreted.",
    "Target-scoped dependency tables, workspace dependency tables, dependency subtables, features, patch, and replace tables are not semantic proof.",
    "The collector records exact source anchors for selected manifest keys only; it does not validate Cargo behavior.",
];
const GENERIC_STRUCTURAL_NODE_KINDS: &[NodeKind] =
    &[NodeKind::MODULE, NodeKind::FUNCTION, NodeKind::ANNOTATION];
const GENERIC_STRUCTURAL_UNSUPPORTED_SHAPES: &[&str] = &[
    "Anchors are conservative source labels, not a complete syntax tree.",
    "Imports, references, substitutions, execution behavior, and typed targets are not resolved.",
    "The collector records exact source anchors only; it does not admit packet semantic proof.",
];

pub const STRUCTURAL_SOURCE_PROOF_CONTRACTS: &[StructuralSourceProofContract] = &[
    StructuralSourceProofContract {
        collector_name: "github_actions_workflow",
        path_pattern: ".github/workflows/*.{yml,yaml}",
        emitted_node_kinds: GITHUB_ACTIONS_WORKFLOW_NODE_KINDS,
        source_span: "1-based source line and column span for the matched workflow, job, or step anchor",
        evidence_tier: PacketEvidenceTierDto::StructuralText,
        resolution: PacketEvidenceResolutionDto::SourceRangeOnly,
        confidence: 1.0,
        unsupported_shape_notes: GITHUB_ACTIONS_WORKFLOW_UNSUPPORTED_SHAPES,
        claim_boundary: "structural exact-source proof only; not parser-backed graph parity, typed semantic resolution, or packet semantic-proof admission",
        semantic_proof_allowed: false,
    },
    StructuralSourceProofContract {
        collector_name: "docker_compose",
        path_pattern: "compose*.{yml,yaml}, docker-compose*.{yml,yaml}, docker/*-compose.{yml,yaml}",
        emitted_node_kinds: DOCKER_COMPOSE_NODE_KINDS,
        source_span: "1-based source line and column span for the matched stack, service, or service property anchor",
        evidence_tier: PacketEvidenceTierDto::StructuralText,
        resolution: PacketEvidenceResolutionDto::SourceRangeOnly,
        confidence: 1.0,
        unsupported_shape_notes: DOCKER_COMPOSE_UNSUPPORTED_SHAPES,
        claim_boundary: "structural exact-source proof only; not parser-backed graph parity, typed semantic resolution, container runtime behavior, or packet semantic-proof admission",
        semantic_proof_allowed: false,
    },
    StructuralSourceProofContract {
        collector_name: "openapi_endpoint_schema",
        path_pattern: "OpenAPI/Swagger JSON or YAML schema files with paths",
        emitted_node_kinds: OPENAPI_ENDPOINT_NODE_KINDS,
        source_span: "1-based source line and column span for the matched schema endpoint method anchor",
        evidence_tier: PacketEvidenceTierDto::ExactSource,
        resolution: PacketEvidenceResolutionDto::SourceRangeOnly,
        confidence: 1.0,
        unsupported_shape_notes: OPENAPI_ENDPOINT_UNSUPPORTED_SHAPES,
        claim_boundary: "dedicated OpenAPI exact-source schema anchor only; not handler implementation, auth behavior, request validation, response semantics, runtime route proof, generated-client correctness, generic YAML support, or packet semantic-proof admission",
        semantic_proof_allowed: false,
    },
    StructuralSourceProofContract {
        collector_name: "cargo_manifest",
        path_pattern: "**/Cargo.toml",
        emitted_node_kinds: CARGO_MANIFEST_NODE_KINDS,
        source_span: "1-based source line and column span for matched workspace member, package name, or direct dependency key anchors",
        evidence_tier: PacketEvidenceTierDto::StructuralText,
        resolution: PacketEvidenceResolutionDto::SourceRangeOnly,
        confidence: 1.0,
        unsupported_shape_notes: CARGO_MANIFEST_UNSUPPORTED_SHAPES,
        claim_boundary: "structural exact-source proof only; not parser-backed graph parity, typed semantic resolution, not semantic dependency proof, Cargo resolution, or packet semantic-proof admission",
        semantic_proof_allowed: false,
    },
    StructuralSourceProofContract {
        collector_name: "markdown",
        path_pattern: "**/*.{md,markdown,mdx}",
        emitted_node_kinds: GENERIC_STRUCTURAL_NODE_KINDS,
        source_span: "1-based exact source span for heading text, link/reference labels, or fenced-block labels",
        evidence_tier: PacketEvidenceTierDto::StructuralText,
        resolution: PacketEvidenceResolutionDto::SourceRangeOnly,
        confidence: 1.0,
        unsupported_shape_notes: GENERIC_STRUCTURAL_UNSUPPORTED_SHAPES,
        claim_boundary: "structural exact-source proof only; not Markdown or MDX semantic parsing, typed target resolution, or packet semantic-proof admission",
        semantic_proof_allowed: false,
    },
    StructuralSourceProofContract {
        collector_name: "yaml",
        path_pattern: "generic **/*.{yml,yaml} after dedicated workflow, Compose, and OpenAPI routing",
        emitted_node_kinds: GENERIC_STRUCTURAL_NODE_KINDS,
        source_span: "1-based exact source span for a conservative mapping-key anchor",
        evidence_tier: PacketEvidenceTierDto::StructuralText,
        resolution: PacketEvidenceResolutionDto::SourceRangeOnly,
        confidence: 1.0,
        unsupported_shape_notes: GENERIC_STRUCTURAL_UNSUPPORTED_SHAPES,
        claim_boundary: "structural exact-source proof only; not workflow, Compose, OpenAPI, YAML execution, typed target resolution, or packet semantic-proof admission",
        semantic_proof_allowed: false,
    },
    StructuralSourceProofContract {
        collector_name: "toml",
        path_pattern: "generic **/*.toml after dedicated Cargo manifest routing",
        emitted_node_kinds: GENERIC_STRUCTURAL_NODE_KINDS,
        source_span: "1-based exact source span for a table header or key anchor",
        evidence_tier: PacketEvidenceTierDto::StructuralText,
        resolution: PacketEvidenceResolutionDto::SourceRangeOnly,
        confidence: 1.0,
        unsupported_shape_notes: GENERIC_STRUCTURAL_UNSUPPORTED_SHAPES,
        claim_boundary: "structural exact-source proof only; not Cargo semantics, typed target resolution, or packet semantic-proof admission",
        semantic_proof_allowed: false,
    },
    StructuralSourceProofContract {
        collector_name: "json",
        path_pattern: "generic **/*.json after dedicated OpenAPI routing",
        emitted_node_kinds: GENERIC_STRUCTURAL_NODE_KINDS,
        source_span: "1-based exact source span for an object-key anchor",
        evidence_tier: PacketEvidenceTierDto::StructuralText,
        resolution: PacketEvidenceResolutionDto::SourceRangeOnly,
        confidence: 1.0,
        unsupported_shape_notes: GENERIC_STRUCTURAL_UNSUPPORTED_SHAPES,
        claim_boundary: "structural exact-source proof only; not OpenAPI semantics, typed target resolution, or packet semantic-proof admission",
        semantic_proof_allowed: false,
    },
    StructuralSourceProofContract {
        collector_name: "shell",
        path_pattern: "**/*.{zsh,ksh,command}; excludes parser-backed .sh and .bash",
        emitted_node_kinds: GENERIC_STRUCTURAL_NODE_KINDS,
        source_span: "1-based exact source span for a function or import anchor",
        evidence_tier: PacketEvidenceTierDto::StructuralText,
        resolution: PacketEvidenceResolutionDto::SourceRangeOnly,
        confidence: 1.0,
        unsupported_shape_notes: GENERIC_STRUCTURAL_UNSUPPORTED_SHAPES,
        claim_boundary: "structural exact-source proof only; not parser-backed Bash graph coverage, shell execution, typed target resolution, or packet semantic-proof admission",
        semantic_proof_allowed: false,
    },
    StructuralSourceProofContract {
        collector_name: "powershell",
        path_pattern: "**/*.{ps1,psm1}",
        emitted_node_kinds: GENERIC_STRUCTURAL_NODE_KINDS,
        source_span: "1-based exact source span for a function or import anchor",
        evidence_tier: PacketEvidenceTierDto::StructuralText,
        resolution: PacketEvidenceResolutionDto::SourceRangeOnly,
        confidence: 1.0,
        unsupported_shape_notes: GENERIC_STRUCTURAL_UNSUPPORTED_SHAPES,
        claim_boundary: "structural exact-source proof only; not PowerShell execution, typed target resolution, or packet semantic-proof admission",
        semantic_proof_allowed: false,
    },
];

/// Public language profile exposed to callers and diagnostics.
///
/// `claim_label` is presentation text, while `support_mode` and
/// `evidence_tier` are compatibility surfaces for downstream logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanguageSupportProfile {
    pub language_name: &'static str,
    pub extensions: &'static [&'static str],
    pub support_mode: LanguageSupportMode,
    pub evidence_tier: LanguageEvidenceTier,
    pub claim_label: &'static str,
}

const PARSER_BACKED_GRAPH: &str = "parser-backed graph, fidelity-gated";
const STRUCTURAL_COLLECTOR: &str = "structural collector only";
const PARSER_BACKED_CLAIM_TIERS: &[LanguageClaimTier] = &[
    LanguageClaimTier::FilenameRoute,
    LanguageClaimTier::GrammarParse,
    LanguageClaimTier::SourceGraphExtraction,
];
const STRUCTURAL_CLAIM_TIERS: &[LanguageClaimTier] = &[
    LanguageClaimTier::FilenameRoute,
    LanguageClaimTier::StructuralSourceProof,
];

pub const LANGUAGE_SUPPORT_PROFILES: &[LanguageSupportProfile] = &[
    parser_profile("python", &["py", "pyi"]),
    parser_profile("java", &["java"]),
    parser_profile("rust", &["rs"]),
    parser_profile("javascript", &["js", "jsx", "mjs", "cjs"]),
    parser_profile("typescript", &["ts", "tsx", "mts", "cts"]),
    parser_profile("cpp", &["cpp", "cc", "cxx", "hpp", "hh", "hxx"]),
    parser_profile("c", &["c", "h"]),
    parser_profile("go", &["go"]),
    parser_profile("ruby", &["rb"]),
    parser_profile("php", &["php"]),
    parser_profile("csharp", &["cs"]),
    parser_profile("kotlin", &["kt", "kts"]),
    parser_profile("swift", &["swift"]),
    parser_profile("dart", &["dart"]),
    parser_profile("bash", &["sh", "bash"]),
    structural_profile("html", &["html", "htm"]),
    structural_profile("css", &["css"]),
    structural_profile("sql", &["sql"]),
    structural_profile("markdown", &["md", "markdown", "mdx"]),
    structural_profile("yaml", &["yml", "yaml"]),
    structural_profile("toml", &["toml"]),
    structural_profile("json", &["json"]),
    structural_profile("shell", &["zsh", "ksh", "command"]),
    structural_profile("powershell", &["ps1", "psm1"]),
    structural_profile("docker_compose", &[]),
    structural_profile("cargo_manifest", &[]),
];

/// Extension that rides along with a registered language without being a claim
/// of its own.
///
/// Template dialects (`.vue`, `.svelte`, `.astro`, `.cshtml`) and style
/// preprocessor extensions (`.scss`, `.sass`, `.less`) used to be hand-repeated
/// at every call site that cared: the workspace's source-group compatibility
/// map, the indexer's template blanking pipeline, the indexer's text-only
/// language projection, and the CLI's snippet highlighter (ARCH-012). They live
/// here instead so those sites agree.
///
/// This is deliberately a sibling table, not more rows in
/// [`LANGUAGE_SUPPORT_PROFILES`]. Adding `.vue` there would silently widen
/// [`supported_extensions`], [`language_name_for_path`] and
/// [`is_structural_source_path`], which decide what the product indexes and
/// what claim tier it advertises. A companion extension makes no claim: it only
/// records which registered languages already accept it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompanionExtensionProfile {
    /// Extension without a leading dot, lowercase.
    pub extension: &'static str,
    /// Surface language reported for the file, when the extension names a
    /// dialect of its own. `None` means the file is only ever attributed to the
    /// language that owns it (Razor is C#, SCSS is CSS).
    pub surface_language: Option<&'static str>,
    /// Language source groups whose discovery accepts this extension.
    ///
    /// Entries are language names, either from [`LANGUAGE_SUPPORT_PROFILES`] or
    /// from `surface_language` itself. They are not claim tiers.
    pub source_group_languages: &'static [&'static str],
}

pub const COMPANION_EXTENSION_PROFILES: &[CompanionExtensionProfile] = &[
    CompanionExtensionProfile {
        extension: "vue",
        surface_language: Some("vue"),
        source_group_languages: &["javascript", "typescript", "vue"],
    },
    CompanionExtensionProfile {
        extension: "svelte",
        surface_language: Some("svelte"),
        source_group_languages: &["javascript", "typescript", "svelte"],
    },
    CompanionExtensionProfile {
        extension: "astro",
        surface_language: Some("astro"),
        source_group_languages: &["javascript", "typescript", "astro"],
    },
    CompanionExtensionProfile {
        extension: "cshtml",
        surface_language: None,
        source_group_languages: &["csharp"],
    },
    CompanionExtensionProfile {
        extension: "lua",
        surface_language: None,
        source_group_languages: &["lua"],
    },
    CompanionExtensionProfile {
        extension: "scss",
        surface_language: None,
        source_group_languages: &["css"],
    },
    CompanionExtensionProfile {
        extension: "sass",
        surface_language: None,
        source_group_languages: &["css"],
    },
    CompanionExtensionProfile {
        extension: "less",
        surface_language: None,
        source_group_languages: &["css"],
    },
];

/// Look up a companion extension profile.
pub fn companion_extension_profile(ext: &str) -> Option<&'static CompanionExtensionProfile> {
    let ext = ext.trim().trim_start_matches('.');
    COMPANION_EXTENSION_PROFILES
        .iter()
        .find(|profile| profile.extension.eq_ignore_ascii_case(ext))
}

/// Surface language for a companion extension, when it names its own dialect.
pub fn companion_surface_language(ext: &str) -> Option<&'static str> {
    companion_extension_profile(ext).and_then(|profile| profile.surface_language)
}

/// Line-comment marker for a language whose extraction rules have been
/// registered.
///
/// Comment syntax is presentation, not evidence, which is why it is a sibling
/// table rather than a field on [`LanguageSupportProfile`] — every field there
/// is a claim-tier fact and must not be diluted by a rendering detail.
///
/// A language appears here only once its per-language extraction package has
/// landed (S3, #1676 onward). Until then the consumer's own match arm owns it,
/// and consumers therefore read this table first and fall back to that arm.
/// Populating the table ahead of the migration would change rendering for
/// languages whose consumer roster deliberately differs from their real syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanguageCommentProfile {
    /// The name consumers actually spell. Usually a registry language name,
    /// but a dialect that only the indexer and the highlighter distinguish
    /// (`tsx`) is keyed by that dialect, because that is the string the
    /// consumer roster held.
    pub language_name: &'static str,
    pub line_comment: &'static str,
}

pub const LANGUAGE_COMMENT_PROFILES: &[LanguageCommentProfile] = &[
    LanguageCommentProfile {
        language_name: "kotlin",
        line_comment: "//",
    },
    LanguageCommentProfile {
        language_name: "java",
        line_comment: "//",
    },
    LanguageCommentProfile {
        language_name: "cpp",
        line_comment: "//",
    },
    LanguageCommentProfile {
        language_name: "javascript",
        line_comment: "//",
    },
    LanguageCommentProfile {
        language_name: "typescript",
        line_comment: "//",
    },
    // `tsx` is not a registry language — the registry routes `.tsx` to
    // `typescript` — but it is the indexer's dispatch name for the JSX-aware
    // TypeScript config and the CLI highlighter's own dialect string. The row
    // covers that dialect only; `typescript` itself stays with the consumer
    // roster until #1681.
    LanguageCommentProfile {
        language_name: "tsx",
        line_comment: "//",
    },
    LanguageCommentProfile {
        language_name: "python",
        line_comment: "#",
    },
    LanguageCommentProfile {
        language_name: "rust",
        line_comment: "//",
    },
    LanguageCommentProfile {
        language_name: "go",
        line_comment: "//",
    },
    LanguageCommentProfile {
        language_name: "ruby",
        line_comment: "#",
    },
    LanguageCommentProfile {
        language_name: "php",
        line_comment: "//",
    },
];

/// Line-comment marker for a registered language name.
pub fn line_comment_for_language(language_name: &str) -> Option<&'static str> {
    let language_name = language_name.trim();
    LANGUAGE_COMMENT_PROFILES
        .iter()
        .find(|profile| profile.language_name.eq_ignore_ascii_case(language_name))
        .map(|profile| profile.line_comment)
}

const fn parser_profile(
    language_name: &'static str,
    extensions: &'static [&'static str],
) -> LanguageSupportProfile {
    LanguageSupportProfile {
        language_name,
        extensions,
        support_mode: LanguageSupportMode::ParserBackedGraph,
        evidence_tier: LanguageEvidenceTier::GraphFidelity,
        claim_label: PARSER_BACKED_GRAPH,
    }
}

const fn structural_profile(
    language_name: &'static str,
    extensions: &'static [&'static str],
) -> LanguageSupportProfile {
    LanguageSupportProfile {
        language_name,
        extensions,
        support_mode: LanguageSupportMode::StructuralCollector,
        evidence_tier: LanguageEvidenceTier::StructuralOnly,
        claim_label: STRUCTURAL_COLLECTOR,
    }
}

/// Normalize a file extension for registry lookup.
pub fn normalize_extension(ext: &str) -> String {
    ext.trim().trim_start_matches('.').to_ascii_lowercase()
}

/// Look up a language profile by extension.
///
/// The lookup is case-insensitive and accepts a leading dot. A hit proves only
/// the profile's declared claim tier, not parser or answer quality beyond it.
pub fn language_support_profile_for_ext(ext: &str) -> Option<&'static LanguageSupportProfile> {
    let ext = ext.trim().trim_start_matches('.');
    LANGUAGE_SUPPORT_PROFILES.iter().find(|profile| {
        profile
            .extensions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(ext))
    })
}

/// Look up a profile by its public language name.
pub fn language_support_profile_for_language_name(
    language_name: &str,
) -> Option<&'static LanguageSupportProfile> {
    let language_name = language_name.trim();
    LANGUAGE_SUPPORT_PROFILES
        .iter()
        .find(|profile| profile.language_name.eq_ignore_ascii_case(language_name))
}

/// Look up a profile from a file path's final extension.
pub fn language_support_profile_for_path(
    path: Option<&str>,
) -> Option<&'static LanguageSupportProfile> {
    let ext = path?.rsplit('.').next()?.trim_start_matches('.');
    language_support_profile_for_ext(ext)
}

/// Return the public language name for a supported path.
pub fn language_name_for_path(path: Option<&str>) -> Option<&'static str> {
    language_support_profile_for_path(path).map(|profile| profile.language_name)
}

/// Return a language name only when the path maps to parser-backed graph support.
pub fn parser_backed_language_name_for_path(path: Option<&str>) -> Option<&'static str> {
    language_support_profile_for_path(path)
        .filter(|profile| profile.support_mode == LanguageSupportMode::ParserBackedGraph)
        .map(|profile| profile.language_name)
}

/// Return a language name only when the path maps to structural-only support.
pub fn structural_language_name_for_path(path: Option<&str>) -> Option<&'static str> {
    language_support_profile_for_path(path)
        .filter(|profile| profile.support_mode == LanguageSupportMode::StructuralCollector)
        .map(|profile| profile.language_name)
}

/// Whether a public language name is structural-only.
pub fn is_structural_language_name(language_name: &str) -> bool {
    language_support_profile_for_language_name(language_name)
        .is_some_and(|profile| profile.support_mode == LanguageSupportMode::StructuralCollector)
}

/// Native lexical case rules used for basename-only routing.
///
/// Existing filesystem identity remains owned by the workspace layer. This
/// enum is only for the final basename spelling after a path has already been
/// selected for indexing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativePathCase {
    Sensitive,
    Insensitive,
}

impl NativePathCase {
    pub const fn current() -> Self {
        #[cfg(windows)]
        {
            Self::Insensitive
        }
        #[cfg(not(windows))]
        {
            Self::Sensitive
        }
    }
}

/// Whether a path is in the GitHub Actions workflow collector scope.
pub fn is_github_actions_workflow_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    let mut parts = normalized.rsplit('/');
    let Some(file_name) = parts.next().filter(|part| !part.is_empty()) else {
        return false;
    };
    let Some(parent) = parts.next() else {
        return false;
    };
    let Some(grandparent) = parts.next() else {
        return false;
    };
    (file_name.ends_with(".yml") || file_name.ends_with(".yaml"))
        && parent == "workflows"
        && grandparent == ".github"
}

/// Whether a path is in the Docker Compose collector scope.
pub fn is_docker_compose_file_path(path: &str) -> bool {
    if is_github_actions_workflow_path(path) {
        return false;
    }
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    let mut parts = normalized.rsplit('/');
    let Some(file_name) = parts.next().filter(|part| !part.is_empty()) else {
        return false;
    };
    let Some((stem, ext)) = file_name.rsplit_once('.') else {
        return false;
    };
    if !matches!(ext, "yml" | "yaml") {
        return false;
    }
    stem.starts_with("compose")
        || stem.starts_with("docker-compose")
        || (stem.ends_with("-compose") && parts.any(|part| part == "docker"))
}

/// Whether a path is exactly a Cargo manifest by basename.
pub fn is_cargo_manifest_file_path(path: &str) -> bool {
    is_cargo_manifest_file_path_with_case(path, NativePathCase::current())
}

/// Whether a path is exactly a Cargo manifest under explicit native lexical rules.
pub fn is_cargo_manifest_file_path_with_case(path: &str, path_case: NativePathCase) -> bool {
    path.replace('\\', "/")
        .rsplit('/')
        .next()
        .is_some_and(|file_name| match path_case {
            NativePathCase::Sensitive => file_name == "Cargo.toml",
            NativePathCase::Insensitive => file_name.eq_ignore_ascii_case("Cargo.toml"),
        })
}

/// Whether a path belongs to a structural source format.
///
/// JSONC is deliberately absent: the public structural contract is JSON only.
pub fn is_structural_source_path(path: &str) -> bool {
    structural_language_name_for_path(Some(path)).is_some()
        || is_github_actions_workflow_path(path)
        || is_docker_compose_file_path(path)
        || is_cargo_manifest_file_path(path)
}

/// Return the shared exclusion reason for one workspace-relative structural path.
///
/// Absolute paths are intentionally ignored so repository ancestors named
/// `build`, `target`, `vendor`, or `secrets` cannot affect source admission.
pub fn structural_source_path_exclusion(path: &str) -> Option<&'static str> {
    let normalized = path.replace('\\', "/");
    let normalized = normalized.trim();
    if normalized.is_empty()
        || normalized.starts_with('/')
        || normalized.starts_with("//")
        || normalized.as_bytes().get(1) == Some(&b':')
            && normalized
                .as_bytes()
                .get(2)
                .is_some_and(|byte| *byte == b'/')
    {
        return None;
    }
    let normalized = normalized
        .strip_prefix("./")
        .unwrap_or(normalized)
        .to_ascii_lowercase();
    let components = normalized
        .split('/')
        .filter(|component| !component.is_empty() && *component != ".")
        .collect::<Vec<_>>();
    if components.contains(&"..") {
        return None;
    }
    let file_name = components.last().copied().unwrap_or_default();
    if components.iter().any(|component| {
        matches!(
            *component,
            ".git"
                | "node_modules"
                | "vendor"
                | "vendors"
                | "third_party"
                | "third-party"
                | "generated"
                | "dist"
                | "build"
                | "target"
                | "coverage"
                | ".next"
                | ".cache"
        )
    }) {
        return Some("generated_or_vendor");
    }
    if components.iter().any(|component| {
        matches!(
            *component,
            "secret" | "secrets" | ".secrets" | "credentials"
        )
    }) || file_name.starts_with(".env")
        || matches!(
            file_name,
            "credentials.json"
                | "secrets.json"
                | "secrets.yaml"
                | "secrets.yml"
                | "id_rsa"
                | "id_ed25519"
        )
        || file_name.ends_with(".pem")
        || file_name.ends_with(".key")
    {
        return Some("secret_bearing");
    }
    if matches!(
        file_name,
        "cargo.lock"
            | "package-lock.json"
            | "npm-shrinkwrap.json"
            | "yarn.lock"
            | "pnpm-lock.yaml"
            | "bun.lock"
            | "bun.lockb"
            | "poetry.lock"
            | "pipfile.lock"
            | "composer.lock"
            | "gemfile.lock"
    ) || file_name.ends_with(".lock")
        || file_name.ends_with("-lock.json")
    {
        return Some("lockfile");
    }
    if file_name.contains(".min.")
        || file_name.ends_with(".min")
        || file_name.starts_with("generated-")
        || file_name.starts_with("generated_")
        || file_name.contains(".generated.")
        || file_name.contains("_generated.")
        || file_name.ends_with(".map")
    {
        return Some("generated_or_high_noise");
    }
    None
}

/// All extensions that have a public language profile.
pub fn supported_extensions() -> impl Iterator<Item = &'static str> {
    LANGUAGE_SUPPORT_PROFILES
        .iter()
        .flat_map(|profile| profile.extensions.iter().copied())
}

/// Claim tiers implied by a language support profile.
pub fn language_claim_tiers_for_profile(
    profile: &LanguageSupportProfile,
) -> &'static [LanguageClaimTier] {
    match profile.support_mode {
        LanguageSupportMode::ParserBackedGraph => PARSER_BACKED_CLAIM_TIERS,
        LanguageSupportMode::StructuralCollector => STRUCTURAL_CLAIM_TIERS,
    }
}

/// Contract row for one claim tier.
pub fn language_claim_tier_contract(
    tier: LanguageClaimTier,
) -> Option<&'static LanguageClaimTierContract> {
    LANGUAGE_CLAIM_TIER_CONTRACTS
        .iter()
        .find(|contract| contract.tier == tier)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn profile_lookup_covers_claimed_parser_and_structural_languages() {
        assert_eq!(
            language_support_profile_for_ext("kt")
                .expect("kotlin profile")
                .language_name,
            "kotlin"
        );
        assert_eq!(
            language_support_profile_for_ext(".swift")
                .expect("swift profile")
                .support_mode,
            LanguageSupportMode::ParserBackedGraph
        );
        assert_eq!(
            language_support_profile_for_ext("html")
                .expect("html profile")
                .evidence_tier,
            LanguageEvidenceTier::StructuralOnly
        );
        assert_eq!(
            language_support_profile_for_path(Some("src/lib.RS"))
                .expect("rust path profile")
                .language_name,
            "rust"
        );
        assert_eq!(
            parser_backed_language_name_for_path(Some("src/app.tsx")),
            Some("typescript")
        );
        assert_eq!(
            structural_language_name_for_path(Some("assets/site.CSS")),
            Some("css")
        );
        assert!(is_structural_language_name(" SQL "));
        assert_eq!(
            structural_language_name_for_path(Some("docs/guide.mdx")),
            Some("markdown")
        );
        assert_eq!(
            structural_language_name_for_path(Some("scripts/setup.ps1")),
            Some("powershell")
        );
        assert!(
            language_name_for_path(Some("src/app/Program.cshtml")).is_none(),
            "Razor .cshtml files are workspace-compatible, but not a public parser-backed C# claim"
        );
    }

    #[test]
    fn profile_extensions_are_unique() {
        let mut seen = HashSet::new();
        for extension in supported_extensions() {
            assert!(
                seen.insert(extension),
                "extension should have exactly one owner: {extension}"
            );
        }
    }

    #[test]
    fn claim_tier_contracts_cover_profile_tiers_without_overclaiming() {
        for contract in LANGUAGE_CLAIM_TIER_CONTRACTS {
            assert!(
                !contract.allowed_proof_roles.is_empty(),
                "{} needs a proof role",
                contract.tier.as_str()
            );
            assert!(
                !contract.provenance_expectations.is_empty(),
                "{} needs provenance expectations",
                contract.tier.as_str()
            );
        }

        for profile in LANGUAGE_SUPPORT_PROFILES {
            let tiers = language_claim_tiers_for_profile(profile);
            assert!(
                tiers.contains(&LanguageClaimTier::FilenameRoute),
                "{} must at least claim filename routing",
                profile.language_name
            );
            for tier in tiers {
                assert!(
                    language_claim_tier_contract(*tier).is_some(),
                    "{} has no tier contract",
                    tier.as_str()
                );
            }

            match profile.support_mode {
                LanguageSupportMode::ParserBackedGraph => {
                    assert!(tiers.contains(&LanguageClaimTier::GrammarParse));
                    assert!(tiers.contains(&LanguageClaimTier::SourceGraphExtraction));
                }
                LanguageSupportMode::StructuralCollector => {
                    assert!(!tiers.contains(&LanguageClaimTier::GrammarParse));
                    assert!(tiers.contains(&LanguageClaimTier::StructuralSourceProof));
                    assert!(!tiers.contains(&LanguageClaimTier::SourceGraphExtraction));
                }
            }

            assert!(
                !tiers.contains(&LanguageClaimTier::TypedSemanticEdges),
                "{} runtime profile must not imply typed semantic edges",
                profile.language_name
            );
            assert!(
                !tiers.contains(&LanguageClaimTier::PacketSufficientAnswerQuality),
                "{} runtime profile must not imply packet-quality proof",
                profile.language_name
            );
        }
    }

    #[test]
    fn structural_source_proof_contract_is_structural_text_not_semantic() {
        let contract = STRUCTURAL_SOURCE_PROOF_CONTRACTS
            .iter()
            .find(|contract| contract.collector_name == "github_actions_workflow")
            .expect("github actions structural contract");
        assert_eq!(contract.path_pattern, ".github/workflows/*.{yml,yaml}");
        assert!(contract.emitted_node_kinds.contains(&NodeKind::MODULE));
        assert!(contract.emitted_node_kinds.contains(&NodeKind::FUNCTION));
        assert_eq!(
            contract.evidence_tier,
            PacketEvidenceTierDto::StructuralText
        );
        assert_eq!(
            contract.resolution,
            PacketEvidenceResolutionDto::SourceRangeOnly
        );
        assert_eq!(contract.confidence, 1.0);
        assert!(!contract.unsupported_shape_notes.is_empty());
        assert!(!contract.semantic_proof_allowed);
        assert!(contract.claim_boundary.contains("not parser-backed"));

        let compose_contract = STRUCTURAL_SOURCE_PROOF_CONTRACTS
            .iter()
            .find(|contract| contract.collector_name == "docker_compose")
            .expect("docker compose structural contract");
        assert_eq!(
            compose_contract.path_pattern,
            "compose*.{yml,yaml}, docker-compose*.{yml,yaml}, docker/*-compose.{yml,yaml}"
        );
        assert!(
            compose_contract
                .emitted_node_kinds
                .contains(&NodeKind::MODULE)
        );
        assert!(
            compose_contract
                .emitted_node_kinds
                .contains(&NodeKind::FUNCTION)
        );
        assert!(
            compose_contract
                .emitted_node_kinds
                .contains(&NodeKind::ANNOTATION)
        );
        assert_eq!(
            compose_contract.evidence_tier,
            PacketEvidenceTierDto::StructuralText
        );
        assert_eq!(
            compose_contract.resolution,
            PacketEvidenceResolutionDto::SourceRangeOnly
        );
        assert!(!compose_contract.semantic_proof_allowed);
        assert!(
            compose_contract
                .unsupported_shape_notes
                .iter()
                .any(|note| note.contains("interpolation"))
        );

        let openapi_contract = STRUCTURAL_SOURCE_PROOF_CONTRACTS
            .iter()
            .find(|contract| contract.collector_name == "openapi_endpoint_schema")
            .expect("OpenAPI endpoint schema contract");
        assert_eq!(
            openapi_contract.path_pattern,
            "OpenAPI/Swagger JSON or YAML schema files with paths"
        );
        assert_eq!(openapi_contract.emitted_node_kinds, &[NodeKind::FUNCTION]);
        assert_eq!(
            openapi_contract.evidence_tier,
            PacketEvidenceTierDto::ExactSource
        );
        assert_eq!(
            openapi_contract.resolution,
            PacketEvidenceResolutionDto::SourceRangeOnly
        );
        assert!(!openapi_contract.semantic_proof_allowed);
        assert!(
            openapi_contract
                .claim_boundary
                .contains("not handler implementation")
        );
        assert!(
            openapi_contract
                .claim_boundary
                .contains("generic YAML support")
        );

        let cargo_contract = STRUCTURAL_SOURCE_PROOF_CONTRACTS
            .iter()
            .find(|contract| contract.collector_name == "cargo_manifest")
            .expect("cargo manifest structural contract");
        assert_eq!(cargo_contract.path_pattern, "**/Cargo.toml");
        assert!(
            cargo_contract
                .emitted_node_kinds
                .contains(&NodeKind::MODULE)
        );
        assert!(
            cargo_contract
                .emitted_node_kinds
                .contains(&NodeKind::PACKAGE)
        );
        assert!(
            cargo_contract
                .emitted_node_kinds
                .contains(&NodeKind::ANNOTATION)
        );
        assert_eq!(
            cargo_contract.evidence_tier,
            PacketEvidenceTierDto::StructuralText
        );
        assert_eq!(
            cargo_contract.resolution,
            PacketEvidenceResolutionDto::SourceRangeOnly
        );
        assert!(!cargo_contract.semantic_proof_allowed);
        assert!(
            cargo_contract
                .claim_boundary
                .contains("not semantic dependency proof")
        );

        for collector_name in ["markdown", "yaml", "toml", "json", "shell", "powershell"] {
            let contract = STRUCTURAL_SOURCE_PROOF_CONTRACTS
                .iter()
                .find(|contract| contract.collector_name == collector_name)
                .unwrap_or_else(|| panic!("missing {collector_name} structural contract"));
            assert_eq!(
                contract.evidence_tier,
                PacketEvidenceTierDto::StructuralText
            );
            assert_eq!(
                contract.resolution,
                PacketEvidenceResolutionDto::SourceRangeOnly
            );
            assert!(!contract.semantic_proof_allowed);
            assert!(contract.claim_boundary.contains("packet semantic-proof"));
        }
    }

    #[test]
    fn github_actions_workflow_path_keeps_specialized_scope_with_generic_yaml_support() {
        assert!(is_github_actions_workflow_path(
            "repo/.github/workflows/ci.yml"
        ));
        assert!(is_github_actions_workflow_path(
            r"repo\.github\workflows\release.yaml"
        ));
        assert!(!is_github_actions_workflow_path("openapi.yaml"));
        assert!(!is_github_actions_workflow_path(
            "docs/not.github/workflows/ci.yml"
        ));
        assert!(!is_github_actions_workflow_path(
            "repo/.github/workflows/nested/ci.yml"
        ));
        assert!(!is_github_actions_workflow_path(
            "repo/.github/workflows/readme.md"
        ));
        assert_eq!(
            language_support_profile_for_ext("yaml")
                .expect("generic YAML structural profile")
                .support_mode,
            LanguageSupportMode::StructuralCollector
        );
    }

    #[test]
    fn docker_compose_path_keeps_specialized_scope_with_generic_yaml_support() {
        assert!(is_docker_compose_file_path("compose.yaml"));
        assert!(is_docker_compose_file_path("deploy/compose.yml"));
        assert!(is_docker_compose_file_path("docker-compose.override.yml"));
        assert!(is_docker_compose_file_path(
            r"repo\docker\retrieval-compose.yml"
        ));
        assert!(!is_docker_compose_file_path(".github/workflows/ci.yml"));
        assert!(!is_docker_compose_file_path("openapi.yaml"));
        assert!(!is_docker_compose_file_path("docs/service.yml"));
        assert_eq!(
            language_support_profile_for_ext("yaml")
                .expect("generic YAML structural profile")
                .language_name,
            "yaml"
        );
        let profile = language_support_profile_for_language_name("docker_compose")
            .expect("docker compose structural profile");
        assert_eq!(
            profile.support_mode,
            LanguageSupportMode::StructuralCollector
        );
        assert_eq!(profile.evidence_tier, LanguageEvidenceTier::StructuralOnly);
        assert!(profile.extensions.is_empty());
    }

    #[test]
    fn cargo_manifest_path_keeps_specialized_scope_with_generic_toml_support() {
        assert!(is_cargo_manifest_file_path("Cargo.toml"));
        assert!(is_cargo_manifest_file_path("crates/tool/Cargo.toml"));
        assert!(is_cargo_manifest_file_path(r"crates\tool\Cargo.toml"));
        #[cfg(not(windows))]
        assert!(!is_cargo_manifest_file_path("cargo.toml"));
        #[cfg(windows)]
        assert!(is_cargo_manifest_file_path("cargo.toml"));
        assert!(is_cargo_manifest_file_path_with_case(
            r"crates\tool\CARGO.TOML",
            NativePathCase::Insensitive
        ));
        assert!(!is_cargo_manifest_file_path_with_case(
            r"crates\tool\CARGO.TOML",
            NativePathCase::Sensitive
        ));
        assert!(!is_cargo_manifest_file_path("config.toml"));
        assert!(!is_cargo_manifest_file_path(".cargo/config.toml"));
        assert!(!is_cargo_manifest_file_path("Cargo.lock"));
        assert_eq!(
            language_support_profile_for_ext("toml")
                .expect("generic TOML structural profile")
                .language_name,
            "toml"
        );
        let profile = language_support_profile_for_language_name("cargo_manifest")
            .expect("cargo manifest structural profile");
        assert_eq!(
            profile.support_mode,
            LanguageSupportMode::StructuralCollector
        );
        assert_eq!(profile.evidence_tier, LanguageEvidenceTier::StructuralOnly);
        assert!(profile.extensions.is_empty());
    }

    /// Companion extensions must stay out of the claim registry.
    ///
    /// The whole reason this is a sibling table is that a companion extension
    /// must not widen what the product indexes or what tier it advertises. If a
    /// future edit moved `.vue` or `.scss` into `LANGUAGE_SUPPORT_PROFILES`,
    /// `supported_extensions`, `language_name_for_path` and
    /// `is_structural_source_path` would all start answering for it.
    #[test]
    fn companion_extensions_make_no_public_language_claim() {
        for profile in COMPANION_EXTENSION_PROFILES {
            assert_eq!(
                language_support_profile_for_ext(profile.extension),
                None,
                "companion extension `{}` must not carry a public claim tier",
                profile.extension
            );
            assert!(
                !is_structural_source_path(&format!("src/file.{}", profile.extension)),
                "companion extension `{}` must not be a structural source path",
                profile.extension
            );
            assert!(
                !profile.source_group_languages.is_empty(),
                "companion extension `{}` needs at least one source group",
                profile.extension
            );
        }

        let mut seen = HashSet::new();
        for profile in COMPANION_EXTENSION_PROFILES {
            assert!(
                seen.insert(profile.extension),
                "companion extension `{}` is declared twice",
                profile.extension
            );
            assert_eq!(
                profile.extension.to_ascii_lowercase(),
                profile.extension,
                "companion extensions are stored lowercase"
            );
        }
    }

    /// Only the three blanked component dialects report a surface language.
    ///
    /// `template_surface_language` in the indexer routes on exactly this set,
    /// and `semantic::detect_language` reports whatever it returns. Giving
    /// Razor or SCSS a surface here would start attributing those files to a
    /// language that has no parser behind it.
    #[test]
    fn only_component_dialects_carry_a_surface_language() {
        let surfaces = COMPANION_EXTENSION_PROFILES
            .iter()
            .filter_map(|profile| {
                profile
                    .surface_language
                    .map(|surface| (profile.extension, surface))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            surfaces,
            vec![("vue", "vue"), ("svelte", "svelte"), ("astro", "astro")]
        );
        assert_eq!(companion_surface_language(".VUE"), Some("vue"));
        assert_eq!(companion_surface_language("cshtml"), None);
        assert_eq!(companion_surface_language("scss"), None);
        assert_eq!(companion_surface_language("kt"), None);
    }

    /// Every source group a companion extension names must be a real language.
    ///
    /// A typo here would silently drop the extension out of discovery for that
    /// source group rather than fail.
    #[test]
    fn companion_source_groups_name_real_languages() {
        let surface_names = COMPANION_EXTENSION_PROFILES
            .iter()
            .filter_map(|profile| profile.surface_language)
            .collect::<HashSet<_>>();
        for profile in COMPANION_EXTENSION_PROFILES {
            for language in profile.source_group_languages {
                assert!(
                    language_support_profile_for_language_name(
                        language
                    ).is_some(
                    )
                    || surface_names.contains(
                        language
                    )
                    // `lua` has no registered profile at all; it exists
                    // only as a workspace source-group compatibility name.
                    || *language == "lua",
                    "`{}` names unknown source group `{language}`",
                    profile.extension
                );
            }
        }
    }

    /// Comment rows exist only for languages whose extraction package landed.
    #[test]
    fn comment_profiles_cover_only_migrated_languages() {
        let names = LANGUAGE_COMMENT_PROFILES
            .iter()
            .map(|profile| profile.language_name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "kotlin",
                "java",
                "cpp",
                "javascript",
                "typescript",
                "tsx",
                "python",
                "rust",
                "go",
                "ruby",
                "php"
            ]
        );
        for profile in LANGUAGE_COMMENT_PROFILES {
            assert!(
                language_support_profile_for_language_name(
                    profile.language_name
                ).is_some(
                )
                // `tsx` is a dialect, not a registered surface: the registry
                // routes `.tsx` to `typescript`. It is keyed here under its
                // own name because that is the string the indexer dispatches
                // on and the CLI highlighter renders, and because
                // `typescript` itself has not migrated.
                || profile.language_name == "tsx",
                "`{}` has no public language profile",
                profile.language_name
            );
            assert!(
                !profile.line_comment.is_empty(),
                "`{}` needs a line-comment marker",
                profile.language_name
            );
        }
        assert_eq!(line_comment_for_language("kotlin"), Some("//"));
        assert_eq!(line_comment_for_language("php"), Some("//"));
        assert_eq!(line_comment_for_language("ruby"), Some("#"));
        assert_eq!(line_comment_for_language("go"), Some("//"));
        assert_eq!(line_comment_for_language("rust"), Some("//"));
        assert_eq!(line_comment_for_language("python"), Some("#"));
        assert_eq!(line_comment_for_language("tsx"), Some("//"));
        // The dialect row must not answer for the language it routes to: the
        // `typescript` roster entry is #1681's to move.
        assert_eq!(
            language_support_profile_for_ext("tsx").map(|profile| profile.language_name),
            Some("typescript")
        );
        assert_eq!(line_comment_for_language("typescript"), None);
        assert_eq!(line_comment_for_language("typescript"), Some("//"));
        // `tsx` is an indexer-only dispatch name with no public profile, so it
        // keeps answering from the CLI's own roster until #1682 lands.
        assert_eq!(line_comment_for_language("tsx"), None);
        assert_eq!(line_comment_for_language("java"), Some("//"));
        assert_eq!(line_comment_for_language("cpp"), Some("//"));
        assert_eq!(line_comment_for_language("javascript"), Some("//"));
        assert_eq!(line_comment_for_language("swift"), None);
        // `jsx` is a CLI highlighter dialect, not a registered language; it
        // must keep answering from the CLI's own residual roster.
        assert_eq!(line_comment_for_language("jsx"), None);
    }

    #[test]
    fn structural_exclusion_requires_a_workspace_relative_structural_path() {
        for relative in [
            "vendor/config.json",
            r"target\docs\guide.md",
            "build/settings.yaml",
            r"secrets\deploy.ps1",
            "web/app.min.json",
            "plugins/codestory/generated-mcp-catalog.json",
            "config/generated_service.yaml",
            "config/package-lock.json",
            "skills-lock.json",
        ] {
            assert!(is_structural_source_path(relative), "{relative}");
            assert!(
                structural_source_path_exclusion(relative).is_some(),
                "{relative}"
            );
        }
        for absolute in [
            "/tmp/vendor/repo/config.json",
            r"C:\target\repo\config.json",
            r"\\server\secrets\repo\config.json",
        ] {
            assert_eq!(
                structural_source_path_exclusion(absolute),
                None,
                "{absolute}"
            );
        }
        for root_relative in [
            "config.json",
            "generatedly-config.json",
            "lock.json",
            "docs/guide.md",
            "scripts/deploy.ps1",
            "config/service.yaml",
        ] {
            assert_eq!(
                structural_source_path_exclusion(root_relative),
                None,
                "{root_relative}"
            );
        }
        assert!(!is_structural_source_path("config.jsonc"));
    }
}
