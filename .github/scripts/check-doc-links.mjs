#!/usr/bin/env node
/**
 * Validates relative markdown links in docs/**, plugins/codestory/docs/**,
 * plugins/codestory/skills/ (recursive), README.md, and plugins/codestory/README.md.
 * Structure only: file targets and in-repo anchors. Skips http(s), mailto, and absolute paths.
 */
import fs from "node:fs";
import path from "node:path";

const repoRoot = process.cwd();
const violations = [];
const checkedLinks = new Set();

const scopeFiles = [
  path.join(repoRoot, "README.md"),
  path.join(repoRoot, "plugins", "codestory", "README.md"),
  ...collectMarkdownFiles(path.join(repoRoot, "docs")),
  ...collectMarkdownFiles(path.join(repoRoot, "plugins", "codestory", "docs")),
  ...collectMarkdownFiles(path.join(repoRoot, "plugins", "codestory", "skills")),
];

function collectMarkdownFiles(dir) {
  const files = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      files.push(...collectMarkdownFiles(fullPath));
    } else if (entry.isFile() && entry.name.endsWith(".md")) {
      files.push(fullPath);
    }
  }
  return files;
}

function slugifyHeading(text) {
  return text
    .trim()
    .toLowerCase()
    .replace(/[.`]/g, "")
    .replace(/[^\w\s-]/g, "")
    .replace(/\s+/g, "-")
    .replace(/-+/g, "-");
}

function collectAnchors(content) {
  const anchors = new Set();
  const slugCounts = new Map();

  for (const line of content.split(/\r?\n/)) {
    const heading = line.match(/^#{1,6}\s+(.+?)\s*#*\s*$/);
    if (heading) {
      const base = slugifyHeading(heading[1]);
      const seen = slugCounts.get(base) || 0;
      slugCounts.set(base, seen + 1);
      if (seen === 0) {
        anchors.add(base);
      } else {
        anchors.add(`${base}-${seen}`);
      }
    }

    const explicit = line.match(/<a\s+id=["']([^"']+)["']/i);
    if (explicit) {
      anchors.add(explicit[1].toLowerCase());
    }
  }

  return anchors;
}

const anchorCache = new Map();

function anchorsForFile(filePath) {
  if (!anchorCache.has(filePath)) {
    const content = fs.readFileSync(filePath, "utf8");
    anchorCache.set(filePath, collectAnchors(content));
  }
  return anchorCache.get(filePath);
}

function extractLinks(content) {
  const links = [];
  const inline = /!?\[[^\]]*\]\(([^)\s]+(?:\s+"[^"]*")?)\)/gs;
  let match;
  while ((match = inline.exec(content)) !== null) {
    links.push(match[1].trim());
  }
  return links;
}

function shouldSkipTarget(rawTarget) {
  if (!rawTarget || rawTarget.startsWith("#")) {
    return false;
  }
  if (/^[a-z][a-z0-9+.-]*:/i.test(rawTarget)) {
    return true;
  }
  if (rawTarget.startsWith("//")) {
    return true;
  }
  return false;
}

function decodeAnchor(anchor) {
  try {
    return decodeURIComponent(anchor).toLowerCase();
  } catch {
    return anchor.toLowerCase();
  }
}

function checkLink(sourceFile, rawTarget) {
  const cacheKey = `${sourceFile} -> ${rawTarget}`;
  if (checkedLinks.has(cacheKey)) {
    return;
  }
  checkedLinks.add(cacheKey);

  if (shouldSkipTarget(rawTarget)) {
    return;
  }

  const withoutTitle = rawTarget.replace(/\s+"[^"]*"$/, "");
  const hashIndex = withoutTitle.indexOf("#");
  const pathPart = hashIndex === -1 ? withoutTitle : withoutTitle.slice(0, hashIndex);
  const anchorPart =
    hashIndex === -1 ? null : withoutTitle.slice(hashIndex + 1).split("#")[0];

  let targetFile;
  if (pathPart === "") {
    targetFile = sourceFile;
  } else {
    targetFile = path.resolve(path.dirname(sourceFile), pathPart);
  }

  const relSource = path.relative(repoRoot, sourceFile);
  const relTarget = path.relative(repoRoot, targetFile);

  if (!fs.existsSync(targetFile)) {
    violations.push(`${relSource}: missing target ${rawTarget} (resolved ${relTarget})`);
    return;
  }

  if (anchorPart !== null && anchorPart !== "") {
    const anchors = anchorsForFile(targetFile);
    const decoded = decodeAnchor(anchorPart);
    if (!anchors.has(decoded)) {
      violations.push(
        `${relSource}: anchor #${anchorPart} not found in ${relTarget || path.basename(targetFile)}`,
      );
    }
  }
}

for (const file of scopeFiles) {
  const content = fs.readFileSync(file, "utf8");
  for (const link of extractLinks(content)) {
    checkLink(file, link);
  }
}

if (violations.length > 0) {
  console.error("Documentation link check failed:\n");
  for (const violation of violations) {
    console.error(`  - ${violation}`);
  }
  process.exit(1);
}

console.log(
  `Documentation link check passed (${scopeFiles.length} files, ${checkedLinks.size} relative links).`,
);
