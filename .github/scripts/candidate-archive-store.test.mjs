import assert from "node:assert/strict";
import {
  linkSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  statSync,
  symlinkSync,
  truncateSync,
  writeFileSync,
} from "node:fs";
import { createHash } from "node:crypto";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  admitCandidateArchive,
  buildCandidateArchiveRecord,
  candidateArchiveStoreKey,
  readCandidateArchiveRecord,
  restoreCandidateArchive,
  validateCandidateArchiveRecord,
  writeCandidateArchiveRecord,
} from "./candidate-archive-store.mjs";

const SCRIPT = fileURLToPath(
  new URL("./candidate-archive-store.mjs", import.meta.url),
);
const REPOSITORY = "TheGreenCedar/CodeStory";
const SHA_A = "a".repeat(40);
const SHA_B = "b".repeat(40);
const TREE_A = "1".repeat(40);
const TREE_B = "2".repeat(40);
const TARGET = "windows-x64";
const ARCHIVE_NAME = "codestory-cli-v0.16.3-windows-x64.zip";

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function descriptor(role, relativePath, bytes) {
  return {
    role,
    relative_path: relativePath,
    bytes: bytes.length,
    sha256: sha256(bytes),
  };
}

function fixtureRecord({
  sourceSha = SHA_A,
  sourceTree = TREE_A,
  archiveBytes = Buffer.from("exact Windows archive bytes"),
} = {}) {
  const archiveDigest = sha256(archiveBytes);
  const companions = [
    descriptor(
      "archive_checksum",
      `${ARCHIVE_NAME}.sha256`,
      Buffer.from(`${archiveDigest}  ${ARCHIVE_NAME}\n`),
    ),
    descriptor(
      "checksum_manifest",
      "SHA256SUMS.txt",
      Buffer.from(`${archiveDigest}  ${ARCHIVE_NAME}\n`),
    ),
  ];
  return buildCandidateArchiveRecord({
    repository: REPOSITORY,
    sourceSha,
    sourceTree,
    target: TARGET,
    archive: {
      name: ARCHIVE_NAME,
      relative_path: ARCHIVE_NAME,
      bytes: archiveBytes.length,
      sha256: archiveDigest,
    },
    companions,
  });
}

function payloads(record, archiveBytes = Buffer.from("exact Windows archive bytes")) {
  const values = new Map([[record.archive.relative_path, archiveBytes]]);
  for (const companion of record.companions) {
    if (companion.role === "archive_checksum" || companion.role === "checksum_manifest") {
      values.set(
        companion.relative_path,
        Buffer.from(`${record.archive.sha256}  ${record.archive.name}\n`),
      );
    }
  }
  return values;
}

function writePayloadTree(root, record, values = payloads(record)) {
  for (const [relative, bytes] of values) {
    const file = path.join(root, ...relative.split("/"));
    mkdirSync(path.dirname(file), { recursive: true });
    writeFileSync(file, bytes, { flag: "wx" });
  }
}

function createFixture(options = {}) {
  const temporaryRoot = realpathSync.native(os.tmpdir());
  const root = mkdtempSync(path.join(temporaryRoot, "codestory-candidate-store-"));
  const inputRoot = path.join(root, "input");
  const outputRoot = path.join(root, "outputs");
  const storeRoot = path.join(root, "store");
  mkdirSync(inputRoot);
  mkdirSync(outputRoot);
  mkdirSync(storeRoot);
  const record = fixtureRecord(options);
  writePayloadTree(inputRoot, record, payloads(record, options.archiveBytes));
  return {
    inputRoot,
    outputRoot,
    record,
    root,
    storeRoot,
  };
}

function cleanup(fixture) {
  rmSync(fixture.root, { force: true, recursive: true });
}

function outputDir(fixture, name = "release-dist") {
  return path.join(fixture.outputRoot, name);
}

function entryDir(fixture, record = fixture.record) {
  return path.join(
    fixture.storeRoot,
    "objects",
    "v1",
    ...candidateArchiveStoreKey(record).split("/"),
  );
}

function storedRecordPath(fixture, record = fixture.record) {
  return path.join(entryDir(fixture, record), "candidate-archive-record.json");
}

