// The catalog delivery states, named once.
//
// Catalog publication is delivery, not a release gate, so a release can be proved against
// either the live public catalog or a catalog pinned to the exact published commit. Deferred and
// automatically restored delivery are DISTINCT local-fixture states, and neither may quietly read
// as live publication. So no state is an absence: each has its own installer identity, its own
// attestation repository name, and its own accepted shape in the Python predicate, and the
// three names live here so the producer and the verifier cannot drift apart.
//
// `.github/scripts/packaged_agent_proof/marketplace_installation.py` holds the Python side of
// the same contract; `install-codestory-marketplace-proof.test.mjs` asserts the two agree.

/** Installer identity for a resolve through the live public catalog. */
export const LIVE_INSTALLATION_SOURCE = "codex_marketplace_install";
/** Installer identity for a resolve through a catalog pinned to the published commit. */
export const DEFERRED_INSTALLATION_SOURCE = "codex_marketplace_deferred_fixture";
/** Installer identity after a failed live-catalog smoke restored the previous public pin. */
export const RESTORED_INSTALLATION_SOURCE = "codex_marketplace_restored_fixture";

/** `marketplace.repository` for the live state: the real catalog repository. */
export const LIVE_MARKETPLACE_REPOSITORY = "TheGreenCedar/AgentPluginMarketplace";
/**
 * `marketplace.repository` for the deferred state. Deliberately not a filesystem path: the
 * path is a per-run temporary directory, and writing it here made the attestation claim a
 * "repository" that no one can resolve. This name is stable, is not a repository, and cannot
 * be mistaken for one.
 */
export const DEFERRED_MARKETPLACE_REPOSITORY = "local:candidate-pinned-marketplace-fixture";
/** `marketplace.repository` for proof after the public catalog was automatically restored. */
export const RESTORED_MARKETPLACE_REPOSITORY = "local:candidate-pinned-marketplace-restored-fixture";

/** Marker file the fixture builder writes so a fixture can identify itself to the verifier. */
export const FIXTURE_MARKER_FILENAME = ".codestory-marketplace-fixture.json";
export const FIXTURE_MARKER_PURPOSE = "codestory-candidate-pinned-marketplace-fixture";
