use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{V3SurfaceSet, profile::McpRevisionV3};

pub(crate) const PUBLICATION_SCHEMA_VERSION_V3: u16 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveryContractIdentityV3 {
    pub(crate) revision: McpRevisionV3,
    pub(crate) sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeDiscoveryIdentityV3 {
    pub(crate) revision: String,
    pub(crate) discovery_sha256: String,
    pub(crate) publication_schema_version: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeSessionV3 {
    requested_revision: Option<String>,
    negotiated_revision: McpRevisionV3,
    discovery_identity: DiscoveryContractIdentityV3,
}

impl NativeSessionV3 {
    pub(crate) fn negotiate(requested: Option<&str>) -> Self {
        let requested_revision = requested.map(str::to_string);
        let negotiated_revision = requested
            .and_then(McpRevisionV3::parse)
            .unwrap_or_else(McpRevisionV3::preferred);
        let discovery_identity =
            discovery_contract_for_surface_v3(negotiated_revision, V3SurfaceSet::WithProof);
        Self {
            requested_revision,
            negotiated_revision,
            discovery_identity,
        }
    }

    pub(crate) const fn negotiated_revision(&self) -> McpRevisionV3 {
        self.negotiated_revision
    }

    pub(crate) const fn discovery_identity(&self) -> &DiscoveryContractIdentityV3 {
        &self.discovery_identity
    }

    pub(crate) fn initialize_result(&self) -> Value {
        let compatible = self
            .requested_revision
            .as_deref()
            .is_none_or(|requested| McpRevisionV3::parse(requested).is_some());
        let status = match self.requested_revision.as_deref() {
            None => "defaulted",
            Some(_) if compatible => "agreed",
            Some(_) => "unsupported_client_revision",
        };
        json!({
            "protocolVersion": self.negotiated_revision.as_str(),
            "name": "codestory",
            "version": env!("CARGO_PKG_VERSION"),
            "serverInfo": {"name":"codestory","version":env!("CARGO_PKG_VERSION")},
            "capabilities": initialize_capabilities_v3(),
            "_meta": {
                "codestory_protocol": {
                    "requested": self.requested_revision,
                    "negotiated": self.negotiated_revision.as_str(),
                    "supported": McpRevisionV3::all()
                        .iter()
                        .map(|revision| revision.as_str())
                        .collect::<Vec<_>>(),
                    "preferred": McpRevisionV3::preferred().as_str(),
                    "status": status,
                    "compatible": compatible,
                    "discovery_contract_sha256": self.discovery_identity.sha256
                },
                "codestory_publication": {
                    "schema_version": PUBLICATION_SCHEMA_VERSION_V3,
                    "minimum_compatible_schema_version": PUBLICATION_SCHEMA_VERSION_V3
                }
            }
        })
    }

    pub(crate) fn validate_runtime(
        &self,
        runtime: &RuntimeDiscoveryIdentityV3,
    ) -> Result<(), HandoffSkewV3> {
        validate_runtime_handoff_v3(&self.discovery_identity, runtime)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HandoffSkewV3 {
    NegotiatedRevision,
    DiscoveryContract,
    PublicationSchema,
}

pub(crate) fn discovery_contract_v3(revision: McpRevisionV3) -> DiscoveryContractIdentityV3 {
    // Qualification measures the sealed proof-capable contract explicitly.
    discovery_contract_for_surface_v3(revision, V3SurfaceSet::WithProof)
}

pub(crate) fn discovery_contract_for_surface_v3(
    revision: McpRevisionV3,
    surface: V3SurfaceSet,
) -> DiscoveryContractIdentityV3 {
    let document = discovery_document_v3(revision, surface);
    let canonical = canonical_json_bytes_v3(&document);
    DiscoveryContractIdentityV3 {
        revision,
        sha256: hex_v3(&Sha256::digest(canonical)),
    }
}

pub(crate) fn initialize_result_v3(requested: Option<&str>) -> Value {
    NativeSessionV3::negotiate(requested).initialize_result()
}

pub(crate) fn validate_runtime_handoff_v3(
    expected: &DiscoveryContractIdentityV3,
    runtime: &RuntimeDiscoveryIdentityV3,
) -> Result<(), HandoffSkewV3> {
    if runtime.revision != expected.revision.as_str() {
        return Err(HandoffSkewV3::NegotiatedRevision);
    }
    if runtime.discovery_sha256 != expected.sha256 {
        return Err(HandoffSkewV3::DiscoveryContract);
    }
    if runtime.publication_schema_version != PUBLICATION_SCHEMA_VERSION_V3 {
        return Err(HandoffSkewV3::PublicationSchema);
    }
    Ok(())
}

fn discovery_document_v3(revision: McpRevisionV3, surface: V3SurfaceSet) -> Value {
    json!({
        "domain": "codestory.mcp.discovery-contract.v3",
        "protocolRevision": revision.as_str(),
        "surfaceSet": surface.as_str(),
        "publicationSchemaVersion": PUBLICATION_SCHEMA_VERSION_V3,
        "initializeCapabilities": initialize_capabilities_v3(),
        "tools": super::catalog::tools_for_surface_v3(revision, surface),
        "resources": crate::stdio_catalog::resources_list_json()["result"]["resources"].clone(),
        "resourceTemplates": crate::stdio_catalog::resource_templates_list_json()["result"]["resourceTemplates"].clone(),
        "prompts": crate::stdio_catalog::prompts_list_json()["result"]["prompts"].clone()
    })
}

fn initialize_capabilities_v3() -> Value {
    json!({
        "tools": {"listChanged":false},
        "resources": {"subscribe":false,"listChanged":false},
        "prompts": {"listChanged":false}
    })
}

fn canonical_json_bytes_v3(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write_canonical_json_v3(value, &mut out);
    out
}

fn write_canonical_json_v3(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(value) => out.extend_from_slice(if *value { b"true" } else { b"false" }),
        Value::Number(value) => out.extend_from_slice(value.to_string().as_bytes()),
        Value::String(value) => out.extend_from_slice(
            serde_json::to_string(value)
                .expect("JSON string serialization cannot fail")
                .as_bytes(),
        ),
        Value::Array(values) => {
            out.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    out.push(b',');
                }
                write_canonical_json_v3(value, out);
            }
            out.push(b']');
        }
        Value::Object(values) => {
            out.push(b'{');
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(name, _)| *name);
            for (index, (name, value)) in entries.into_iter().enumerate() {
                if index != 0 {
                    out.push(b',');
                }
                out.extend_from_slice(
                    serde_json::to_string(name)
                        .expect("JSON object-key serialization cannot fail")
                        .as_bytes(),
                );
                out.push(b':');
                write_canonical_json_v3(value, out);
            }
            out.push(b'}');
        }
    }
}

