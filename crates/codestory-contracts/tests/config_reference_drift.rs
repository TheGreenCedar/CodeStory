use std::path::PathBuf;

use codestory_contracts::config_registry::{
    CONFIGURATION_REFERENCE_PATH, render_configuration_reference,
};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("contracts crate has a crates parent")
        .parent()
        .expect("workspace root exists")
        .to_path_buf()
}

#[test]
fn checked_in_configuration_reference_matches_the_registry() {
    let path = repo_root().join(CONFIGURATION_REFERENCE_PATH);
    let checked_in = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    assert_eq!(
        checked_in,
        render_configuration_reference(),
        "{CONFIGURATION_REFERENCE_PATH} is stale; run \
         `cargo run -p codestory-contracts --bin generate_config_docs`"
    );
}
