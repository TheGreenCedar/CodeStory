#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";

function parseArgs(argv) {
  const opts = { dryRun: false };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--dry-run") {
      opts.dryRun = true;
      continue;
    }
    if (arg === "--issue" || arg === "--pr" || arg === "--repo" || arg === "--body-file" || arg === "--body") {
      const value = argv[index + 1];
      if (!value) {
        throw new Error(`${arg} requires a value`);
      }
      opts[arg.slice(2).replace("-", "_")] = value;
      index += 1;
      continue;
    }
    throw new Error(`unknown argument: ${arg}`);
  }
  if (Boolean(opts.issue) === Boolean(opts.pr)) {
    throw new Error("pass exactly one of --issue or --pr");
  }
  if (opts.body && opts.body_file) {
    throw new Error("pass only one of --body or --body-file");
  }
  return opts;
}

function commentBody(opts, stdin = process.stdin) {
  if (opts.body_file) {
    return readFileSync(opts.body_file, "utf8");
  }
  if (opts.body) {
    return opts.body;
  }
  return readFileSync(stdin.fd, "utf8");
}

function validateCommentBody(body) {
  if (!body.trim()) {
    throw new Error("comment body is empty");
  }
  if (body.includes("\\n")) {
    throw new Error("comment body contains literal \\\\n; use real newlines via stdin or --body-file");
  }
}

function ghArgs(opts, bodyFile) {
  const command = opts.issue ? ["issue", "comment", opts.issue] : ["pr", "comment", opts.pr];
  if (opts.repo) {
    command.push("--repo", opts.repo);
  }
  command.push("--body-file", bodyFile);
  return command;
}

function postComment(opts, body) {
  if (opts.dryRun) {
    process.stdout.write(body);
    if (!body.endsWith("\n")) {
      process.stdout.write("\n");
    }
    return;
  }
  const dir = mkdtempSync(path.join(tmpdir(), "codestory-comment-"));
  const bodyFile = path.join(dir, "body.md");
  try {
    writeFileSync(bodyFile, body, "utf8");
    execFileSync("gh", ghArgs(opts, bodyFile), { stdio: "inherit" });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function main(argv = process.argv.slice(2)) {
  const opts = parseArgs(argv);
  const body = commentBody(opts);
  validateCommentBody(body);
  postComment(opts, body);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}

export { ghArgs, parseArgs, validateCommentBody };
