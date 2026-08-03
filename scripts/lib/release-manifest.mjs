// The native lane's archive-digest owner.
//
// A frozen source tree cannot carry native archive digests. The archives are built *from* the
// frozen head, so a digest committed to that head would have to describe bytes that do not exist
// yet, and a post-build source commit would invalidate the freeze and the calibration lineage.
// bump-version.mjs therefore deletes `archives` on a native bump and the resulting pin is lawful
// without them. The digests live here instead: a manifest generated from the archives the release
// actually built, carrying the release identity those archives belong to.
//
// Containment, not closure. Until the Ed25519 signature arms, a manifest fetched over the same
// channel as the archive is corruption detection, not authentication. Nothing in this module
// claims otherwise, and the plugin lane's source pin -- which ships inside the reviewed plugin
// package rather than beside the archive -- remains a separate, independently owned assertion.

import { PINNED_ARCHIVE_TARGETS } from "./pinned-archive-digests.mjs";

export const RELEASE_MANIFEST_DOMAIN = "codestory.release-manifest";
export const RELEASE_MANIFEST_SCHEMA_VERSION = 1;
export const RELEASE_MANIFEST_ASSET = "codestory-release-manifest.json";

const SHA256 = /^[0-9a-f]{64}$/u;
const COMMIT = /^[0-9a-f]{40}$/u;
const SEMVER = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/u;
const MANIFEST_TARGETS = Object.freeze(Object.keys(PINNED_ARCHIVE_TARGETS).sort());

function refuse(message) {
  throw new Error(`release manifest ${message}`);
}

