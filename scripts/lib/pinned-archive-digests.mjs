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

export function requirePinnedArchiveDigest(pin, target, observedDigest) {
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
