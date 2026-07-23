use codestory_contracts::api::{
    EmbeddingVectorEvidenceCompatibilityDto, EmbeddingVectorEvidenceMigrationDispositionDto,
};

#[test]
fn vector_evidence_migration_disposition_is_publicly_nameable() {
    let compatibility = EmbeddingVectorEvidenceCompatibilityDto {
        compatible: false,
        migration_required: true,
        migration_disposition: EmbeddingVectorEvidenceMigrationDispositionDto::RebuildRequired,
        mismatches: vec!["engine".into()],
    };

    assert!(matches!(
        compatibility.migration_disposition,
        EmbeddingVectorEvidenceMigrationDispositionDto::RebuildRequired
    ));
}
