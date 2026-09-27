//! Public publication and protocol compatibility contract.
//!
//! Two identities cross the CodeStory process boundary on every MCP session and
//! neither can be inferred from the payload itself:
//!
//! * the `_meta.codestory_publication` **stamp schema version**, which tells a
//!   reader which response vocabulary and which request-validation rules the
//!   producer is running, and
//! * the **MCP protocol revisions** this build actually implements.
//!
//! The packaged plugin ships pinned to one CLI, so in a released pair these
//! values always agree. The documented `CODESTORY_CLI` override is the one
//! supported channel that can put a different CLI behind the same launcher, and
//! the stamp is the only detector that channel leaves available. Everything in
//! this module is therefore a compatibility surface: changing a value here
//! changes what an out-of-repo consumer is entitled to believe.

use serde::{Deserialize, Serialize};

/// Schema version stamped into `_meta.codestory_publication`.
///
/// * **v0** — the stamp is absent. Readers must treat a missing stamp as v0 and
///   must not infer any newer guarantee from its absence.
/// * **v1** — the additive stamp: publication identities, `served_from`, and
///   `contract_runtime`. Tool arguments were repaired before dispatch, so a
///   request that violated the published catalog could still be served.
/// * **v2** — `tools/call` arguments are validated strictly against the
///   published catalog and rejected with JSON-RPC `-32602` instead of being
///   repaired, and `initialize` negotiates the protocol revision instead of
///   echoing whatever the client asked for.
/// * **v3** — packet, context, and search publish closed evidence projections;
///   packet truth dispositions and its evidence opt-out are no longer public.
pub const PUBLICATION_STAMP_SCHEMA_VERSION: u32 = 3;

/// Oldest reader schema version that can still interpret a payload stamped with
/// [`PUBLICATION_STAMP_SCHEMA_VERSION`] without misreading it.
///
/// v2 is deliberately not backward compatible with a v1 reader. A v1 client was
/// written against a server that repaired malformed tool arguments; the same
/// client against a v2 server receives `-32602` for requests it believes are
/// valid. Consumers must compare against this bound rather than assuming the
/// stamp is purely additive.
pub const MINIMUM_COMPATIBLE_PUBLICATION_STAMP_SCHEMA_VERSION: u32 = 3;

/// Schema version a reader must assume when `_meta.codestory_publication` is
/// absent from a response that should carry it.
pub const LEGACY_PUBLICATION_STAMP_SCHEMA_VERSION: u32 = 0;

/// MCP protocol revisions this build implements, in stable chronological order.
///
/// Advertising a revision here is a claim that the server honours it. The list
/// stays deliberately short: an unimplemented revision echoed back to a client
/// is a false compatibility claim, which is the defect this contract closes.
pub const SUPPORTED_MCP_PROTOCOL_VERSIONS: &[&str] =
    &["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

/// Revision the server answers with when the client offers nothing usable.
pub const PREFERRED_MCP_PROTOCOL_VERSION: &str = "2025-11-25";

/// Stable labels emitted for the retrieval planner's stage timing records.
///
/// These values cross the retrieval/runtime boundary in diagnostics and packet
/// traces. Consumers compare the labels directly, so their spelling is a wire
/// compatibility surface.
pub const RETRIEVAL_STAGE0_SCIP_ANCHOR_LABEL: &str = "stage0_scip_anchor";
pub const RETRIEVAL_STAGE1_LEXICAL_LABEL: &str = "stage1_lexical";
pub const RETRIEVAL_STAGE1B_SEMANTIC_LABEL: &str = "stage1b_semantic";
pub const RETRIEVAL_STAGE2_SCIP_EXPAND_LABEL: &str = "stage2_scip_expand";
pub const RETRIEVAL_STAGE3_REPO_TEXT_FALLBACK_LABEL: &str = "stage3_repo_text_fallback";

/// Outcome of one `initialize` protocol-revision negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpProtocolNegotiationStatus {
    /// The client named a revision this build implements; it is echoed.
    Agreed,
    /// The client named no revision. The server default applies and nothing was
    /// rejected.
    Defaulted,
    /// The client named a revision this build does not implement. The server
    /// answers with its own revision so the client can decide, and never
    /// claims the requested revision is supported.
    UnsupportedClientRevision,
}

impl McpProtocolNegotiationStatus {
    /// Whether the negotiated revision is one the client can rely on.
    ///
    /// `Defaulted` is compatible: the client asserted no requirement. Only a
    /// revision the client explicitly asked for and this build does not
    /// implement is incompatible.
    pub const fn compatible(self) -> bool {
        matches!(self, Self::Agreed | Self::Defaulted)
    }
}

