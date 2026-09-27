#!/usr/bin/env node
import path from "node:path";
import { fileURLToPath } from "node:url";
import { runPacketGeneralizationBoundaryCheck } from "./lib/packet-generalization-boundary.mjs";

const repoRoot = process.argv[2]
  ? path.resolve(process.argv[2])
  : path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const result = runPacketGeneralizationBoundaryCheck(repoRoot);
process.stdout.write(result.stdout);
process.stderr.write(result.stderr);
process.exitCode = result.exitCode;
