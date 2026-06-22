import { readFile } from "node:fs/promises";
import { accessSync, constants } from "node:fs";
import { delimiter, dirname, join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const pluginRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const manifest = JSON.parse(
  await readFile(join(pluginRoot, ".codex-plugin", "plugin.json"), "utf8"),
);
const expectedVersion = manifest.version;
const pathDirs = (process.env.PATH ?? "").split(delimiter).filter(Boolean);
const extensions =
  process.platform === "win32"
    ? (process.env.PATHEXT ?? ".COM;.EXE;.BAT;.CMD")
        .split(";")
        .filter(Boolean)
    : [""];

function findOnPath(command) {
  const names = extensions.map((extension) => command + extension.toLowerCase());
  for (const dir of pathDirs) {
    for (const name of names) {
      const candidate = join(dir, name);
      try {
        accessSync(candidate, constants.X_OK);
        return candidate;
      } catch {
        // Keep scanning PATH.
      }
    }
  }
  return null;
}

function fail(message) {
  console.error(message);
  console.error("");
  console.error("Repair:");
  console.error(
    "- Install the CodeStory release asset matching plugin version " +
      expectedVersion +
      " into a stable user bin directory.",
  );
  console.error(
    "- Put that directory before older codestory-cli entries on PATH.",
  );
  console.error(
    "- Stop existing `codestory-cli serve --stdio --refresh none` processes before replacing a locked binary.",
  );
  console.error(
    "- Restart the Codex host/app if PATH changed, then start a fresh thread.",
  );
  process.exit(1);
}

const resolved = findOnPath("codestory-cli");
if (!resolved) {
  fail("codestory-cli was not found on PATH; installed MCP launch would fail.");
}

const isWindowsScript =
  process.platform === "win32" && /\.(bat|cmd)$/iu.test(resolved);
const version = isWindowsScript
  ? spawnSync(
      process.env.ComSpec ?? "cmd.exe",
      ["/d", "/c", resolved, "--version"],
      {
        encoding: "utf8",
      },
    )
  : spawnSync(resolved, ["--version"], {
      encoding: "utf8",
    });
if (version.error || version.status !== 0) {
  fail(
    "Failed to run `" +
      resolved +
      " --version`; installed MCP launch may resolve a broken runtime.",
  );
}

const output = (version.stdout + version.stderr).trim();
const match = output.match(/\b(\d+\.\d+\.\d+)\b/u);
if (!match) {
  fail("Could not parse codestory-cli version from: " + output);
}

const actualVersion = match[1];
if (actualVersion !== expectedVersion) {
  fail(
    "Resolved codestory-cli is " +
      actualVersion +
      ", but plugin package expects " +
      expectedVersion +
      ": " +
      resolved,
  );
}

console.log(
  "codestory-cli runtime OK: " + resolved + " (version " + actualVersion + ")",
);
