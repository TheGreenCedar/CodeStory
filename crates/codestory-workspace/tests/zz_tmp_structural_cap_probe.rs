//! TEMPORARY probe (delete after). Tests the claim that a stale structural
//! byte-bound exclusion row survives a structural-cap change.

use codestory_contracts::workspace::{
    DEFAULT_STRUCTURAL_UNIT_CAP, OVERSIZED_SOURCE_POLICY_VERSION,
    OversizedSourceExclusionCandidate, RefreshInputs, SourceIndexPolicy, WorkspaceInventory,
};
use codestory_workspace::{WorkspaceDiscovery, WorkspaceManifest};
use std::fs;
use tempfile::tempdir;

fn policy(byte_cap: u64, structural_byte_cap: u64) -> SourceIndexPolicy {
    SourceIndexPolicy {
        policy_version: OVERSIZED_SOURCE_POLICY_VERSION.to_string(),
        byte_cap,
        structural_byte_cap,
        structural_unit_cap: DEFAULT_STRUCTURAL_UNIT_CAP,
    }
}

/// Simulate the claim exactly: a core published under structural cap 1 MiB,
/// then the structural cap drops to 512 KiB with the headroom unchanged at
/// 2,000,000. Does the stale row (byte_cap = 1_048_576) survive?
#[test]
fn probe_stale_structural_row_under_a_lowered_structural_cap() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let root = temp.path().join("repo");
    fs::create_dir_all(root.join("data"))?;
    let json = root.join("data").join("config.json");
    fs::write(&json, vec![b'x'; 1_500_000])?;
    let manifest = WorkspaceManifest::open(root.clone())?;

    // Step 1: publish-time classification under the OLD structural cap.
    let old = policy(2_000_000, 1_048_576);
    let old_inventory = manifest.source_inventory_with_policy(&old)?;
    let stale_row = old_inventory
        .policy_exclusions
        .iter()
        .find(|c| c.normalized_path == "data/config.json")
        .expect("stale row")
        .clone();
    assert_eq!(stale_row.byte_cap, 1_048_576);
    assert_eq!(stale_row.observed_unit_count, 0, "byte-bound");
    println!("STALE ROW: {stale_row:?}");

    // Step 2: the future release. Structural cap 512 KiB, headroom unchanged.
    let new = policy(2_000_000, 512 * 1024);

    // 2a. Does planning carry the stale byte-bound row forward?
    let inputs = RefreshInputs {
        stored_files: Vec::new(),
        policy_exclusions: vec![stale_row.clone()],
        inventory: WorkspaceInventory::default(),
    };
    let outcome = manifest.build_execution_outcome_with_policy(&inputs, &new)?;
    println!("REPLANNED EXCLUSIONS: {:?}", outcome.policy_exclusions);
    assert_eq!(
        outcome.policy_exclusions.len(),
        1,
        "the file is still over the new cap, so it is re-derived"
    );
    assert_eq!(
        outcome.policy_exclusions[0].byte_cap,
        512 * 1024,
        "the re-derived row must name the NEW cap, not the stale 1 MiB"
    );

    // 2b. Does the publication fence accept the stale row?
    let fence = WorkspaceDiscovery.revalidate_source_policy_exclusions(
        &manifest,
        std::slice::from_ref(&stale_row),
        &new,
    );
    println!("FENCE on stale row: {fence:?}");
    assert!(
        fence.is_err(),
        "the publication fence must reject a row naming a superseded structural cap"
    );

    // 2c. The re-derived row passes the same fence.
    let verified = WorkspaceDiscovery.revalidate_source_policy_exclusions(
        &manifest,
        &outcome.policy_exclusions,
        &new,
    )?;
    assert_eq!(verified.len(), 1);
    Ok(())
}