function storedPayloadPath(fixture, relativePath, record = fixture.record) {
  return path.join(entryDir(fixture, record), "payload", ...relativePath.split("/"));
}

function clone(value) {
  return JSON.parse(JSON.stringify(value));
}

function admit(fixture, name = "release-dist") {
  return admitCandidateArchive({
    inputRoot: fixture.inputRoot,
    outputDir: outputDir(fixture, name),
    outputRoot: fixture.outputRoot,
    record: fixture.record,
    storeRoot: fixture.storeRoot,
  });
}

function cliArguments(fixture, command, name, { recordFile } = {}) {
  const arguments_ = [
    SCRIPT,
    command,
    "--store-root",
    fixture.storeRoot,
    "--output-root",
    fixture.outputRoot,
    "--output-dir",
    outputDir(fixture, name),
  ];
  if (command === "admit") {
    arguments_.push("--input-root", fixture.inputRoot);
  }
  if (recordFile) {
    arguments_.push("--record", recordFile);
  } else {
    arguments_.push(
      "--repository",
      fixture.record.repository,
      "--source-sha",
      fixture.record.source.commit,
      "--source-tree",
      fixture.record.source.tree,
      "--target",
      fixture.record.target,
      "--archive-name",
      fixture.record.archive.name,
      "--archive-bytes",
      String(fixture.record.archive.bytes),
      "--archive-sha256",
      fixture.record.archive.sha256,
    );
    for (const companion of fixture.record.companions) {
      arguments_.push(
        "--companion",
        [
          companion.role,
          companion.relative_path,
          companion.bytes,
          companion.sha256,
        ].join("|"),
      );
    }
  }
  return arguments_;
}

function writeAuthenticatedRecord(fixture, record = fixture.record, name = "record.json") {
  const file = path.join(fixture.root, name);
  writeFileSync(file, `${JSON.stringify(record, null, 2)}\n`, { flag: "wx" });
  return file;
}

test("the canonical store key contains only source SHA, target, and archive digest", () => {
  const record = fixtureRecord();
  assert.equal(
    candidateArchiveStoreKey(record),
    `${SHA_A}/${TARGET}/${record.archive.sha256}`,
  );
  const nextSource = fixtureRecord({ sourceSha: SHA_B, sourceTree: TREE_B });
  assert.notEqual(candidateArchiveStoreKey(nextSource), candidateArchiveStoreKey(record));
});

test("restore reports an explicit miss without publishing output", () => {
  const fixture = createFixture();
  try {
    const output = outputDir(fixture);
    const result = restoreCandidateArchive({
      outputDir: output,
      outputRoot: fixture.outputRoot,
      record: fixture.record,
      storeRoot: fixture.storeRoot,
    });
    assert.equal(result.hit, false);
    assert.equal(result.key, candidateArchiveStoreKey(fixture.record));
    assert.equal(lstatSync(fixture.storeRoot).isDirectory(), true);
    assert.equal(statExists(output), false);
  } finally {
    cleanup(fixture);
  }
});

test("admission and a later hit materialize the complete exact payload as fresh copies", () => {
  const fixture = createFixture();
  try {
    const admitted = admit(fixture, "first");
    assert.equal(admitted.admitted, true);
    assert.equal(admitted.hit, false);
    assert.equal(
      readFileSync(admitted.archive, "utf8"),
      "exact Windows archive bytes",
    );
    assert.deepEqual(
      Object.keys(admitted.companions).sort(),
      fixture.record.companions.map((entry) => entry.role).sort(),
    );

    const restored = restoreCandidateArchive({
      outputDir: outputDir(fixture, "second"),
      outputRoot: fixture.outputRoot,
      record: fixture.record,
      storeRoot: fixture.storeRoot,
    });
    assert.equal(restored.hit, true);
    assert.equal(statSync(restored.archive).nlink, 1);
    assert.equal(
      statSync(storedPayloadPath(fixture, fixture.record.archive.relative_path)).nlink,
      1,
    );
    assert.notEqual(restored.archive, admitted.archive);
    assert.notEqual(
      restored.archive,
      storedPayloadPath(fixture, fixture.record.archive.relative_path),
    );
    for (const companion of fixture.record.companions) {
      const restoredFile = restored.companions[companion.role];
      assert.equal(statSync(restoredFile).nlink, 1);
      assert.notEqual(restoredFile, storedPayloadPath(fixture, companion.relative_path));
    }
  } finally {
    cleanup(fixture);
  }
});

