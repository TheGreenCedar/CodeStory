use crate::api::{PacketEvidenceResolutionDto, PacketEvidenceTierDto};
use crate::graph::NodeKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageSupportMode {
    ParserBackedGraph,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageEvidenceTier {
    GraphFidelity,
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

pub const STRUCTURAL_SOURCE_PROOF_CONTRACTS: &[StructuralSourceProofContract] = &[
    StructuralSourceProofContract {
        collector_name: "github_actions_workflow",
        path_pattern: ".github/workflows/*.{yml,yaml}",
        emitted_node_kinds: GITHUB_ACTIONS_WORKFLOW_NODE_KINDS,
        source_span: "1-based source line and column span for the matched workflow, job, or step anchor",
        evidence_tier: PacketEvidenceTierDto::ExactSource,
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
        evidence_tier: PacketEvidenceTierDto::ExactSource,
        resolution: PacketEvidenceResolutionDto::SourceRangeOnly,
        confidence: 1.0,
        unsupported_shape_notes: DOCKER_COMPOSE_UNSUPPORTED_SHAPES,
        claim_boundary: "structural exact-source proof only; not parser-backed graph parity, typed semantic resolution, container runtime behavior, or packet semantic-proof admission",
        semantic_proof_allowed: false,
    },
];

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
];

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

pub fn normalize_extension(ext: &str) -> String {
    ext.trim().trim_start_matches('.').to_ascii_lowercase()
}

pub fn language_support_profile_for_ext(ext: &str) -> Option<&'static LanguageSupportProfile> {
    let ext = ext.trim().trim_start_matches('.');
    LANGUAGE_SUPPORT_PROFILES.iter().find(|profile| {
        profile
            .extensions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(ext))
    })
}

pub fn language_support_profile_for_language_name(
    language_name: &str,
) -> Option<&'static LanguageSupportProfile> {
    let language_name = language_name.trim();
    LANGUAGE_SUPPORT_PROFILES
        .iter()
        .find(|profile| profile.language_name.eq_ignore_ascii_case(language_name))
}

pub fn language_support_profile_for_path(
    path: Option<&str>,
) -> Option<&'static LanguageSupportProfile> {
    let ext = path?.rsplit('.').next()?.trim_start_matches('.');
    language_support_profile_for_ext(ext)
}

pub fn language_name_for_path(path: Option<&str>) -> Option<&'static str> {
    language_support_profile_for_path(path).map(|profile| profile.language_name)
}

pub fn parser_backed_language_name_for_path(path: Option<&str>) -> Option<&'static str> {
    language_support_profile_for_path(path)
        .filter(|profile| profile.support_mode == LanguageSupportMode::ParserBackedGraph)
        .map(|profile| profile.language_name)
}

pub fn structural_language_name_for_path(path: Option<&str>) -> Option<&'static str> {
    language_support_profile_for_path(path)
        .filter(|profile| profile.support_mode == LanguageSupportMode::StructuralCollector)
        .map(|profile| profile.language_name)
}

pub fn is_structural_language_name(language_name: &str) -> bool {
    language_support_profile_for_language_name(language_name)
        .is_some_and(|profile| profile.support_mode == LanguageSupportMode::StructuralCollector)
}

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

pub fn supported_extensions() -> impl Iterator<Item = &'static str> {
    LANGUAGE_SUPPORT_PROFILES
        .iter()
        .flat_map(|profile| profile.extensions.iter().copied())
}

pub fn language_claim_tiers_for_profile(
    profile: &LanguageSupportProfile,
) -> &'static [LanguageClaimTier] {
    match profile.support_mode {
        LanguageSupportMode::ParserBackedGraph => PARSER_BACKED_CLAIM_TIERS,
        LanguageSupportMode::StructuralCollector => STRUCTURAL_CLAIM_TIERS,
    }
}

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
    fn structural_source_proof_contract_is_exact_source_not_semantic() {
        let contract = STRUCTURAL_SOURCE_PROOF_CONTRACTS
            .iter()
            .find(|contract| contract.collector_name == "github_actions_workflow")
            .expect("github actions structural contract");
        assert_eq!(contract.path_pattern, ".github/workflows/*.{yml,yaml}");
        assert!(contract.emitted_node_kinds.contains(&NodeKind::MODULE));
        assert!(contract.emitted_node_kinds.contains(&NodeKind::FUNCTION));
        assert_eq!(contract.evidence_tier, PacketEvidenceTierDto::ExactSource);
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
            PacketEvidenceTierDto::ExactSource
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
    }

    #[test]
    fn github_actions_workflow_path_is_path_scoped_not_yaml_support() {
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
        assert!(language_support_profile_for_ext("yaml").is_none());
    }

    #[test]
    fn docker_compose_path_is_path_scoped_not_yaml_support() {
        assert!(is_docker_compose_file_path("compose.yaml"));
        assert!(is_docker_compose_file_path("deploy/compose.yml"));
        assert!(is_docker_compose_file_path("docker-compose.override.yml"));
        assert!(is_docker_compose_file_path(
            r"repo\docker\retrieval-compose.yml"
        ));
        assert!(!is_docker_compose_file_path(".github/workflows/ci.yml"));
        assert!(!is_docker_compose_file_path("openapi.yaml"));
        assert!(!is_docker_compose_file_path("docs/service.yml"));
        assert!(language_support_profile_for_ext("yaml").is_none());
    }
}