/// Negotiated protocol revision published on the `initialize` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpProtocolNegotiationDto {
    /// Exactly what the client asked for, or `None` when it named nothing.
    pub requested: Option<String>,
    /// Revision the server will speak for this session.
    pub negotiated: String,
    /// Every revision this build implements.
    pub supported: Vec<String>,
    pub status: McpProtocolNegotiationStatus,
    /// Convenience mirror of [`McpProtocolNegotiationStatus::compatible`] for
    /// JSON consumers that cannot match on the enum.
    pub compatible: bool,
}

/// Negotiate one MCP protocol revision against [`SUPPORTED_MCP_PROTOCOL_VERSIONS`].
///
/// Never echoes an unsupported revision. A client that asked for something this
/// build does not implement receives the server's own revision plus
/// [`McpProtocolNegotiationStatus::UnsupportedClientRevision`], which is the
/// signal it needs to refuse the session.
pub fn negotiate_mcp_protocol_version(requested: Option<&str>) -> McpProtocolNegotiationDto {
    let requested = requested
        .map(str::trim)
        .filter(|requested| !requested.is_empty());
    let (negotiated, status) = match requested {
        None => (
            PREFERRED_MCP_PROTOCOL_VERSION,
            McpProtocolNegotiationStatus::Defaulted,
        ),
        Some(requested) => match SUPPORTED_MCP_PROTOCOL_VERSIONS
            .iter()
            .find(|supported| **supported == requested)
        {
            Some(supported) => (*supported, McpProtocolNegotiationStatus::Agreed),
            None => (
                PREFERRED_MCP_PROTOCOL_VERSION,
                McpProtocolNegotiationStatus::UnsupportedClientRevision,
            ),
        },
    };
    McpProtocolNegotiationDto {
        requested: requested.map(str::to_string),
        negotiated: negotiated.to_string(),
        supported: SUPPORTED_MCP_PROTOCOL_VERSIONS
            .iter()
            .map(|version| (*version).to_string())
            .collect(),
        status,
        compatible: status.compatible(),
    }
}

/// Why a reader refused a publication stamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationStampSkew {
    /// `_meta.codestory_publication` is absent: a legacy v0 producer.
    LegacyV0,
    /// The stamp exists but carries no readable `schema_version`.
    Malformed,
    /// The producer is older than this reader's minimum.
    ProducerTooOld,
    /// The producer declares a minimum this reader does not meet, or a schema
    /// version this reader has never heard of.
    ProducerTooNew,
}

impl PublicationStampSkew {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LegacyV0 => "legacy_v0",
            Self::Malformed => "malformed",
            Self::ProducerTooOld => "producer_too_old",
            Self::ProducerTooNew => "producer_too_new",
        }
    }
}

