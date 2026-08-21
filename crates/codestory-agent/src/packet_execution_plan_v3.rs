//! Dark, pure v3 packet evidence planning.
//!
//! The module is compiled only for unit tests and the internal `test-support`
//! feature. It translates the narrow historical inputs needed by the v3
//! compatibility proof and evaluates an execution ledger without performing
//! retrieval or changing the current packet compiler.

use std::collections::{BTreeMap, BTreeSet};

use codestory_contracts::api::{PacketClaimObligationDto, PacketClaimObligationKindDto};

pub const PACKET_EXECUTION_PLAN_VERSION_V3: u32 = 3;

macro_rules! stable_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

stable_id!(DiscoveryLeadIdV3);
stable_id!(ClaimIdV3);
stable_id!(RequirementIdV3);
stable_id!(QueryIdV3);
stable_id!(ReceiptIdV3);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReceiptRequirementKindV3 {
    TypedProbeBinding {
        input_index: u32,
        path: Option<String>,
        symbol_id: Option<String>,
    },
    SourceSpan {
        path: String,
        start_byte: u32,
        end_byte: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptRequirementV3 {
    pub id: RequirementIdV3,
    pub kind: ReceiptRequirementKindV3,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryLeadV3 {
    pub id: DiscoveryLeadIdV3,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialClaimV3 {
    pub id: ClaimIdV3,
    pub requirements: Vec<ReceiptRequirementV3>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanningSubjectV3 {
    DiscoveryLead(DiscoveryLeadV3),
    MaterialClaim(MaterialClaimV3),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSpannedMaterialInputV3 {
    pub claim_id: ClaimIdV3,
    pub path: String,
    pub start_byte: u32,
    pub end_byte: u32,
}

pub enum ExistingObligationInputV3<'a> {
    Legacy(&'a PacketClaimObligationDto),
    SourceSpanned(SourceSpannedMaterialInputV3),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassifierErrorV3 {
    UnsupportedLegacyObligation,
    InvalidSourceSpan,
}

pub fn classify_existing_obligation_v3(
    input: ExistingObligationInputV3<'_>,
) -> Result<PlanningSubjectV3, ClassifierErrorV3> {
    match input {
        ExistingObligationInputV3::Legacy(obligation) => {
            if obligation.kind != PacketClaimObligationKindDto::ExactProbe {
                return Err(ClassifierErrorV3::UnsupportedLegacyObligation);
            }
            let Some(binding) = obligation.probe_binding.as_ref() else {
                return Ok(PlanningSubjectV3::DiscoveryLead(DiscoveryLeadV3 {
                    id: DiscoveryLeadIdV3::from(obligation.id.clone()),
                }));
            };
            Ok(PlanningSubjectV3::MaterialClaim(MaterialClaimV3 {
                id: ClaimIdV3::from(obligation.id.clone()),
                requirements: vec![ReceiptRequirementV3 {
                    id: RequirementIdV3::from(format!(
                        "typed_probe_binding:{}",
                        binding.input_index
                    )),
                    kind: ReceiptRequirementKindV3::TypedProbeBinding {
                        input_index: binding.input_index,
                        path: binding.path.clone(),
                        symbol_id: binding.symbol_id.clone(),
                    },
                }],
            }))
        }
        ExistingObligationInputV3::SourceSpanned(source) => {
            if source.path.is_empty() || source.start_byte >= source.end_byte {
                return Err(ClassifierErrorV3::InvalidSourceSpan);
            }
            Ok(PlanningSubjectV3::MaterialClaim(MaterialClaimV3 {
                id: source.claim_id,
                requirements: vec![ReceiptRequirementV3 {
                    id: RequirementIdV3::from("source_span"),
                    kind: ReceiptRequirementKindV3::SourceSpan {
                        path: source.path,
                        start_byte: source.start_byte,
                        end_byte: source.end_byte,
                    },
                }],
            }))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptOwnerV3 {
    DiscoveryLead(DiscoveryLeadIdV3),
    MaterialClaim(ClaimIdV3),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedReceiptV3 {
    pub id: ReceiptIdV3,
    pub owner: ReceiptOwnerV3,
    pub requirement_id: RequirementIdV3,
    pub kind: ReceiptRequirementKindV3,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuerySubjectV3 {
    DiscoveryLead(DiscoveryLeadIdV3),
    MaterialClaim(ClaimIdV3),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedQueryV3 {
    pub id: QueryIdV3,
    pub subject: QuerySubjectV3,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum QueryFailureGapV3 {
    RetrievalUnavailable,
    SourceUnavailable,
    BudgetUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryOutcomeV3 {
    Completed {
        receipt_ids: Vec<ReceiptIdV3>,
    },
    SkippedBecauseDischarged {
        claim_id: ClaimIdV3,
        receipt_ids: Vec<ReceiptIdV3>,
    },
    NotDispatched,
    Failed {
        gap: QueryFailureGapV3,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryExecutionV3 {
    pub query_id: QueryIdV3,
    pub outcome: QueryOutcomeV3,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketExecutionPlanV3 {
    pub version: u32,
    pub leads: Vec<DiscoveryLeadV3>,
    pub claims: Vec<MaterialClaimV3>,
    pub queries: Vec<PlannedQueryV3>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimDispositionV3 {
    Discharged {
        receipt_ids: Vec<ReceiptIdV3>,
    },
    Gap {
        missing_requirement_ids: Vec<RequirementIdV3>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimResultV3 {
    pub claim_id: ClaimIdV3,
    pub disposition: ClaimDispositionV3,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryResultV3 {
    pub query_id: QueryIdV3,
    pub outcome: QueryOutcomeV3,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum QueryGapV3 {
    NotDispatched {
        query_id: QueryIdV3,
    },
    Failed {
        query_id: QueryIdV3,
        gap: QueryFailureGapV3,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketExecutionLedgerV3 {
    pub claim_results: Vec<ClaimResultV3>,
    pub query_results: Vec<QueryResultV3>,
    pub query_gaps: Vec<QueryGapV3>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanValidationErrorV3 {
    WrongPlanVersion(u32),
    DuplicateLeadId(DiscoveryLeadIdV3),
    DuplicateClaimId(ClaimIdV3),
    DuplicateRequirementId {
        claim_id: ClaimIdV3,
        requirement_id: RequirementIdV3,
    },
    EmptyMaterialClaimRequirements(ClaimIdV3),
    DuplicateQueryId(QueryIdV3),
    UnknownQuerySubject(QueryIdV3),
    DuplicateReceiptId(ReceiptIdV3),
    UnknownReceiptOwner(ReceiptIdV3),
    DuplicateQueryExecution(QueryIdV3),
    UnknownQueryExecution(QueryIdV3),
    MissingQueryExecution(QueryIdV3),
    DuplicateOutcomeReceiptId {
        query_id: QueryIdV3,
        receipt_id: ReceiptIdV3,
    },
    EmptySkipReceiptSet {
        query_id: QueryIdV3,
    },
    SkipClaimDoesNotMatchQuery {
        query_id: QueryIdV3,
        claim_id: ClaimIdV3,
    },
    UnknownSkipReceipt {
        query_id: QueryIdV3,
        receipt_id: ReceiptIdV3,
    },
    ForeignSkipReceipt {
        query_id: QueryIdV3,
        receipt_id: ReceiptIdV3,
        claim_id: ClaimIdV3,
    },
    NonDischargingSkipReceiptSet {
        query_id: QueryIdV3,
        claim_id: ClaimIdV3,
    },
}

pub fn evaluate_execution_plan_v3(
    mut plan: PacketExecutionPlanV3,
    mut admitted_receipts: Vec<AdmittedReceiptV3>,
    executions: Vec<QueryExecutionV3>,
) -> Result<PacketExecutionLedgerV3, PlanValidationErrorV3> {
    if plan.version != PACKET_EXECUTION_PLAN_VERSION_V3 {
        return Err(PlanValidationErrorV3::WrongPlanVersion(plan.version));
    }

    plan.leads.sort_by(|left, right| left.id.cmp(&right.id));
    reject_adjacent_duplicate(&plan.leads, |lead| &lead.id)
        .map_err(PlanValidationErrorV3::DuplicateLeadId)?;
    plan.claims.sort_by(|left, right| left.id.cmp(&right.id));
    reject_adjacent_duplicate(&plan.claims, |claim| &claim.id)
        .map_err(PlanValidationErrorV3::DuplicateClaimId)?;
    for claim in &mut plan.claims {
        if claim.requirements.is_empty() {
            return Err(PlanValidationErrorV3::EmptyMaterialClaimRequirements(
                claim.id.clone(),
            ));
        }
        claim
            .requirements
            .sort_by(|left, right| left.id.cmp(&right.id));
        if let Err(requirement_id) =
            reject_adjacent_duplicate(&claim.requirements, |requirement| &requirement.id)
        {
            return Err(PlanValidationErrorV3::DuplicateRequirementId {
                claim_id: claim.id.clone(),
                requirement_id,
            });
        }
    }
    plan.queries.sort_by(|left, right| left.id.cmp(&right.id));
    reject_adjacent_duplicate(&plan.queries, |query| &query.id)
        .map_err(PlanValidationErrorV3::DuplicateQueryId)?;

    let lead_ids = plan
        .leads
        .iter()
        .map(|lead| lead.id.clone())
        .collect::<BTreeSet<_>>();
    let claims = plan
        .claims
        .iter()
        .map(|claim| (claim.id.clone(), claim))
        .collect::<BTreeMap<_, _>>();
    for query in &plan.queries {
        let subject_exists = match &query.subject {
            QuerySubjectV3::DiscoveryLead(id) => lead_ids.contains(id),
            QuerySubjectV3::MaterialClaim(id) => claims.contains_key(id),
        };
        if !subject_exists {
            return Err(PlanValidationErrorV3::UnknownQuerySubject(query.id.clone()));
        }
    }

    admitted_receipts.sort_by(|left, right| left.id.cmp(&right.id));
    reject_adjacent_duplicate(&admitted_receipts, |receipt| &receipt.id)
        .map_err(PlanValidationErrorV3::DuplicateReceiptId)?;
    for receipt in &admitted_receipts {
        let owner_exists = match &receipt.owner {
            ReceiptOwnerV3::DiscoveryLead(id) => lead_ids.contains(id),
            ReceiptOwnerV3::MaterialClaim(id) => claims.contains_key(id),
        };
        if !owner_exists {
            return Err(PlanValidationErrorV3::UnknownReceiptOwner(
                receipt.id.clone(),
            ));
        }
    }
    let receipts_by_id = admitted_receipts
        .iter()
        .map(|receipt| (receipt.id.clone(), receipt))
        .collect::<BTreeMap<_, _>>();

    let planned_queries = plan
        .queries
        .iter()
        .map(|query| (query.id.clone(), query))
        .collect::<BTreeMap<_, _>>();
    let mut executions_by_id = BTreeMap::new();
    for mut execution in executions {
        let Some(query) = planned_queries.get(&execution.query_id) else {
            return Err(PlanValidationErrorV3::UnknownQueryExecution(
                execution.query_id,
            ));
        };
        if let Err(receipt_id) = normalize_receipt_ids(&mut execution.outcome) {
            return Err(PlanValidationErrorV3::DuplicateOutcomeReceiptId {
                query_id: execution.query_id,
                receipt_id,
            });
        }
        if executions_by_id
            .insert(execution.query_id.clone(), execution)
            .is_some()
        {
            return Err(PlanValidationErrorV3::DuplicateQueryExecution(
                query.id.clone(),
            ));
        }
    }
    for query in &plan.queries {
        if !executions_by_id.contains_key(&query.id) {
            return Err(PlanValidationErrorV3::MissingQueryExecution(
                query.id.clone(),
            ));
        }
    }

    for execution in executions_by_id.values() {
        let QueryOutcomeV3::SkippedBecauseDischarged {
            claim_id,
            receipt_ids,
        } = &execution.outcome
        else {
            continue;
        };
        if receipt_ids.is_empty() {
            return Err(PlanValidationErrorV3::EmptySkipReceiptSet {
                query_id: execution.query_id.clone(),
            });
        }
        let query = planned_queries
            .get(&execution.query_id)
            .expect("executions were checked against planned queries");
        if query.subject != QuerySubjectV3::MaterialClaim(claim_id.clone()) {
            return Err(PlanValidationErrorV3::SkipClaimDoesNotMatchQuery {
                query_id: execution.query_id.clone(),
                claim_id: claim_id.clone(),
            });
        }
        let claim = claims
            .get(claim_id)
            .expect("a planned material query references an existing claim");
        let mut skip_receipts = Vec::with_capacity(receipt_ids.len());
        for receipt_id in receipt_ids {
            let Some(receipt) = receipts_by_id.get(receipt_id) else {
                return Err(PlanValidationErrorV3::UnknownSkipReceipt {
                    query_id: execution.query_id.clone(),
                    receipt_id: receipt_id.clone(),
                });
            };
            if receipt.owner != ReceiptOwnerV3::MaterialClaim(claim_id.clone()) {
                return Err(PlanValidationErrorV3::ForeignSkipReceipt {
                    query_id: execution.query_id.clone(),
                    receipt_id: receipt_id.clone(),
                    claim_id: claim_id.clone(),
                });
            }
            skip_receipts.push(*receipt);
        }
        if !receipts_discharge_claim(claim, skip_receipts.into_iter()) {
            return Err(PlanValidationErrorV3::NonDischargingSkipReceiptSet {
                query_id: execution.query_id.clone(),
                claim_id: claim_id.clone(),
            });
        }
    }

    let claim_results = plan
        .claims
        .iter()
        .map(|claim| {
            let exact_receipts = admitted_receipts
                .iter()
                .filter(|receipt| receipt.owner == ReceiptOwnerV3::MaterialClaim(claim.id.clone()));
            let disposition = claim_disposition(claim, exact_receipts);
            ClaimResultV3 {
                claim_id: claim.id.clone(),
                disposition,
            }
        })
        .collect();
    let query_results = executions_by_id
        .into_values()
        .map(|execution| QueryResultV3 {
            query_id: execution.query_id,
            outcome: execution.outcome,
        })
        .collect::<Vec<_>>();
    let mut query_gaps = query_results
        .iter()
        .filter_map(|result| match &result.outcome {
            QueryOutcomeV3::NotDispatched => Some(QueryGapV3::NotDispatched {
                query_id: result.query_id.clone(),
            }),
            QueryOutcomeV3::Failed { gap } => Some(QueryGapV3::Failed {
                query_id: result.query_id.clone(),
                gap: gap.clone(),
            }),
            QueryOutcomeV3::Completed { .. } | QueryOutcomeV3::SkippedBecauseDischarged { .. } => {
                None
            }
        })
        .collect::<Vec<_>>();
    query_gaps.sort();

    Ok(PacketExecutionLedgerV3 {
        claim_results,
        query_results,
        query_gaps,
    })
}

fn reject_adjacent_duplicate<T, I, F>(values: &[T], identity: F) -> Result<(), I>
where
    I: Clone + Eq,
    F: Fn(&T) -> &I,
{
    for pair in values.windows(2) {
        if identity(&pair[0]) == identity(&pair[1]) {
            return Err(identity(&pair[0]).clone());
        }
    }
    Ok(())
}

fn normalize_receipt_ids(outcome: &mut QueryOutcomeV3) -> Result<(), ReceiptIdV3> {
    let receipt_ids = match outcome {
        QueryOutcomeV3::Completed { receipt_ids }
        | QueryOutcomeV3::SkippedBecauseDischarged { receipt_ids, .. } => receipt_ids,
        QueryOutcomeV3::NotDispatched | QueryOutcomeV3::Failed { .. } => return Ok(()),
    };
    receipt_ids.sort();
    reject_adjacent_duplicate(receipt_ids, |receipt_id| receipt_id)
}

fn receipts_discharge_claim<'a>(
    claim: &MaterialClaimV3,
    receipts: impl Iterator<Item = &'a AdmittedReceiptV3> + Clone,
) -> bool {
    claim.requirements.iter().all(|requirement| {
        receipts.clone().any(|receipt| {
            receipt.requirement_id == requirement.id && receipt.kind == requirement.kind
        })
    })
}

fn claim_disposition<'a>(
    claim: &MaterialClaimV3,
    receipts: impl Iterator<Item = &'a AdmittedReceiptV3> + Clone,
) -> ClaimDispositionV3 {
    let missing_requirement_ids = claim
        .requirements
        .iter()
        .filter(|requirement| {
            !receipts.clone().any(|receipt| {
                receipt.requirement_id == requirement.id && receipt.kind == requirement.kind
            })
        })
        .map(|requirement| requirement.id.clone())
        .collect::<Vec<_>>();
    if missing_requirement_ids.is_empty() {
        ClaimDispositionV3::Discharged {
            receipt_ids: receipts
                .filter(|receipt| {
                    claim.requirements.iter().any(|requirement| {
                        receipt.requirement_id == requirement.id && receipt.kind == requirement.kind
                    })
                })
                .map(|receipt| receipt.id.clone())
                .collect::<Vec<_>>(),
        }
    } else {
        ClaimDispositionV3::Gap {
            missing_requirement_ids,
        }
    }
}

#[cfg(test)]
mod tests {
    use codestory_contracts::api::{
        PACKET_OBLIGATION_PLAN_VERSION, PacketClaimObligationDto, PacketClaimObligationKindDto,
        PacketObligationProofStatusDto, PacketProbeDto, PacketProbeResolutionDto,
        PacketProbeResolutionStatusDto,
    };

    use super::*;

    fn legacy_exact(probe_binding: Option<PacketProbeResolutionDto>) -> PacketClaimObligationDto {
        PacketClaimObligationDto {
            id: "exact:entry".to_owned(),
            kind: PacketClaimObligationKindDto::ExactProbe,
            binding_terms: vec!["entry".to_owned()],
            probe_binding,
            material: true,
            allowed_node_kinds: Vec::new(),
            required_edge_kind: None,
            requires_complete_discovery: false,
            proof_status: PacketObligationProofStatusDto::Planned,
            reason: None,
            carrier_node_ids: Vec::new(),
            carrier_paths: Vec::new(),
            carrier_edge_proofs: Vec::new(),
            open_next_candidates: vec!["entry".to_owned()],
        }
    }

    fn binding() -> PacketProbeResolutionDto {
        PacketProbeResolutionDto {
            input_index: 0,
            probe: PacketProbeDto::FileSymbol {
                path: "src/lib.rs".to_owned(),
                symbol: "entry".to_owned(),
            },
            status: PacketProbeResolutionStatusDto::IndexedSymbol,
            normalized_query: Some("entry".to_owned()),
            path: Some("src/lib.rs".to_owned()),
            symbol_id: Some("crate::entry".to_owned()),
            candidates: Vec::new(),
            rejection: None,
        }
    }

    fn requirement(id: &str) -> ReceiptRequirementV3 {
        ReceiptRequirementV3 {
            id: RequirementIdV3::from(id),
            kind: ReceiptRequirementKindV3::TypedProbeBinding {
                input_index: 0,
                path: Some("src/lib.rs".to_owned()),
                symbol_id: Some("crate::entry".to_owned()),
            },
        }
    }

    fn claim(id: &str, requirement_ids: &[&str]) -> MaterialClaimV3 {
        MaterialClaimV3 {
            id: ClaimIdV3::from(id),
            requirements: requirement_ids.iter().map(|id| requirement(id)).collect(),
        }
    }

    fn lead(id: &str) -> DiscoveryLeadV3 {
        DiscoveryLeadV3 {
            id: DiscoveryLeadIdV3::from(id),
        }
    }

    fn receipt(id: &str, claim_id: &str, requirement_id: &str) -> AdmittedReceiptV3 {
        AdmittedReceiptV3 {
            id: ReceiptIdV3::from(id),
            owner: ReceiptOwnerV3::MaterialClaim(ClaimIdV3::from(claim_id)),
            requirement_id: RequirementIdV3::from(requirement_id),
            kind: requirement(requirement_id).kind,
        }
    }

    fn query(id: &str, claim_id: &str) -> PlannedQueryV3 {
        PlannedQueryV3 {
            id: QueryIdV3::from(id),
            subject: QuerySubjectV3::MaterialClaim(ClaimIdV3::from(claim_id)),
        }
    }

    fn execution(id: &str, outcome: QueryOutcomeV3) -> QueryExecutionV3 {
        QueryExecutionV3 {
            query_id: QueryIdV3::from(id),
            outcome,
        }
    }

    fn plan(
        leads: Vec<DiscoveryLeadV3>,
        claims: Vec<MaterialClaimV3>,
        queries: Vec<PlannedQueryV3>,
    ) -> PacketExecutionPlanV3 {
        PacketExecutionPlanV3 {
            version: PACKET_EXECUTION_PLAN_VERSION_V3,
            leads,
            claims,
            queries,
        }
    }

    #[test]
    fn packet_execution_plan_v3_classifies_historical_inputs_without_legacy_authority() {
        assert_eq!(PACKET_OBLIGATION_PLAN_VERSION, 1);
        assert_ne!(
            PACKET_EXECUTION_PLAN_VERSION_V3,
            PACKET_OBLIGATION_PLAN_VERSION
        );

        let unbound = legacy_exact(None);
        assert_eq!(
            classify_existing_obligation_v3(ExistingObligationInputV3::Legacy(&unbound))
                .expect("classify generated exact probe"),
            PlanningSubjectV3::DiscoveryLead(lead("exact:entry"))
        );

        let bound = legacy_exact(Some(binding()));
        let PlanningSubjectV3::MaterialClaim(bound_claim) =
            classify_existing_obligation_v3(ExistingObligationInputV3::Legacy(&bound))
                .expect("classify bound probe")
        else {
            panic!("bound typed probe must be material");
        };
        assert_eq!(bound_claim.id, ClaimIdV3::from("exact:entry"));
        assert_eq!(bound_claim.requirements.len(), 1);

        let PlanningSubjectV3::MaterialClaim(source_claim) = classify_existing_obligation_v3(
            ExistingObligationInputV3::SourceSpanned(SourceSpannedMaterialInputV3 {
                claim_id: ClaimIdV3::from("source:entry"),
                path: "src/lib.rs".to_owned(),
                start_byte: 12,
                end_byte: 44,
            }),
        )
        .expect("classify source-spanned claim") else {
            panic!("source-spanned language must be material");
        };
        assert_eq!(source_claim.requirements.len(), 1);
        assert!(matches!(
            source_claim.requirements[0].kind,
            ReceiptRequirementKindV3::SourceSpan { .. }
        ));
    }

    #[test]
    fn packet_execution_plan_v3_discharge_requires_separately_admitted_exact_receipts() {
        let material = claim("claim-a", &["requirement-a"]);
        let planned = plan(
            Vec::new(),
            vec![material],
            vec![query("query-a", "claim-a")],
        );

        for receipt_ids in [Vec::new(), vec![ReceiptIdV3::from("receipt-a")]] {
            let ledger = evaluate_execution_plan_v3(
                planned.clone(),
                Vec::new(),
                vec![execution(
                    "query-a",
                    QueryOutcomeV3::Completed { receipt_ids },
                )],
            )
            .expect("completed query is a valid execution record");
            assert_eq!(
                ledger.claim_results[0].disposition,
                ClaimDispositionV3::Gap {
                    missing_requirement_ids: vec![RequirementIdV3::from("requirement-a")],
                }
            );
        }

        let discovery_receipt = AdmittedReceiptV3 {
            id: ReceiptIdV3::from("receipt-a"),
            owner: ReceiptOwnerV3::DiscoveryLead(DiscoveryLeadIdV3::from("lead-a")),
            requirement_id: RequirementIdV3::from("requirement-a"),
            kind: requirement("requirement-a").kind,
        };
        let ledger = evaluate_execution_plan_v3(
            plan(
                vec![lead("lead-a")],
                vec![claim("claim-a", &["requirement-a"])],
                vec![query("query-a", "claim-a")],
            ),
            vec![discovery_receipt],
            vec![execution(
                "query-a",
                QueryOutcomeV3::Completed {
                    receipt_ids: vec![ReceiptIdV3::from("receipt-a")],
                },
            )],
        )
        .expect("discovery evidence remains admissible as evidence");
        assert!(matches!(
            ledger.claim_results[0].disposition,
            ClaimDispositionV3::Gap { .. }
        ));
    }

    #[test]
    fn packet_execution_plan_v3_discharge_retains_only_matching_receipts() {
        let ledger = evaluate_execution_plan_v3(
            plan(
                Vec::new(),
                vec![claim("claim-a", &["requirement-a"])],
                vec![query("query-a", "claim-a")],
            ),
            vec![
                receipt("receipt-unused", "claim-a", "other-requirement"),
                receipt("receipt-exact", "claim-a", "requirement-a"),
            ],
            vec![execution(
                "query-a",
                QueryOutcomeV3::Completed {
                    receipt_ids: vec![ReceiptIdV3::from("receipt-exact")],
                },
            )],
        )
        .expect("evaluate exact receipt retention");

        assert_eq!(
            ledger.claim_results[0].disposition,
            ClaimDispositionV3::Discharged {
                receipt_ids: vec![ReceiptIdV3::from("receipt-exact")],
            }
        );
    }

    #[test]
    fn packet_execution_plan_v3_not_dispatched_and_failed_are_exact_gaps() {
        let ledger = evaluate_execution_plan_v3(
            plan(
                Vec::new(),
                vec![claim("claim-a", &["requirement-a"])],
                vec![query("query-b", "claim-a"), query("query-a", "claim-a")],
            ),
            vec![receipt("receipt-a", "claim-a", "requirement-a")],
            vec![
                execution(
                    "query-b",
                    QueryOutcomeV3::Failed {
                        gap: QueryFailureGapV3::RetrievalUnavailable,
                    },
                ),
                execution("query-a", QueryOutcomeV3::NotDispatched),
            ],
        )
        .expect("evaluate query gaps");

        assert_eq!(
            ledger.query_gaps,
            vec![
                QueryGapV3::NotDispatched {
                    query_id: QueryIdV3::from("query-a"),
                },
                QueryGapV3::Failed {
                    query_id: QueryIdV3::from("query-b"),
                    gap: QueryFailureGapV3::RetrievalUnavailable,
                },
            ]
        );
    }

    #[test]
    fn packet_execution_plan_v3_receipt_backed_skip_fails_closed() {
        let base_plan = || {
            plan(
                Vec::new(),
                vec![
                    claim("claim-a", &["requirement-a"]),
                    claim("claim-b", &["requirement-b"]),
                ],
                vec![query("query-a", "claim-a")],
            )
        };
        let skip = |claim_id: &str, receipt_ids: Vec<ReceiptIdV3>| {
            vec![execution(
                "query-a",
                QueryOutcomeV3::SkippedBecauseDischarged {
                    claim_id: ClaimIdV3::from(claim_id),
                    receipt_ids,
                },
            )]
        };

        assert_eq!(
            evaluate_execution_plan_v3(base_plan(), Vec::new(), skip("claim-a", Vec::new())),
            Err(PlanValidationErrorV3::EmptySkipReceiptSet {
                query_id: QueryIdV3::from("query-a")
            })
        );
        assert_eq!(
            evaluate_execution_plan_v3(
                base_plan(),
                Vec::new(),
                skip("claim-a", vec![ReceiptIdV3::from("unknown")])
            ),
            Err(PlanValidationErrorV3::UnknownSkipReceipt {
                query_id: QueryIdV3::from("query-a"),
                receipt_id: ReceiptIdV3::from("unknown"),
            })
        );
        assert_eq!(
            evaluate_execution_plan_v3(
                base_plan(),
                vec![receipt("receipt-b", "claim-b", "requirement-b")],
                skip("claim-a", vec![ReceiptIdV3::from("receipt-b")])
            ),
            Err(PlanValidationErrorV3::ForeignSkipReceipt {
                query_id: QueryIdV3::from("query-a"),
                receipt_id: ReceiptIdV3::from("receipt-b"),
                claim_id: ClaimIdV3::from("claim-a"),
            })
        );
        assert_eq!(
            evaluate_execution_plan_v3(
                base_plan(),
                vec![receipt("receipt-a", "claim-a", "wrong-requirement")],
                skip("claim-a", vec![ReceiptIdV3::from("receipt-a")])
            ),
            Err(PlanValidationErrorV3::NonDischargingSkipReceiptSet {
                query_id: QueryIdV3::from("query-a"),
                claim_id: ClaimIdV3::from("claim-a"),
            })
        );
        assert_eq!(
            evaluate_execution_plan_v3(
                base_plan(),
                vec![receipt("receipt-a", "claim-a", "requirement-a")],
                skip("claim-b", vec![ReceiptIdV3::from("receipt-a")])
            ),
            Err(PlanValidationErrorV3::SkipClaimDoesNotMatchQuery {
                query_id: QueryIdV3::from("query-a"),
                claim_id: ClaimIdV3::from("claim-b"),
            })
        );

        let ledger = evaluate_execution_plan_v3(
            base_plan(),
            vec![receipt("receipt-a", "claim-a", "requirement-a")],
            skip("claim-a", vec![ReceiptIdV3::from("receipt-a")]),
        )
        .expect("exact admitted discharging skip");
        assert_eq!(
            ledger.query_results[0].outcome,
            QueryOutcomeV3::SkippedBecauseDischarged {
                claim_id: ClaimIdV3::from("claim-a"),
                receipt_ids: vec![ReceiptIdV3::from("receipt-a")],
            }
        );
    }

    #[test]
    fn packet_execution_plan_v3_duplicate_identities_fail_closed() {
        let duplicate_claim = claim("claim-a", &["requirement-a"]);
        assert_eq!(
            evaluate_execution_plan_v3(
                plan(
                    Vec::new(),
                    vec![duplicate_claim.clone(), duplicate_claim],
                    Vec::new()
                ),
                Vec::new(),
                Vec::new()
            ),
            Err(PlanValidationErrorV3::DuplicateClaimId(ClaimIdV3::from(
                "claim-a"
            )))
        );
        assert_eq!(
            evaluate_execution_plan_v3(
                plan(
                    Vec::new(),
                    vec![claim("claim-a", &["requirement-a"])],
                    vec![query("query-a", "claim-a"), query("query-a", "claim-a")]
                ),
                Vec::new(),
                Vec::new()
            ),
            Err(PlanValidationErrorV3::DuplicateQueryId(QueryIdV3::from(
                "query-a"
            )))
        );
        let duplicate_receipt = receipt("receipt-a", "claim-a", "requirement-a");
        assert_eq!(
            evaluate_execution_plan_v3(
                plan(
                    Vec::new(),
                    vec![claim("claim-a", &["requirement-a"])],
                    Vec::new()
                ),
                vec![duplicate_receipt.clone(), duplicate_receipt],
                Vec::new()
            ),
            Err(PlanValidationErrorV3::DuplicateReceiptId(
                ReceiptIdV3::from("receipt-a")
            ))
        );
        assert_eq!(
            evaluate_execution_plan_v3(
                plan(
                    Vec::new(),
                    vec![claim("claim-a", &["requirement-a"])],
                    vec![query("query-a", "claim-a")]
                ),
                vec![receipt("receipt-a", "claim-a", "requirement-a")],
                vec![execution(
                    "query-a",
                    QueryOutcomeV3::Completed {
                        receipt_ids: vec![
                            ReceiptIdV3::from("receipt-a"),
                            ReceiptIdV3::from("receipt-a"),
                        ],
                    },
                )]
            ),
            Err(PlanValidationErrorV3::DuplicateOutcomeReceiptId {
                query_id: QueryIdV3::from("query-a"),
                receipt_id: ReceiptIdV3::from("receipt-a"),
            })
        );
    }

    #[test]
    fn packet_execution_plan_v3_ordering_is_identity_stable() {
        let first = evaluate_execution_plan_v3(
            plan(
                Vec::new(),
                vec![
                    claim("claim-b", &["requirement-b"]),
                    claim("claim-a", &["requirement-a"]),
                ],
                vec![query("query-b", "claim-b"), query("query-a", "claim-a")],
            ),
            vec![
                receipt("receipt-b", "claim-b", "requirement-b"),
                receipt("receipt-a", "claim-a", "requirement-a"),
            ],
            vec![
                execution("query-b", QueryOutcomeV3::NotDispatched),
                execution(
                    "query-a",
                    QueryOutcomeV3::Completed {
                        receipt_ids: vec![ReceiptIdV3::from("receipt-a")],
                    },
                ),
            ],
        )
        .expect("evaluate first ordering");
        let second = evaluate_execution_plan_v3(
            plan(
                Vec::new(),
                vec![
                    claim("claim-a", &["requirement-a"]),
                    claim("claim-b", &["requirement-b"]),
                ],
                vec![query("query-a", "claim-a"), query("query-b", "claim-b")],
            ),
            vec![
                receipt("receipt-a", "claim-a", "requirement-a"),
                receipt("receipt-b", "claim-b", "requirement-b"),
            ],
            vec![
                execution(
                    "query-a",
                    QueryOutcomeV3::Completed {
                        receipt_ids: vec![ReceiptIdV3::from("receipt-a")],
                    },
                ),
                execution("query-b", QueryOutcomeV3::NotDispatched),
            ],
        )
        .expect("evaluate second ordering");

        assert_eq!(first, second);
        assert_eq!(first.claim_results[0].claim_id, ClaimIdV3::from("claim-a"));
        assert_eq!(first.query_results[0].query_id, QueryIdV3::from("query-a"));
    }
}
