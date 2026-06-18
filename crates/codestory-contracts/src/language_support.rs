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
    TypedSemanticEdges,
    PacketSufficientAnswerQuality,
}

impl LanguageClaimTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FilenameRoute => "filename_route",
            Self::GrammarParse => "grammar_parse",
            Self::SourceGraphExtraction => "source_graph_extraction",
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
    SemanticResolverFixture,
    PacketRuntimeArtifact,
}

impl LanguageProofRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExtensionRouting => "extension_routing",
            Self::ParserSmoke => "parser_smoke",
            Self::GraphFixture => "graph_fixture",
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
    LanguageClaimTier::SourceGraphExtraction,
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
    let ext = normalize_extension(ext);
    LANGUAGE_SUPPORT_PROFILES
        .iter()
        .find(|profile| profile.extensions.iter().any(|candidate| *candidate == ext))
}

pub fn language_support_profile_for_language_name(
    language_name: &str,
) -> Option<&'static LanguageSupportProfile> {
    let language_name = language_name.trim().to_ascii_lowercase();
    LANGUAGE_SUPPORT_PROFILES
        .iter()
        .find(|profile| profile.language_name == language_name)
}

pub fn language_name_for_path(path: Option<&str>) -> Option<&'static str> {
    let ext = path?.rsplit('.').next()?.trim_start_matches('.');
    language_support_profile_for_ext(ext).map(|profile| profile.language_name)
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
                    assert!(tiers.contains(&LanguageClaimTier::SourceGraphExtraction));
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
}