/// Decide whether a reader running [`PUBLICATION_STAMP_SCHEMA_VERSION`] may act
/// on a payload stamped `observed` and declaring `observed_minimum`.
///
/// Fail-closed: an absent stamp, an unreadable version, or a version outside
/// the mutually supported window is refused. `None` is the legacy v0 case and
/// is never silently upgraded.
///
/// This is the rule the packaged launcher applies to the runtime's `initialize`
/// result, mirrored there in JavaScript because it runs before the native
/// process is trusted. Publishing it here keeps the decision owned by the
/// contract rather than by the reader that happens to run first.
pub fn classify_publication_stamp(
    observed: Option<u32>,
    observed_minimum: Option<u32>,
) -> Result<u32, PublicationStampSkew> {
    let Some(observed) = observed else {
        return Err(PublicationStampSkew::LegacyV0);
    };
    if observed == LEGACY_PUBLICATION_STAMP_SCHEMA_VERSION {
        return Err(PublicationStampSkew::LegacyV0);
    }
    if observed > PUBLICATION_STAMP_SCHEMA_VERSION {
        return Err(PublicationStampSkew::ProducerTooNew);
    }
    if observed < MINIMUM_COMPATIBLE_PUBLICATION_STAMP_SCHEMA_VERSION {
        return Err(PublicationStampSkew::ProducerTooOld);
    }
    // A producer may raise its own floor ahead of ours; honour the stricter of
    // the two rather than assuming our version is enough.
    if observed_minimum.is_some_and(|minimum| minimum > PUBLICATION_STAMP_SCHEMA_VERSION) {
        return Err(PublicationStampSkew::ProducerTooNew);
    }
    Ok(observed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_revision_is_agreed_and_echoed() {
        let negotiation = negotiate_mcp_protocol_version(Some("2024-11-05"));

        assert_eq!(negotiation.negotiated, "2024-11-05");
        assert_eq!(negotiation.status, McpProtocolNegotiationStatus::Agreed);
        assert!(negotiation.compatible);
        assert_eq!(negotiation.requested.as_deref(), Some("2024-11-05"));
    }

    #[test]
    fn unsupported_revision_answers_with_the_server_revision() {
        let negotiation = negotiate_mcp_protocol_version(Some("2099-01-01"));

        assert_eq!(
            negotiation.negotiated, "2025-11-25",
            "an unimplemented revision must never be echoed back as supported"
        );
        assert_eq!(
            negotiation.status,
            McpProtocolNegotiationStatus::UnsupportedClientRevision
        );
        assert!(!negotiation.compatible);
        assert_eq!(negotiation.requested.as_deref(), Some("2099-01-01"));
    }

    #[test]
    fn absent_revision_defaults_without_claiming_a_client_revision() {
        for requested in [None, Some(""), Some("   ")] {
            let negotiation = negotiate_mcp_protocol_version(requested);

            assert_eq!(negotiation.negotiated, "2025-11-25");
            assert_eq!(negotiation.status, McpProtocolNegotiationStatus::Defaulted);
            assert!(negotiation.compatible);
            assert_eq!(negotiation.requested, None);
        }
    }

    #[test]
    fn missing_stamp_is_legacy_v0_and_never_upgraded() {
        assert_eq!(
            classify_publication_stamp(None, None),
            Err(PublicationStampSkew::LegacyV0)
        );
        assert_eq!(
            classify_publication_stamp(Some(LEGACY_PUBLICATION_STAMP_SCHEMA_VERSION), None),
            Err(PublicationStampSkew::LegacyV0)
        );
    }

    #[test]
    fn stamp_window_is_closed_on_both_sides() {
        assert_eq!(
            classify_publication_stamp(Some(1), None),
            Err(PublicationStampSkew::ProducerTooOld)
        );
        assert_eq!(
            classify_publication_stamp(Some(PUBLICATION_STAMP_SCHEMA_VERSION + 1), None),
            Err(PublicationStampSkew::ProducerTooNew)
        );
        assert_eq!(
            classify_publication_stamp(
                Some(PUBLICATION_STAMP_SCHEMA_VERSION),
                Some(PUBLICATION_STAMP_SCHEMA_VERSION + 1)
            ),
            Err(PublicationStampSkew::ProducerTooNew),
            "a producer floor above this reader must be refused, not ignored"
        );
        assert_eq!(
            classify_publication_stamp(
                Some(PUBLICATION_STAMP_SCHEMA_VERSION),
                Some(MINIMUM_COMPATIBLE_PUBLICATION_STAMP_SCHEMA_VERSION)
            ),
            Ok(PUBLICATION_STAMP_SCHEMA_VERSION)
        );
    }

    #[test]
    fn published_stamp_bounds_are_the_documented_values() {
        assert_eq!(
            PUBLICATION_STAMP_SCHEMA_VERSION, 3,
            "the evidence-only v3 contract publishes stamp schema 3"
        );
        assert_eq!(MINIMUM_COMPATIBLE_PUBLICATION_STAMP_SCHEMA_VERSION, 3);
        assert_eq!(LEGACY_PUBLICATION_STAMP_SCHEMA_VERSION, 0);
        assert_eq!(
            SUPPORTED_MCP_PROTOCOL_VERSIONS,
            &["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]
        );
        assert_eq!(PREFERRED_MCP_PROTOCOL_VERSION, "2025-11-25");
    }

    #[test]
    fn retrieval_stage_labels_are_the_published_wire_values() {
        assert_eq!(RETRIEVAL_STAGE0_SCIP_ANCHOR_LABEL, "stage0_scip_anchor");
        assert_eq!(RETRIEVAL_STAGE1_LEXICAL_LABEL, "stage1_lexical");
        assert_eq!(RETRIEVAL_STAGE1B_SEMANTIC_LABEL, "stage1b_semantic");
        assert_eq!(RETRIEVAL_STAGE2_SCIP_EXPAND_LABEL, "stage2_scip_expand");
        assert_eq!(
            RETRIEVAL_STAGE3_REPO_TEXT_FALLBACK_LABEL,
            "stage3_repo_text_fallback"
        );
    }
}