/// The other direction: the structural cap RISES. A stale row excluding a
/// 1.5 MB JSON at 1 MiB should be re-admitted for indexing.
#[test]
fn probe_stale_structural_row_under_a_raised_structural_cap() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let root = temp.path().join("repo");
    fs::create_dir_all(root.join("data"))?;
    let json = root.join("data").join("config.json");
    fs::write(&json, vec![b'x'; 1_500_000])?;
    let manifest = WorkspaceManifest::open(root.clone())?;

    let old = policy(2_000_000, 1_048_576);
    let stale_row = manifest
        .source_inventory_with_policy(&old)?
        .policy_exclusions
        .into_iter()
        .find(|c| c.normalized_path == "data/config.json")
        .expect("stale row");

    let new = policy(2_000_000, 1_800_000);
    let inputs = RefreshInputs {
        stored_files: Vec::new(),
        policy_exclusions: vec![stale_row.clone()],
        inventory: WorkspaceInventory::default(),
    };
    let outcome = manifest.build_execution_outcome_with_policy(&inputs, &new)?;
    println!("RAISED: exclusions={:?}", outcome.policy_exclusions);
    println!("RAISED: to_index={:?}", outcome.refresh.plan.files_to_index);
    assert!(
        outcome.policy_exclusions.is_empty(),
        "the stale byte-bound row must not be carried forward"
    );
    assert!(
        outcome.refresh.plan.files_to_index.contains(&json),
        "the file must be scheduled now that it fits the raised structural cap"
    );

    let fence = WorkspaceDiscovery.revalidate_source_policy_exclusions(
        &manifest,
        std::slice::from_ref(&stale_row),
        &new,
    );
    println!("RAISED fence on stale row: {fence:?}");
    assert!(fence.is_err());
    Ok(())
}

/// A unit-bound row IS carried forward. Does it dodge the cap check?
#[test]
fn probe_unit_bound_row_carry_forward_under_a_lowered_structural_cap() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let root = temp.path().join("repo");
    fs::create_dir_all(&root)?;
    let json = root.join("evidence.json");
    fs::write(&json, "{\"one\":1,\"two\":2,\"three\":3}\n")?;
    let manifest = WorkspaceManifest::open(root.clone())?;

    let old = SourceIndexPolicy {
        policy_version: OVERSIZED_SOURCE_POLICY_VERSION.to_string(),
        byte_cap: 2_000_000,
        structural_byte_cap: 1_048_576,
        structural_unit_cap: 2,
    };
    // Borrow the crate's own content hash by classifying the file byte-bound
    // under a tiny cap first.
    let tiny = SourceIndexPolicy {
        policy_version: OVERSIZED_SOURCE_POLICY_VERSION.to_string(),
        byte_cap: 4,
        structural_byte_cap: 4,
        structural_unit_cap: 2,
    };
    let seed = manifest
        .source_inventory_with_policy(&tiny)?
        .policy_exclusions
        .into_iter()
        .find(|c| c.normalized_path == "evidence.json")
        .expect("seed row");
    let retained = OversizedSourceExclusionCandidate {
        normalized_path: "evidence.json".to_string(),
        content_hash: seed.content_hash,
        observed_size: seed.observed_size,
        observed_unit_count: 3,
        policy_version: old.policy_version.clone(),
        // Under the OLD policy a unit-bound structural row names the OLD
        // structural cap.
        byte_cap: 1_048_576,
        structural_unit_cap: 2,
    };

    let inputs = RefreshInputs {
        stored_files: Vec::new(),
        policy_exclusions: vec![retained.clone()],
        inventory: WorkspaceInventory::default(),
    };

    // Same policy: carried forward.
    let same = manifest.build_execution_outcome_with_policy(&inputs, &old)?;
    println!("UNIT same-policy exclusions: {:?}", same.policy_exclusions);

    // Lowered structural cap: is the stale unit-bound row still carried?
    let new = SourceIndexPolicy {
        structural_byte_cap: 512 * 1024,
        ..old.clone()
    };
    let lowered = manifest.build_execution_outcome_with_policy(&inputs, &new)?;
    println!("UNIT lowered exclusions: {:?}", lowered.policy_exclusions);
    println!(
        "UNIT lowered to_index: {:?}",
        lowered.refresh.plan.files_to_index
    );

    let fence = WorkspaceDiscovery.revalidate_source_policy_exclusions(
        &manifest,
        std::slice::from_ref(&retained),
        &new,
    );
    println!("UNIT fence under lowered cap: {fence:?}");
    Ok(())
}
