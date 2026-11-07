import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

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

function lexicalPath(file) {
  const absolute = path.resolve(file);
  return process.platform === "win32" ? absolute.toLowerCase() : absolute;
}

function pathObservation(file) {
  try {
    const stat = fs.statSync(file, { bigint: true });
    if (stat.ino === 0n) return { state: "unknown" };
    return { state: "existing", stat, canonical: fs.realpathSync.native(file) };
  } catch (error) {
    if (error.code !== "ENOENT") return { state: "unknown" };
    // A dangling final symlink is not an ordinary missing spelling.
    try {
      fs.lstatSync(file);
      return { state: "unknown" };
    } catch (linkError) {
      return { state: linkError.code === "ENOENT" ? "missing" : "unknown" };
    }
  }
}

function explicitModuleOwnsSource(moduleFile, target) {
  const module = pathObservation(moduleFile);
  const source = pathObservation(target);
  if (module.state === "unknown" || source.state === "unknown") return true;
  if (module.state === "existing" && source.state === "existing") {
    if (!module.stat.isFile() || !source.stat.isFile()) return true;
    if (module.stat.dev === source.stat.dev && module.stat.ino === source.stat.ino) return true;
  } else if (lexicalPath(moduleFile) === lexicalPath(target)) {
    return true;
  }
  // Rust's child namespace derives from the declared path spelling, before
  // resolving an existing directory alias. A file alias's basename is not it.
  const moduleDirectory = path.basename(moduleFile) === "mod.rs"
    ? path.dirname(moduleFile)
    : path.join(path.dirname(moduleFile), path.basename(moduleFile, ".rs"));
  const directory = pathObservation(moduleDirectory);
  if (directory.state === "unknown") return true;
  if (directory.state === "existing" && !directory.stat.isDirectory()) return true;
  const bothExisting = directory.state === "existing" && source.state === "existing";
  const relative = path.relative(
    bothExisting ? directory.canonical : lexicalPath(moduleDirectory),
    bothExisting ? source.canonical : lexicalPath(target),
  );
  return relative === "" || (!path.isAbsolute(relative)
    && relative !== ".." && !relative.startsWith(".." + path.sep));
}

function guardedTestModule(owner, file) {
  if (!fs.existsSync(owner)) return null;
  const source = fs.readFileSync(owner, "utf8");
  const declarations = [
    ...source.matchAll(
      /^(?<attributes>(?:\s*#\s*\[[^\r\n]*\]\s*)*)\s*(?:pub(?:\s*\([^)\r\n]+\))?\s+)?mod\s+tests\s*;/gmu,
    ),
  ].filter((declaration) => {
    const explicitPaths = [
      ...declaration.groups.attributes.matchAll(/#\s*\[\s*path\s*=\s*"([^"\\\r\n]+)"\s*\]/gu),
    ];
    // Unknown or ambiguous attribute syntax keeps the conservative existing
    // guard. Only a resolved explicit destination can exclude an owner.
    const pathAssignments = [...declaration.groups.attributes.matchAll(/\bpath\s*=/gu)];
    if (explicitPaths.length !== 1 || pathAssignments.length !== 1) return true;
    const moduleFile = path.resolve(path.dirname(owner), explicitPaths[0][1]);
    const target = path.resolve(file);
    return explicitModuleOwnsSource(moduleFile, target);
  });
  if (!declarations.length) return null;
  return declarations.every((declaration) =>
    /#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]/u.test(declaration.groups.attributes),
  );
}

function isOutOfLineTestSource(file) {
  const components = file.split(path.sep);
  const sourceIndex = components.lastIndexOf("src");
  if (sourceIndex < 0) return false;
  const sourceRelative = components.slice(sourceIndex + 1);
  const testIndex = sourceRelative.findIndex(
    (component) => component === "tests" || component === "tests.rs",
  );
  if (testIndex < 0) return false;

  const sourceRoot = components.slice(0, sourceIndex + 1).join(path.sep);
  const modulePath = sourceRelative.slice(0, testIndex);
  const owners = modulePath.length
    ? [
        path.join(sourceRoot, ...modulePath) + ".rs",
        path.join(sourceRoot, ...modulePath, "mod.rs"),
      ]
    : fs
        .readdirSync(sourceRoot, { withFileTypes: true })
        .filter((entry) => entry.isFile() && entry.name.endsWith(".rs") && entry.name !== "tests.rs")
        .map((entry) => path.join(sourceRoot, entry.name));
  const guards = owners.map((owner) => guardedTestModule(owner, file)).filter((guard) => guard !== null);
  return guards.length > 0 && guards.every(Boolean);
}

function main() {
  const violations = [];
  for (const file of roots.flatMap(rustFiles)) {
    if (isOutOfLineTestSource(file)) continue;
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
    console.error(
      "Production runtime configuration must remain immutable:\n" + violations.join("\n"),
    );
    process.exit(1);
  }
  console.log("runtime config boundary ok");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}

export { isOutOfLineTestSource };
