//! TMP refutation probe: is (path=".../alpha.rs", byte_cap=1_048_576) producible?
use codestory_contracts::workspace::SourceIndexPolicy;

#[test]
fn tmp_probe_a_rs_path_can_carry_the_1_mib_cap() {
    // Exactly what CODESTORY_INDEX_SOURCE_FILE_BYTE_CAP=1048576 yields via
    // codestory-cli config.rs `source_index_policy_from_env_value`.
    let policy = SourceIndexPolicy::oversized(1_048_576);
    let cap = policy.effective_byte_cap("src/router/alpha.rs");
    println!("PROBE effective_byte_cap(src/router/alpha.rs) = {cap}");
    assert_eq!(cap, 1_048_576);

    let default = SourceIndexPolicy::default();
    println!(
        "PROBE default effective_byte_cap(.rs) = {}, (.json) = {}",
        default.effective_byte_cap("src/router/alpha.rs"),
        default.effective_byte_cap("docs/api.json")
    );
}
