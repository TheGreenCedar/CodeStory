// The one decision that says whether a build may proceed without an embedded
// model, kept in a dependency-free file so the build script and the contract
// ratchet compile the *same* definition.
//
// `build.rs` `include!`s this file; so does
// `crates/codestory-cli/tests/architecture_contracts.rs`. A build script is
// otherwise unreachable from any test target, which is how the gate came to key
// on Cargo's `DEBUG` (the debug-info setting) instead of `PROFILE` (the profile
// identity) with nothing failing: a routine `[profile.release] debug = 1` for
// symbolication set `DEBUG` to a non-`"false"` value and silently produced a
// release build with no embedded model.
//
// This file is `include!`d, not declared as a module, so it must stay free of
// inner attributes and inner doc comments.

/// True when the profile Cargo reports is one whose artifacts ship, so a build
/// without `CODESTORY_EMBED_MODEL_SOURCE` must fail rather than quietly omit
/// the model.
///
/// Cargo sets `PROFILE` to `release` for release-rooted profiles and `debug`
/// otherwise. `None` means the variable was absent or not UTF-8, which is not a
/// shipping build.
fn profile_requires_embedded_model(profile: Option<&str>) -> bool {
    matches!(profile, Some("release"))
}
