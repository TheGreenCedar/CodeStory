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
];
const lateStartupReads = [
  'std::env::var(PROJECT_NETWORK_CONFIG_OPT_IN_ENV)',
  'std::env::var_os("CODESTORY_STDIO_CACHE_ROOT")',
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

for (const file of [
  "crates/codestory-cli/src/runtime.rs",
  "crates/codestory-cli/src/stdio_transport.rs",
]) {
  const source = fs.readFileSync(file, "utf8");
  for (const token of lateStartupReads) {
    if (source.includes(token)) violations.push(`${file}: late startup read ${token}`);
  }
}

const runtimeSource = fs.readFileSync("crates/codestory-cli/src/runtime.rs", "utf8");
const runtimeSelection = runtimeSource
  .split("fn new_with_startup", 2)[1]
  ?.split("/// Open project", 1)[0];
for (const token of ["user_cache_root()", "for_project_auto_with_defaults("]) {
  if (runtimeSelection?.includes(token)) {
    violations.push(`crates/codestory-cli/src/runtime.rs: project selection ${token}`);
  }
}

if (violations.length) {
  console.error("Production runtime configuration must remain immutable:\n" + violations.join("\n"));
  process.exit(1);
}
console.log("runtime config boundary ok");