function plainObject(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/// Every check the manifest has to survive before any digest read is allowed. Callers never see a
/// partially validated manifest: `validateReleaseManifest` either returns the manifest or throws.
export function validateReleaseManifest(manifest) {
  if (!plainObject(manifest)) refuse("must be a JSON object");
  if (manifest.domain !== RELEASE_MANIFEST_DOMAIN) {
    refuse(`domain must be ${RELEASE_MANIFEST_DOMAIN}, got ${JSON.stringify(manifest.domain)}`);
  }
  if (manifest.schema_version !== RELEASE_MANIFEST_SCHEMA_VERSION) {
    refuse(
      `schema_version must be ${RELEASE_MANIFEST_SCHEMA_VERSION}, ` +
        `got ${JSON.stringify(manifest.schema_version)}`,
    );
  }
  if (typeof manifest.version !== "string" || !SEMVER.test(manifest.version)) {
    refuse(`version must be semver, got ${JSON.stringify(manifest.version)}`);
  }
  if (manifest.tag !== `v${manifest.version}`) {
    refuse(`tag must be v${manifest.version}, got ${JSON.stringify(manifest.tag)}`);
  }
  if (typeof manifest.commit !== "string" || !COMMIT.test(manifest.commit)) {
    refuse(`commit must be a 40-character lowercase hexadecimal id, got ${JSON.stringify(manifest.commit)}`);
  }
  if (!plainObject(manifest.archives)) refuse("archives must be a JSON object");
  const present = Object.keys(manifest.archives).sort();
  if (JSON.stringify(present) !== JSON.stringify(MANIFEST_TARGETS)) {
    // A manifest missing a target is an incomplete release, and a manifest carrying an unknown one
    // describes bytes no lane can name. Both are refused rather than partially trusted.
    refuse(`archives must name exactly ${MANIFEST_TARGETS.join(", ")}, got ${present.join(", ") || "nothing"}`);
  }
  for (const target of MANIFEST_TARGETS) {
    const entry = manifest.archives[target];
    if (!plainObject(entry)) refuse(`${target} entry must be a JSON object`);
    const expectedFilename = PINNED_ARCHIVE_TARGETS[target](manifest.version);
    if (entry.filename !== expectedFilename) {
      refuse(`${target} filename must be ${expectedFilename}, got ${JSON.stringify(entry.filename)}`);
    }
    if (!Number.isSafeInteger(entry.bytes) || entry.bytes <= 0) {
      refuse(`${target} bytes must be a positive integer, got ${JSON.stringify(entry.bytes)}`);
    }
    if (typeof entry.sha256 !== "string" || !SHA256.test(entry.sha256)) {
      refuse(`${target} sha256 must be 64 lowercase hexadecimal characters, got ${JSON.stringify(entry.sha256)}`);
    }
  }
  return manifest;
}

/// Build the manifest from measured archive facts. The caller supplies bytes and digests it read
/// off the built files; this function only decides the shape and refuses anything malformed.
export function buildReleaseManifest({ version, tag, commit, archives }) {
  if (!plainObject(archives)) refuse("archives must be a JSON object");
  const built = {
    domain: RELEASE_MANIFEST_DOMAIN,
    schema_version: RELEASE_MANIFEST_SCHEMA_VERSION,
    version,
    tag,
    commit,
    // Sorted so two runs over the same archives produce byte-identical manifests, and so an
    // unknown target survives into validation instead of being quietly dropped here.
    archives: Object.fromEntries(
      Object.keys(archives)
        .sort()
        .map((target) => {
          const entry = archives[target];
          if (!plainObject(entry)) return [target, entry];
          return [
            target,
            {
              filename: entry.filename,
              bytes: entry.bytes,
              sha256: typeof entry.sha256 === "string" ? entry.sha256.toLowerCase() : entry.sha256,
            },
          ];
        }),
    ),
  };
  return validateReleaseManifest(built);
}

export function parseReleaseManifest(source) {
  let parsed;
  try {
    parsed = JSON.parse(source);
  } catch (error) {
    refuse(`is not valid JSON: ${error.message}`);
  }
  return validateReleaseManifest(parsed);
}

/// The native lane's only permitted archive-digest assertion.
///
/// `expected` is the release the caller believes it is provisioning. Binding it here is what stops
/// a manifest from a *different* release -- an older one whose archives are genuinely intact --
/// from satisfying the proof for this one. `observed` must carry both the digest and the byte
/// length: length alone is not security, but a manifest that records it and a reader that ignores
/// it is a field nothing checks.
export function requireManifestArchiveDigest(manifest, expected, target, observed) {
  validateReleaseManifest(manifest);
  const expectedVersion = expected?.version;
  const expectedTag = expected?.tag;
  if (typeof expectedVersion !== "string" || !SEMVER.test(expectedVersion)) {
    refuse(`assertion needs the semver release being provisioned, got ${JSON.stringify(expectedVersion)}`);
  }
  if (expectedTag !== `v${expectedVersion}`) {
    refuse(`assertion needs tag v${expectedVersion}, got ${JSON.stringify(expectedTag)}`);
  }
  if (manifest.version !== expectedVersion || manifest.tag !== expectedTag) {
    refuse(
      `describes ${manifest.tag} (${manifest.version}), not the ${expectedTag} (${expectedVersion}) ` +
        `being provisioned`,
    );
  }
  if (!MANIFEST_TARGETS.includes(target)) {
    refuse(`has no ${target} target; known targets are ${MANIFEST_TARGETS.join(", ")}`);
  }
  const entry = manifest.archives[target];
  const observedDigest = observed?.sha256;
  const observedBytes = observed?.bytes;
  if (typeof observedDigest !== "string" || !SHA256.test(observedDigest)) {
    refuse(`assertion needs the provisioned ${target} archive digest, got ${JSON.stringify(observedDigest)}`);
  }
  if (!Number.isSafeInteger(observedBytes) || observedBytes <= 0) {
    refuse(`assertion needs the provisioned ${target} archive byte length, got ${JSON.stringify(observedBytes)}`);
  }
  if (observedDigest !== entry.sha256) {
    refuse(
      `${target} digest ${entry.sha256} does not match the provisioned archive digest ${observedDigest}`,
    );
  }
  if (observedBytes !== entry.bytes) {
    refuse(
      `${target} length ${entry.bytes} does not match the provisioned archive length ${observedBytes}`,
    );
  }
  return entry;
}
