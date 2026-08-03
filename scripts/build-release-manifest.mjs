#!/usr/bin/env node
// Generate the release manifest from the archives a release actually built.
//
// The frozen source tree cannot carry native archive digests -- the archives are built from that
// tree, so committing their digests into it is circular and would break the freeze and the
// calibration lineage. This runs after the archives exist and before the release is created, so
// the digests describe bytes that are already on disk.
//
//   node scripts/build-release-manifest.mjs --version X.Y.Z --tag vX.Y.Z --commit SHA \
//     --assets DIR --output FILE
//
// Every digest is read off the archive file itself and then cross-checked against the release's
// own SHA256SUMS.txt. Agreement between two independently produced records is the point: a
// checksum file that drifted from the archives it names stops the release here rather than
// shipping a manifest that pins the wrong bytes.

import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { PINNED_ARCHIVE_TARGETS, parsePublishedArchiveDigests } from "./lib/pinned-archive-digests.mjs";
import { buildReleaseManifest } from "./lib/release-manifest.mjs";

export class ReleaseManifestBuildError extends Error {}

function refuse(message) {
  throw new ReleaseManifestBuildError(message);
}

export function parseBuildArguments(argv) {
  const values = new Map();
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (typeof flag !== "string" || !flag.startsWith("--")) {
      refuse(`unexpected argument ${JSON.stringify(flag ?? null)}`);
    }
    if (!["--version", "--tag", "--commit", "--assets", "--output"].includes(flag)) {
      refuse(`unknown argument ${flag}`);
    }
    if (values.has(flag)) refuse(`${flag} was given more than once`);
    if (typeof value !== "string" || value.trim() === "") {
      refuse(`${flag} needs a value`);
    }
    values.set(flag, value);
  }
  for (const flag of ["--version", "--tag", "--commit", "--assets", "--output"]) {
    if (!values.has(flag)) refuse(`${flag} is required`);
  }
  return {
    version: values.get("--version"),
    tag: values.get("--tag"),
    commit: values.get("--commit"),
    assets: values.get("--assets"),
    output: values.get("--output"),
  };
}

/// Hash and measure one archive without holding it in memory. A release archive is hundreds of
/// megabytes, and this runs on the same job that already has the whole asset set on disk.
function measureArchive(archivePath) {
  const hash = createHash("sha256");
  const buffer = Buffer.allocUnsafe(1 << 20);
  const descriptor = fs.openSync(archivePath, "r");
  let bytes = 0;
  try {
    for (;;) {
      const read = fs.readSync(descriptor, buffer, 0, buffer.length, null);
      if (read === 0) break;
      hash.update(buffer.subarray(0, read));
      bytes += read;
    }
  } finally {
    fs.closeSync(descriptor);
  }
  return { bytes, sha256: hash.digest("hex") };
}

/// Measure the built archives and hold them against the release's own checksum file.
export function buildReleaseManifestFromAssets({ version, tag, commit, assets }) {
  const sumsPath = path.join(assets, "SHA256SUMS.txt");
  let sumsText;
  try {
    sumsText = fs.readFileSync(sumsPath, "utf8");
  } catch (error) {
    refuse(`could not read ${sumsPath}: ${error.message}`);
  }
  let published;
  try {
    published = parsePublishedArchiveDigests(sumsText, version);
  } catch (error) {
    refuse(`${sumsPath} does not describe the ${version} archives: ${error.message}`);
  }

  const archives = {};
  for (const [target, assetName] of Object.entries(PINNED_ARCHIVE_TARGETS)) {
    const filename = assetName(version);
    const archivePath = path.join(assets, filename);
    let measured;
    try {
      measured = measureArchive(archivePath);
    } catch (error) {
      refuse(`could not read the built ${target} archive ${archivePath}: ${error.message}`);
    }
    if (measured.sha256 !== published[target]) {
      refuse(
        `${filename} hashes to ${measured.sha256} but SHA256SUMS.txt records ${published[target]}; ` +
          `the built archives and the release checksum file disagree`,
      );
    }
    archives[target] = { filename, bytes: measured.bytes, sha256: measured.sha256 };
  }
  return buildReleaseManifest({ version, tag, commit, archives });
}

export function main(argv) {
  const options = parseBuildArguments(argv);
  const manifest = buildReleaseManifestFromAssets(options);
  fs.writeFileSync(options.output, `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
  return manifest;
}

function invokedDirectly() {
  if (!process.argv[1]) return false;
  try {
    return (
      fs.realpathSync(fileURLToPath(import.meta.url)) === fs.realpathSync(process.argv[1])
    );
  } catch {
    // Fall back to the URL comparison rather than assuming this is a library import: an entry
    // point that decides it is not one exits 0 having proven nothing.
    return import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href;
  }
}

if (invokedDirectly()) {
  try {
    const manifest = main(process.argv.slice(2));
    const targets = Object.entries(manifest.archives)
      .map(([target, entry]) => `${target} ${entry.sha256.slice(0, 12)}… (${entry.bytes} bytes)`)
      .join(", ");
    console.log(`Release manifest for ${manifest.tag} at ${manifest.commit}: ${targets}.`);
  } catch (error) {
    console.error(`::error::${error.message}`);
    process.exit(1);
  }
}