test("admission rejects sequential publication beneath the final store key", async () => {
  const fixture = createFixture();
  try {
    const source = readFileSync(SCRIPT, "utf8");
    const atomicPublication = "      renameSync(temporary, paths.entry);";
    assert.equal(
      source.split(atomicPublication).length - 1,
      1,
      "atomic store publication must have one mutation target",
    );
    const sequentialPublication = [
      "      mkdirSync(paths.entry, { mode: 0o700 });",
      "      renameSync(temporaryPayload, paths.payload);",
      "      renameSync(path.join(temporary, RECORD_FILE), paths.recordFile);",
      "      rmSync(temporary, { recursive: true });",
    ].join("\n");
    const mutantFile = path.join(fixture.root, "candidate-archive-store-mutant.mjs");
    writeFileSync(
      mutantFile,
      source.replace(atomicPublication, sequentialPublication),
      { flag: "wx" },
    );
    const mutant = await import(pathToFileURL(mutantFile).href);
    assert.throws(
      () => mutant.admitCandidateArchive({
        inputRoot: fixture.inputRoot,
        outputDir: outputDir(fixture),
        outputRoot: fixture.outputRoot,
        record: fixture.record,
        storeRoot: fixture.storeRoot,
      }),
      /not published by atomic directory rename/u,
    );
    assert.equal(statExists(outputDir(fixture)), false);
  } finally {
    cleanup(fixture);
  }
});

test("the public checksum companion pair is mandatory", () => {
  const fixture = createFixture();
  try {
    const result = admit(fixture);
    assert.equal(result.admitted, true);
    assert.deepEqual(
      Object.keys(result.companions).sort(),
      ["archive_checksum", "checksum_manifest"],
    );
  } finally {
    cleanup(fixture);
  }
});

test("an exact archive digest is never reused across source SHAs", () => {
  const fixture = createFixture();
  try {
    admit(fixture, "source-a");
    const sourceB = fixtureRecord({ sourceSha: SHA_B, sourceTree: TREE_B });
    const miss = restoreCandidateArchive({
      outputDir: outputDir(fixture, "source-b-miss"),
      outputRoot: fixture.outputRoot,
      record: sourceB,
      storeRoot: fixture.storeRoot,
    });
    assert.equal(miss.hit, false);
    assert.notEqual(candidateArchiveStoreKey(sourceB), candidateArchiveStoreKey(fixture.record));

    const admittedB = admitCandidateArchive({
      inputRoot: fixture.inputRoot,
      outputDir: outputDir(fixture, "source-b"),
      outputRoot: fixture.outputRoot,
      record: sourceB,
      storeRoot: fixture.storeRoot,
    });
    assert.equal(admittedB.admitted, true);
    assert.equal(statExists(entryDir(fixture, fixture.record)), true);
    assert.equal(statExists(entryDir(fixture, sourceB)), true);
  } finally {
    cleanup(fixture);
  }
});

test("stored record omissions, substitutions, and extra keys never become hits", async (t) => {
  const mutations = [
    ["missing repository", (record) => { delete record.repository; }],
    ["missing source tree", (record) => { delete record.source.tree; }],
    ["missing archive digest", (record) => { delete record.archive.sha256; }],
    ["source substitution", (record) => { record.source.commit = SHA_B; }],
    ["tree substitution", (record) => { record.source.tree = TREE_B; }],
    ["target substitution", (record) => { record.target = "linux-x64"; }],
    ["archive name drift", (record) => { record.archive.name = "wrong.zip"; }],
    ["schema substitution", (record) => { record.schema = "codestory-candidate-archive-store/v2"; }],
    ["extra key", (record) => { record.untrusted = true; }],
  ];
  for (const [name, mutate] of mutations) {
    await t.test(name, () => {
      const fixture = createFixture();
      try {
        admit(fixture, "initial");
        const stored = JSON.parse(readFileSync(storedRecordPath(fixture), "utf8"));
        mutate(stored);
        writeFileSync(storedRecordPath(fixture), `${JSON.stringify(stored)}\n`);
        const originalEntry = entryDir(fixture);
        const result = restoreCandidateArchive({
          outputDir: outputDir(fixture, "mutated"),
          outputRoot: fixture.outputRoot,
          record: fixture.record,
          storeRoot: fixture.storeRoot,
        });
        assert.equal(result.hit, false);
        assert.equal(result.rejectedCorrupt, true);
        assert.match(result.rejection, /candidate archive/u);
        assert.equal(statExists(originalEntry), false);
        assert.equal(statExists(result.rejectedEntry), true);
        assert.equal(statExists(outputDir(fixture, "mutated")), false);
      } finally {
        cleanup(fixture);
      }
    });
  }
});

