// Which lane the pinned-provision proof is running in, and which archive-digest authority that
// lane is allowed to consult.
//
// The two lanes own different things and the proof used to conflate them:
//
//   plugin  -- the fast lane pins an already-published CLI, so `plugins/codestory/cli-version.json`
//              carries that release's archive digests and the proof must hold the provisioned
//              archive against them.
//   native  -- the pin names the release that is about to be built from the tree holding the pin,
//              so it lawfully has no archive digests. The digests are generated with the archives
//              and live in the release manifest. Running the source-pin assertion here is not a
//              stricter check, it is a check that can only fail -- which is exactly what a frozen
//              native head hit.
//
// This module is the single reader of the source-pin assertion. `prove-plugin-pinned-provision.mjs`
// imports only from here, so no lane can reach `requirePinnedArchiveDigest` without declaring
// itself, and neither lane can silently acquire the other's authority.

import { requirePinnedArchiveDigest, SOURCE_PIN_LANE } from "./pinned-archive-digests.mjs";
import { requireManifestArchiveDigest } from "./release-manifest.mjs";

export const PROVISION_LANES = Object.freeze([SOURCE_PIN_LANE, "native"]);
export const NATIVE_LANE = "native";
// The plugin lane always provisions a published GitHub release. The native lane runs both before
// publication -- against archives staged from the frozen head -- and after it, so it has to say
// which one it means instead of accepting whichever it is handed.
export const NATIVE_BUILD_SOURCES = Object.freeze(["github_release", "explicit_package"]);
export const DEFAULT_TIMEOUT_MS = 600_000;

const NATIVE_ONLY_FLAGS = Object.freeze([
  "--expect-build-source",
  "--release-manifest",
  "--defer-archive-digest",
]);

export class ProvisionProofArgumentError extends Error {}

function refuse(message) {
  throw new ProvisionProofArgumentError(message);
}

/// Parse the proof's command line with no permissive defaults.
///
/// Unknown and repeated flags are refused rather than ignored: a misspelled `--lane natvie` that
/// fell through to the plugin lane would run the source-pin assertion on a native pin, which is
/// the failure this split exists to remove.
export function parseProvisionProofArguments(argv) {
  const seen = new Set();
  const values = new Map();
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (typeof flag !== "string" || !flag.startsWith("--")) {
      refuse(`unexpected argument ${JSON.stringify(flag ?? null)}`);
    }
    if (seen.has(flag)) refuse(`${flag} was given more than once`);
    seen.add(flag);
    if (flag === "--defer-archive-digest") {
      values.set(flag, true);
      continue;
    }
    if (!["--timeout-ms", "--lane", "--expect-build-source", "--release-manifest"].includes(flag)) {
      refuse(`unknown argument ${flag}`);
    }
    // `argv[index + 1]` is undefined at the end of the line, which is how `--timeout-ms` with no
    // value reaches the same refusal as `--timeout-ms garbage`.
    values.set(flag, argv[index + 1]);
    index += 1;
  }

  const rawTimeout = values.has("--timeout-ms") ? Number(values.get("--timeout-ms")) : DEFAULT_TIMEOUT_MS;
  if (!Number.isFinite(rawTimeout) || rawTimeout <= 0) {
    refuse(
      `--timeout-ms needs a positive number of milliseconds, got ` +
        `${JSON.stringify(values.get("--timeout-ms") ?? null)}.`,
    );
  }

  const lane = values.has("--lane") ? values.get("--lane") : SOURCE_PIN_LANE;
  if (!PROVISION_LANES.includes(lane)) {
    refuse(`--lane must be one of ${PROVISION_LANES.join(", ")}, got ${JSON.stringify(lane ?? null)}.`);
  }

  if (lane === SOURCE_PIN_LANE) {
    for (const flag of NATIVE_ONLY_FLAGS) {
      if (values.has(flag)) {
        refuse(
          `${flag} belongs to the ${NATIVE_LANE} lane; the ${SOURCE_PIN_LANE} lane proves the ` +
            `archive digest its own source pin carries.`,
        );
      }
    }
    return {
      lane,
      timeoutMs: rawTimeout,
      expectBuildSource: "github_release",
      releaseManifestPath: null,
      deferArchiveDigest: false,
    };
  }

  const expectBuildSource = values.get("--expect-build-source");
  if (!NATIVE_BUILD_SOURCES.includes(expectBuildSource)) {
    refuse(
      `--lane ${NATIVE_LANE} needs --expect-build-source ${NATIVE_BUILD_SOURCES.join("|")}, got ` +
        `${JSON.stringify(expectBuildSource ?? null)}.`,
    );
  }
  const releaseManifestPath = values.get("--release-manifest");
  const deferArchiveDigest = values.get("--defer-archive-digest") === true;
  if (releaseManifestPath !== undefined && deferArchiveDigest) {
    refuse("--release-manifest and --defer-archive-digest are mutually exclusive.");
  }
  if (releaseManifestPath === undefined && !deferArchiveDigest) {
    // No fallback: the native lane has no source-pinned digest to drop back to, so an unstated
    // intent must stop the proof rather than quietly prove less than the operator thinks.
    refuse(
      `--lane ${NATIVE_LANE} needs either --release-manifest PATH or an explicit ` +
        `--defer-archive-digest; a native pin carries no archive digest to fall back on.`,
    );
  }
  if (releaseManifestPath !== undefined && String(releaseManifestPath).trim() === "") {
    refuse("--release-manifest needs a path to the staged release manifest.");
  }
  return {
    lane,
    timeoutMs: rawTimeout,
    expectBuildSource,
    releaseManifestPath: releaseManifestPath ?? null,
    deferArchiveDigest,
  };
}

