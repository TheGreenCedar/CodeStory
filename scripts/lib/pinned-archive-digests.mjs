const SHA256 = /^[0-9a-f]{64}$/u;

export const PINNED_ARCHIVE_TARGETS = Object.freeze({
  "macos-arm64": (version) => `codestory-cli-v${version}-macos-arm64.tar.gz`,
  "windows-x64": (version) => `codestory-cli-v${version}-windows-x64.zip`,
  "linux-x64": (version) => `codestory-cli-v${version}-linux-x64.tar.gz`,
});

export function parsePublishedArchiveDigests(source, version) {
  const published = new Map();
  for (const [lineIndex, rawLine] of source.split(/\r?\n/u).entries()) {
    const line = rawLine.trim();
    if (!line) continue;
    const match = /^([0-9a-f]{64})\s+\*?(\S+)$/u.exec(line);
    if (!match) {
      throw new Error(`SHA256SUMS.txt line ${lineIndex + 1} is malformed`);
    }
    const [, digest, filename] = match;
    if (published.has(filename)) {
      throw new Error(`SHA256SUMS.txt repeats ${filename}`);
    }
    published.set(filename, digest);
  }

  return Object.fromEntries(
    Object.entries(PINNED_ARCHIVE_TARGETS).map(([target, assetName]) => {
      const filename = assetName(version);
      const digest = published.get(filename);
      if (!digest) {
        throw new Error(`SHA256SUMS.txt does not contain ${filename}`);
      }
      return [target, digest];
    }),
  );
}

/// The only lane whose pin can lawfully carry archive digests.
export const SOURCE_PIN_LANE = "plugin";

/// Hold a provisioned archive against the digest the SOURCE PIN carries.
///
/// The plugin fast lane pins an ALREADY PUBLISHED CLI, so its `cli-version.json` names archives
/// that exist and `bump-version.mjs` fills them in. The native lane's pin names the release that
/// is about to be built from the very tree holding the pin, so `bump-version.mjs` deletes
/// `archives` there by design. Running this assertion on a native pin is not a stricter check, it
/// is a check that can only fail -- which is why `lane` is required with no default: a caller that
/// forgets to say which lane it is cannot silently fall into the plugin one.
export function requirePinnedArchiveDigest({ pin, target, observedDigest, lane }) {
  if (lane !== SOURCE_PIN_LANE) {
    throw new Error(
      `only the ${SOURCE_PIN_LANE} lane may assert a source-pinned archive digest, not ` +
        `${JSON.stringify(lane)}: a lawful native pin carries none, and the native lane's archive ` +
        `digests are owned by the release manifest`,
    );
  }
  const pinnedDigest = pin?.archives?.[target];
  if (typeof pinnedDigest !== "string" || !SHA256.test(pinnedDigest)) {
    throw new Error(`CLI pin has no valid ${target} archive digest`);
  }
  if (observedDigest !== pinnedDigest) {
    throw new Error(
      `provisioned archive digest ${observedDigest} does not match the pin's ` +
        `${target} digest ${pinnedDigest}`,
    );
  }
  return pinnedDigest;
}