test("owned malformed and partial store entries are quarantined as authenticated misses", async (t) => {
  await t.test("malformed record", () => {
    const fixture = createFixture();
    try {
      admit(fixture, "initial");
      writeFileSync(storedRecordPath(fixture), "{broken");
      const originalEntry = entryDir(fixture);
      const result = restoreCandidateArchive({
        outputDir: outputDir(fixture, "malformed"),
        outputRoot: fixture.outputRoot,
        record: fixture.record,
        storeRoot: fixture.storeRoot,
      });
      assert.equal(result.hit, false);
      assert.equal(result.rejectedCorrupt, true);
      assert.match(result.rejection, /not valid JSON/u);
      assert.equal(statExists(originalEntry), false);
      assert.equal(statExists(result.rejectedEntry), true);
    } finally {
      cleanup(fixture);
    }
  });

  await t.test("published directory missing its record", () => {
    const fixture = createFixture();
    try {
      const entry = entryDir(fixture);
      mkdirSync(path.join(entry, "payload"), { recursive: true });
      writeFileSync(
        path.join(entry, "payload", ARCHIVE_NAME),
        "partial archive",
      );
      const result = restoreCandidateArchive({
        outputDir: outputDir(fixture, "partial"),
        outputRoot: fixture.outputRoot,
        record: fixture.record,
        storeRoot: fixture.storeRoot,
      });
      assert.equal(result.hit, false);
      assert.equal(result.rejectedCorrupt, true);
      assert.match(result.rejection, /unexpected files/u);
      assert.equal(statExists(entry), false);
      assert.equal(statExists(result.rejectedEntry), true);
      assert.equal(statExists(outputDir(fixture, "partial")), false);
    } finally {
      cleanup(fixture);
    }
  });

  await t.test("orphaned temporary sibling is not a cache hit", () => {
    const fixture = createFixture();
    try {
      const entry = entryDir(fixture);
      mkdirSync(path.dirname(entry), { recursive: true });
      mkdirSync(path.join(path.dirname(entry), `.${path.basename(entry)}.partial-orphan`));
      const result = restoreCandidateArchive({
        outputDir: outputDir(fixture, "orphan"),
        outputRoot: fixture.outputRoot,
        record: fixture.record,
        storeRoot: fixture.storeRoot,
      });
      assert.equal(result.hit, false);
      assert.equal(statExists(outputDir(fixture, "orphan")), false);
    } finally {
      cleanup(fixture);
    }
  });
});

test("every retained payload is rehashed and owned corruption becomes a miss", async (t) => {
  const mutations = [
    ["truncated archive", (fixture) => {
      truncateSync(
        storedPayloadPath(fixture, fixture.record.archive.relative_path),
        5,
      );
    }],
    ["mutated checksum", (fixture) => {
      const companion = fixture.record.companions.find(
        (entry) => entry.role === "checksum_manifest",
      );
      writeFileSync(storedPayloadPath(fixture, companion.relative_path), "wrong\n");
    }],
    ["missing archive checksum", (fixture) => {
      const companion = fixture.record.companions.find(
        (entry) => entry.role === "archive_checksum",
      );
      rmSync(storedPayloadPath(fixture, companion.relative_path));
    }],
    ["extra companion", (fixture) => {
      writeFileSync(
        path.join(entryDir(fixture), "payload", "unlisted.bin"),
        "unlisted",
      );
    }],
  ];
  for (const [name, mutate] of mutations) {
    await t.test(name, () => {
      const fixture = createFixture();
      try {
        admit(fixture, "initial");
        mutate(fixture);
        const originalEntry = entryDir(fixture);
        const result = restoreCandidateArchive({
          outputDir: outputDir(fixture, "mutated"),
          outputRoot: fixture.outputRoot,
          record: fixture.record,
          storeRoot: fixture.storeRoot,
        });
        assert.equal(result.hit, false);
        assert.equal(result.rejectedCorrupt, true);
        assert.match(result.rejection, /candidate archive/u);
        assert.equal(statExists(originalEntry), false);
        assert.equal(statExists(result.rejectedEntry), true);
        assert.equal(statExists(outputDir(fixture, "mutated")), false);
      } finally {
        cleanup(fixture);
      }
    });
  }
});