/// Hold the provisioned archive against the authority this lane owns, and say which one that was.
///
/// The return value distinguishes a proven digest from a declared deferral so a caller cannot
/// report "digest proven" for a run that proved no such thing.
export function assertProvisionedArchiveDigest({
  lane,
  pin,
  target,
  provisioned,
  releaseManifest = null,
  deferArchiveDigest = false,
}) {
  if (lane === SOURCE_PIN_LANE) {
    if (releaseManifest !== null || deferArchiveDigest) {
      throw new Error(`the ${SOURCE_PIN_LANE} lane proves its source pin and nothing else`);
    }
    const pinnedDigest = requirePinnedArchiveDigest({
      pin,
      target,
      observedDigest: provisioned?.sha256,
      lane,
    });
    return {
      asserted: true,
      authority: "source_pin",
      claim: `archive ${pinnedDigest.slice(0, 12)}… matches the ${target} digest in the source pin`,
    };
  }
  if (lane !== NATIVE_LANE) {
    throw new Error(`unknown provision lane ${JSON.stringify(lane ?? null)}`);
  }
  if (releaseManifest !== null && deferArchiveDigest) {
    throw new Error("a deferred native digest assertion cannot also consume a release manifest");
  }
  if (releaseManifest !== null) {
    const entry = requireManifestArchiveDigest(
      releaseManifest,
      { version: pin?.cli_version, tag: pin?.release_tag },
      target,
      provisioned,
    );
    return {
      asserted: true,
      authority: "release_manifest",
      claim:
        `archive ${entry.sha256.slice(0, 12)}… (${entry.bytes} bytes) matches the ${target} entry ` +
        `in the ${releaseManifest.tag} release manifest`,
    };
  }
  if (!deferArchiveDigest) {
    throw new Error(`the ${NATIVE_LANE} lane needs a release manifest or a declared deferral`);
  }
  return {
    asserted: false,
    authority: "deferred",
    claim:
      `archive digest NOT proven here: the ${target} digest assertion is deferred to the ` +
      `post-release manifest proof, because this native pin lawfully carries none`,
  };
}
