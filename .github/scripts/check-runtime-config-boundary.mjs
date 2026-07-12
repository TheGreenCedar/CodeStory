import fs from "node:fs";
import path from "node:path";

const roots = [
  "crates/codestory-cli/src",
  "crates/codestory-retrieval/src",
  "crates/codestory-runtime/src",
];
const forbidden = [
  "std::env::set_var",
  "std::env::remove_var",
  "activate_embed_url",
  "prepare_bundled_llamacpp_client_env_defaults",
  "activate_retrieval_profile_env",
  "CODESTORY_EMBED_LLAMACPP_URL_MANAGED",
];

function rustFiles(root) {
  return fs.readdirSync(root, { withFileTypes: true }).flatMap((entry) => {
    const child = path.join(root, entry.name);
    return entry.isDirectory() ? rustFiles(child) : entry.name.endsWith(".rs") ? [child] : [];
  });
}

const violations = [];
for (const file of roots.flatMap(rustFiles)) {
  const source = fs.readFileSync(file, "utf8");
  const production = source.split(/\r?\n#\[cfg\(test\)\]\r?\nmod tests\s*\{/u, 1)[0];
  for (const token of forbidden) {
    if (production.includes(token)) violations.push(`${file}: ${token}`);
  }
}

if (violations.length) {
  console.error("Production runtime configuration must remain immutable:\n" + violations.join("\n"));
  process.exit(1);
}
console.log("runtime config boundary ok");