test("unowned links in a corrupt store entry fail closed without quarantine", async (t) => {
  const mutations = [
    ["hardlinked retained archive", (fixture) => {
      linkSync(
        storedPayloadPath(fixture, fixture.record.archive.relative_path),
        path.join(fixture.root, "retained-archive-link"),
      );
    }],
    ["hardlinked retained record", (fixture) => {
      linkSync(
        storedRecordPath(fixture),
        path.join(fixture.root, "retained-record-link"),
      );
    }],
    ["symlinked retained payload", (fixture) => {
      const archive = storedPayloadPath(
        fixture,
        fixture.record.archive.relative_path,
      );
      rmSync(archive);
      symlinkSync(
        path.join(fixture.root, "outside-archive"),
        archive,
      );
    }],
  ];
  for (const [name, mutate] of mutations) {
    await t.test(name, () => {
      const fixture = createFixture();
      try {
        admit(fixture, "initial");
        writeFileSync(path.join(fixture.root, "outside-archive"), "outside");
        mutate(fixture);
        const originalEntry = entryDir(fixture);
        assert.throws(
          () => restoreCandidateArchive({
            outputDir: outputDir(fixture, "unowned"),
            outputRoot: fixture.outputRoot,
            record: fixture.record,
            storeRoot: fixture.storeRoot,
          }),
          /unowned file|symbolic link or reparse point/u,
        );
        assert.equal(statExists(originalEntry), true);
        assert.equal(statExists(outputDir(fixture, "unowned")), false);
      } finally {
        cleanup(fixture);
      }
    });
  }
});

test("admission rejects missing, mutated, extra, and hardlinked input payloads", async (t) => {
  const mutations = [
    ["missing companion", (fixture) => {
      const companion = fixture.record.companions.find(
        (entry) => entry.role === "checksum_manifest",
      );
      rmSync(path.join(fixture.inputRoot, ...companion.relative_path.split("/")));
    }],
    ["mutated companion", (fixture) => {
      const companion = fixture.record.companions.find(
        (entry) => entry.role === "archive_checksum",
      );
      writeFileSync(
        path.join(fixture.inputRoot, ...companion.relative_path.split("/")),
        "wrong driver",
      );
    }],
    ["extra file", (fixture) => {
      writeFileSync(path.join(fixture.inputRoot, "untrusted.txt"), "extra");
    }],
    ["extra directory", (fixture) => {
      mkdirSync(path.join(fixture.inputRoot, "untrusted"));
    }],
    ["hardlinked companion", (fixture) => {
      const companion = fixture.record.companions.find(
        (entry) => entry.role === "archive_checksum",
      );
      const checksum = path.join(
        fixture.inputRoot,
        ...companion.relative_path.split("/"),
      );
      linkSync(checksum, path.join(fixture.root, "second-checksum-link"));
    }],
  ];
  for (const [name, mutate] of mutations) {
    await t.test(name, () => {
      const fixture = createFixture();
      try {
        mutate(fixture);
        assert.throws(() => admit(fixture), /candidate archive|candidate payload/u);
        assert.equal(statExists(entryDir(fixture)), false);
        assert.equal(statExists(outputDir(fixture)), false);
      } finally {
        cleanup(fixture);
      }
    });
  }
});

