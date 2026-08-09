import assert from "node:assert/strict";
import test from "node:test";
import { runEquivalence } from "./policy-equivalence.mjs";

// The standing S1 oracle (#1670): every slice that relocates a workflow-policy rule
// between checker code and release-claims.json must leave the violation multiset of
// validateWorkflows verbatim-identical to the merge base, over the pristine tree and
// over a deterministic hostile corpus that deletes every node and perturbs every
// scalar of every workflow. A mismatch means a rule was strengthened, weakened, or
// reworded rather than relocated.
test("relocated policy rules keep the merge-base verdict multiset over pristine and hostile trees", async () => {
  const result = await runEquivalence({ log: message => console.log(message) });
  assert.deepEqual(
    result.pristine.current,
    result.pristine.base,
    "pristine-tree verdicts diverged from the merge base",
  );
  assert.deepEqual(result.mismatches, []);
  assert.ok(result.corpusSize > 0, "hostile corpus must not be empty");
  console.log(
    `policy-equivalence: base ${result.baseSha}, pristine tree identical, `
    + `${result.corpusSize} hostile mutants (${result.enumeratedMutations} enumerated), `
    + `${Math.round(result.durationMs / 1000)}s`,
  );
});