fn hex_v3(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn discovery_contracts_are_deterministic_distinct_and_initialize_bound() {
        let identities = McpRevisionV3::all()
            .iter()
            .map(|revision| discovery_contract_for_surface_v3(*revision, V3SurfaceSet::WithProof))
            .collect::<Vec<_>>();
        assert_eq!(
            identities
                .iter()
                .map(|identity| identity.sha256.as_str())
                .collect::<BTreeSet<_>>()
                .len(),
            4
        );
        for identity in &identities {
            assert_eq!(identity.sha256.len(), 64);
            assert!(identity.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
            assert_eq!(
                discovery_contract_for_surface_v3(identity.revision, V3SurfaceSet::WithProof),
                *identity
            );

            let initialized = initialize_result_v3(Some(identity.revision.as_str()));
            assert_eq!(initialized["protocolVersion"], identity.revision.as_str());
            assert_eq!(
                initialized.pointer("/_meta/codestory_protocol/discovery_contract_sha256"),
                Some(&json!(identity.sha256))
            );
            assert_eq!(
                initialized.pointer("/_meta/codestory_publication/schema_version"),
                Some(&json!(PUBLICATION_SCHEMA_VERSION_V3))
            );
        }
        assert_eq!(
            initialize_result_v3(Some("2099-01-01"))["protocolVersion"],
            McpRevisionV3::preferred().as_str()
        );
    }

    #[test]
    fn evidence_only_discovery_identity_is_bound_to_the_surface_set() {
        for revision in McpRevisionV3::all() {
            let evidence = discovery_contract_for_surface_v3(
                *revision,
                super::super::V3SurfaceSet::EvidenceOnly,
            );
            let proof =
                discovery_contract_for_surface_v3(*revision, super::super::V3SurfaceSet::WithProof);
            assert_ne!(evidence.sha256, proof.sha256);
            assert_eq!(evidence.sha256.len(), 64);
        }
    }

    #[test]
    fn native_session_pins_one_negotiated_revision_and_discovery_identity() {
        let session = NativeSessionV3::negotiate(Some("2025-03-26"));
        assert_eq!(session.negotiated_revision(), McpRevisionV3::March2025);
        let first = session.initialize_result();
        let second = session.initialize_result();
        assert_eq!(first, second);
        assert_eq!(first["protocolVersion"], "2025-03-26");
        assert_eq!(
            first.pointer("/_meta/codestory_protocol/discovery_contract_sha256"),
            Some(&json!(session.discovery_identity().sha256))
        );
        assert_eq!(
            session.validate_runtime(&RuntimeDiscoveryIdentityV3 {
                revision: McpRevisionV3::November2025.as_str().into(),
                discovery_sha256: session.discovery_identity().sha256.clone(),
                publication_schema_version: PUBLICATION_SCHEMA_VERSION_V3,
            }),
            Err(HandoffSkewV3::NegotiatedRevision)
        );
    }

    #[test]
    fn old_new_and_wrong_v3_handoffs_fail_closed_by_exact_identity() {
        let expected = discovery_contract_v3(McpRevisionV3::June2025);
        let exact = RuntimeDiscoveryIdentityV3 {
            revision: expected.revision.as_str().into(),
            discovery_sha256: expected.sha256.clone(),
            publication_schema_version: PUBLICATION_SCHEMA_VERSION_V3,
        };
        assert_eq!(validate_runtime_handoff_v3(&expected, &exact), Ok(()));

        let mut old = exact.clone();
        old.revision = McpRevisionV3::November2024.as_str().into();
        assert_eq!(
            validate_runtime_handoff_v3(&expected, &old),
            Err(HandoffSkewV3::NegotiatedRevision)
        );
        let mut wrong_v3 = exact.clone();
        wrong_v3.discovery_sha256 = "f".repeat(64);
        assert_eq!(
            validate_runtime_handoff_v3(&expected, &wrong_v3),
            Err(HandoffSkewV3::DiscoveryContract)
        );
        let mut new_publication = exact;
        new_publication.publication_schema_version = 4;
        assert_eq!(
            validate_runtime_handoff_v3(&expected, &new_publication),
            Err(HandoffSkewV3::PublicationSchema)
        );
    }
}