test("symlinked input and traversal-shaped record paths are rejected", async (t) => {
  await t.test("symlinked input root", (context) => {
    const fixture = createFixture();
    try {
      const linked = path.join(fixture.root, "linked-input");
      try {
        symlinkSync(fixture.inputRoot, linked, "dir");
      } catch (error) {
        if (["EPERM", "EACCES"].includes(error?.code)) {
          context.skip("host cannot create a directory symlink");
          return;
        }
        throw error;
      }
      assert.throws(
        () => admitCandidateArchive({
          inputRoot: linked,
          outputDir: outputDir(fixture),
          outputRoot: fixture.outputRoot,
          record: fixture.record,
          storeRoot: fixture.storeRoot,
        }),
        /real directory|reparse ancestry/u,
      );
    } finally {
      cleanup(fixture);
    }
  });

  await t.test("symlinked payload", (context) => {
      const fixture = createFixture();
      try {
        const companion = fixture.record.companions.find(
        (entry) => entry.role === "archive_checksum",
        );
      const checksum = path.join(
        fixture.inputRoot,
        ...companion.relative_path.split("/"),
      );
      rmSync(checksum);
      try {
        symlinkSync(path.join(fixture.root, "outside-checksum"), checksum, "file");
      } catch (error) {
        if (["EPERM", "EACCES"].includes(error?.code)) {
          context.skip("host cannot create a file symlink");
          return;
        }
        throw error;
      }
      assert.throws(() => admit(fixture), /symbolic links|reparse points/u);
    } finally {
      cleanup(fixture);
    }
  });

  await t.test("traversal companion", () => {
    const record = clone(fixtureRecord());
    const companion = record.companions.find(
      (entry) => entry.role === "checksum_manifest",
    );
    companion.relative_path = "../SHA256SUMS.txt";
    assert.throws(
      () => validateCandidateArchiveRecord(record),
      /normalized relative POSIX path/u,
    );
  });

  await t.test("unsupported arbitrary companion", () => {
    const record = fixtureRecord();
    assert.throws(
      () => buildCandidateArchiveRecord({
        archive: record.archive,
        companions: [
          ...record.companions,
          descriptor("arbitrary", "arbitrary.bin", Buffer.from("wrong")),
        ],
        repository: record.repository,
        sourceSha: record.source.commit,
        sourceTree: record.source.tree,
        target: record.target,
      }),
      /roles must be unique and supported/u,
    );
  });
});

test("qualification driver payloads cannot enter the public candidate record", () => {
  const record = fixtureRecord();
  assert.throws(
    () => buildCandidateArchiveRecord({
      archive: record.archive,
      companions: [
        ...record.companions,
        descriptor(
          "qualification_driver",
          "qualification-driver/windows-x64/driver.exe",
          Buffer.from("private qualification driver"),
        ),
      ],
      repository: record.repository,
      sourceSha: record.source.commit,
      sourceTree: record.source.tree,
      target: record.target,
    }),
    /roles must be unique and supported/u,
  );
});

test("the per-candidate checksum manifest cannot drift from the archive checksum", () => {
  const record = clone(fixtureRecord());
  const manifest = record.companions.find(
    (entry) => entry.role === "checksum_manifest",
  );
  manifest.sha256 = "f".repeat(64);
  assert.throws(
    () => validateCandidateArchiveRecord(record),
    /same checksum line/u,
  );
});

test("authenticated record files require the exact schema and a real singly linked path", async (t) => {
  await t.test("canonical record", () => {
    const fixture = createFixture();
    try {
      const file = writeAuthenticatedRecord(fixture);
      assert.deepEqual(readCandidateArchiveRecord(file), fixture.record);
    } finally {
      cleanup(fixture);
    }
  });

  await t.test("extra record field", () => {
    const fixture = createFixture();
    try {
      const mutated = clone(fixture.record);
      mutated.untrusted = true;
      const file = writeAuthenticatedRecord(fixture, mutated);
      assert.throws(
        () => readCandidateArchiveRecord(file),
        /record keys changed/u,
      );
    } finally {
      cleanup(fixture);
    }
  });

  await t.test("hardlinked record", () => {
    const fixture = createFixture();
    try {
      const file = writeAuthenticatedRecord(fixture);
      linkSync(file, path.join(fixture.root, "record-link.json"));
      assert.throws(
        () => readCandidateArchiveRecord(file),
        /singly linked/u,
      );
    } finally {
      cleanup(fixture);
    }
  });

  await t.test("symlinked record", (context) => {
    const fixture = createFixture();
    try {
      const file = writeAuthenticatedRecord(fixture);
      const linked = path.join(fixture.root, "linked-record.json");
      try {
        symlinkSync(file, linked, "file");
      } catch (error) {
        if (["EPERM", "EACCES"].includes(error?.code)) {
          context.skip("host cannot create a file symlink");
          return;
        }
        throw error;
      }
      assert.throws(
        () => readCandidateArchiveRecord(linked),
        /symbolic-link or reparse ancestry/u,
      );
    } finally {
      cleanup(fixture);
    }
  });
});

