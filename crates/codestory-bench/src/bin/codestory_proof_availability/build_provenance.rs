pub(crate) const RUSTC_VV: &str =
    include_str!(concat!(env!("OUT_DIR"), "/codestory-proof-rustc-vv.txt"));
pub(crate) const BUILD_PROFILE: &str = include_str!(concat!(
    env!("OUT_DIR"),
    "/codestory-proof-build-profile.txt"
));
pub(crate) const SOURCE_COMMIT: &str = include_str!(concat!(
    env!("OUT_DIR"),
    "/codestory-proof-source-commit.txt"
));
pub(crate) const SOURCE_TREE: &str =
    include_str!(concat!(env!("OUT_DIR"), "/codestory-proof-source-tree.txt"));
pub(crate) const SOURCE_DIRTY: &str = include_str!(concat!(
    env!("OUT_DIR"),
    "/codestory-proof-source-dirty.txt"
));
