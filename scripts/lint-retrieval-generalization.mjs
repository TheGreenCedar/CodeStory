#!/usr/bin/env node
import { runRetrievalGeneralizationLint } from "./lib/retrieval-generalization-lint.mjs";

const result = runRetrievalGeneralizationLint();
process.stdout.write(result.stdout);
process.stderr.write(result.stderr);
process.exitCode = result.exitCode;
