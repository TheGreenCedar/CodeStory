import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { chmod, copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));
const generator = path.join(repoRoot, "scripts", "generate-codestory-skill-syntax.mjs");
const catalog = path.join(repoRoot, "plugins", "codestory", "generated-mcp-catalog.json");
const syntax = path.join(repoRoot, "plugins", "codestory", "skills", "codestory-grounding", "references", "generated-cli-syntax.md");

test("catalog generator derives its preferred protocol revision from the server default", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "codestory-catalog-generator-"));
  const fixtureRepo = path.join(root, "repo");
  const fixtureGenerator = path.join(fixtureRepo, "scripts", "generate-codestory-skill-syntax.mjs");
  const fixtureCatalog = path.join(fixtureRepo, "plugins", "codestory", "generated-mcp-catalog.json");
  const fixtureSyntax = path.join(fixtureRepo, "plugins", "codestory", "skills", "codestory-grounding", "references", "generated-cli-syntax.md");
  const cli = path.join(root, "catalog-cli");
  try {
    await mkdir(path.dirname(fixtureGenerator), { recursive: true });
    await mkdir(path.dirname(fixtureCatalog), { recursive: true });
    await mkdir(path.dirname(fixtureSyntax), { recursive: true });
    await copyFile(generator, fixtureGenerator);
    await copyFile(catalog, fixtureCatalog);
    await copyFile(syntax, fixtureSyntax);
    await writeFile(cli, `#!${process.execPath}
const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const args = process.argv.slice(2);
if (!args.includes("serve")) {
  const syntax = fs.readFileSync(process.env.SYNTAX_PATH, "utf8");
  const tick = String.fromCharCode(96);
  const rootUsage = syntax.split("\\n").find((line) => line.startsWith("Root usage: ")).split(tick)[1];
  const commands = syntax.split("\\n").filter((line) => line.startsWith("| " + tick)).map((line) => line.split(tick)).filter((parts) => parts.length === 5).map((parts) => [parts[1], parts[3]]);
  const command = args.find((arg) => arg !== "--help");
  if (!command) {
    process.stdout.write("Usage: " + rootUsage + "\\n\\nCommands:\\n" + commands.map((entry) => "  " + entry[0] + "  fixture").join("\\n") + "\\n");
  } else {
    const usage = commands.find((entry) => entry[0] === command)?.[1];
    process.stdout.write("Usage: " + usage + "\\n");
  }
  process.exit(0);
}
const requests = fs.readFileSync(0, "utf8").trim().split(/\\r?\\n/u).filter(Boolean).map(JSON.parse);
if (requests[0]?.params?.protocolVersion !== undefined) {
  process.stderr.write("catalog generator must select the server default, not offer a revision\\n");
  process.exit(7);
}
const catalog = JSON.parse(fs.readFileSync(process.env.CATALOG_PATH, "utf8"));
for (const request of requests) {
  const result = request.method === "initialize"
    ? { protocolVersion: catalog.wireContract.preferredMcpProtocolVersion, _meta: { codestory_publication: { schema_version: catalog.wireContract.publicationStampSchemaVersion, minimum_compatible_schema_version: catalog.wireContract.minimumCompatiblePublicationStampSchemaVersion }, codestory_protocol: { supported: catalog.wireContract.supportedMcpProtocolVersions, negotiated: catalog.wireContract.preferredMcpProtocolVersion } } }
    : request.method === "tools/list" ? { tools: catalog.tools }
    : request.method === "resources/list" ? { resources: catalog.resources }
    : request.method === "resources/templates/list" ? { resourceTemplates: catalog.resourceTemplates }
    : { prompts: catalog.prompts };
  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + "\\n");
}
`, "utf8");
    await chmod(cli, 0o755);

    assert.doesNotThrow(() => execFileSync(process.execPath, [fixtureGenerator, "--cli", cli], {
      cwd: fixtureRepo,
      encoding: "utf8",
      env: { ...process.env, CATALOG_PATH: fixtureCatalog, SYNTAX_PATH: fixtureSyntax },
      stdio: "pipe",
    }));
    assert.equal(
      await readFile(fixtureCatalog, "utf8"),
      await readFile(catalog, "utf8"),
      "default-selected v2 catalog bytes must stay unchanged",
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