test("record producer writes one canonical public record without overwriting", () => {
  const fixture = createFixture();
  try {
    const directory = path.join(fixture.root, "record-output");
    mkdirSync(directory);
    const file = path.join(directory, "candidate-archive-record.json");
    assert.equal(writeCandidateArchiveRecord(file, fixture.record), file);
    assert.deepEqual(readCandidateArchiveRecord(file), fixture.record);
    assert.throws(
      () => writeCandidateArchiveRecord(file, fixture.record),
      /EEXIST|exist/u,
    );
  } finally {
    cleanup(fixture);
  }
});

test("materialization refuses to overwrite an existing output directory", () => {
  const fixture = createFixture();
  try {
    admit(fixture, "initial");
    const destination = outputDir(fixture, "occupied");
    mkdirSync(destination);
    writeFileSync(path.join(destination, "sentinel"), "keep");
    assert.throws(
      () => restoreCandidateArchive({
        outputDir: destination,
        outputRoot: fixture.outputRoot,
        record: fixture.record,
        storeRoot: fixture.storeRoot,
      }),
      /must not already exist/u,
    );
    assert.equal(readFileSync(path.join(destination, "sentinel"), "utf8"), "keep");
  } finally {
    cleanup(fixture);
  }
});

test("the executable CLI reports a miss without network or input flags", () => {
  const fixture = createFixture();
  try {
    const result = spawnSync(process.execPath, cliArguments(fixture, "restore", "miss"), {
      encoding: "utf8",
      env: {},
    });
    assert.equal(result.status, 0, result.stderr);
    const output = JSON.parse(result.stdout);
    assert.equal(output.hit, false);
    assert.equal(output.archive, null);
    assert.equal(output.key, candidateArchiveStoreKey(fixture.record));
  } finally {
    cleanup(fixture);
  }
});

test("the executable CLI admits and materializes the exact companion set", () => {
  const fixture = createFixture();
  try {
    const recordFile = writeAuthenticatedRecord(fixture);
    const result = spawnSync(
      process.execPath,
      cliArguments(fixture, "admit", "cli-admit", { recordFile }),
      {
        encoding: "utf8",
        env: {},
      },
    );
    assert.equal(result.status, 0, result.stderr);
    const output = JSON.parse(result.stdout);
    assert.equal(output.admitted, true);
    assert.equal(output.hit, false);
    assert.equal(readFileSync(output.archive, "utf8"), "exact Windows archive bytes");
    assert.deepEqual(
      Object.keys(output.companions).sort(),
      fixture.record.companions.map((entry) => entry.role).sort(),
    );
  } finally {
    cleanup(fixture);
  }
});

test("the executable CLI rejects record files combined with substitute fields", () => {
  const fixture = createFixture();
  try {
    const recordFile = writeAuthenticatedRecord(fixture);
    const arguments_ = cliArguments(
      fixture,
      "restore",
      "record-conflict",
      { recordFile },
    );
    arguments_.push("--source-sha", SHA_B);
    const result = spawnSync(process.execPath, arguments_, {
      encoding: "utf8",
      env: {},
    });
    assert.notEqual(result.status, 0);
    assert.match(
      result.stderr,
      /cannot be combined with explicit record fields/u,
    );
    assert.equal(statExists(outputDir(fixture, "record-conflict")), false);
  } finally {
    cleanup(fixture);
  }
});

function statExists(file) {
  try {
    lstatSync(file);
    return true;
  } catch (error) {
    if (error?.code === "ENOENT") return false;
    throw error;
  }
}
