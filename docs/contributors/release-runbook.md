# Release runbook

Use this page to move one accepted CodeStory tree through freeze, promotion,
publication, and closeout. It owns operator sequence and authority boundaries,
not the changing proof matrix.

The [repository release rules](../../AGENTS.md#release-rules) own policy, the
[testing matrix](testing-matrix.md#workflow-and-release-automation) owns proof
lanes, and `release-claims.json` owns the current evidence cells and identity
constraints. If prose and machine policy disagree, stop and reconcile their
owners before continuing.

## Authority boundary

Ordinary work lands through `codex/*` pull requests into
`dev/codestory-next`. That branch cannot publish a release. Do not change a
product version while remediation or support work remains. The version-bump
commit is the accepted calibration source; the sole later source commit writes
the generated constant set and becomes the frozen candidate. Version surfaces
therefore change once, before the final frozen head, rather than on that final
commit.

The final release PR carries that only version-surface change and the generated
constant-set child. A manual `release.yml` dispatch from
`dev/codestory-next` authenticates pre-publish evidence and cannot publish.

Promotion from `dev/codestory-next` to `main` and the resulting publication
require one explicit combined approval from the designated release maintainer.
For v0.17.0, Albert is that maintainer. The approval is recorded immediately
before the main merge; automatic publication has no second human gate. Do not
infer approval from an accepted source review, green CI, a milestone state, or
an earlier release decision.

Never create or push a `v*` tag manually. A qualifying push to `main` starts
automatic release when its version is unpublished; that includes a same-version
retry when the expected tag or release is still absent.

## Release states

Use these identities throughout the release record:

- `C`: final versioned calibration-source commit;
- `F`: direct single-parent child of `C`, changing only the generated constant
  set;
- `P`: published `main` promotion commit, with the same tree as `F` and `F` as
  a direct parent.

```text
integration head
      |
      C  versioned calibration source
      |
      F  constant-set-only frozen candidate
      |\
      | P  tree-preserving main promotion
      |/
 prior main
```

| State | Entry condition | Exit condition |
| --- | --- | --- |
| Integrated | Every release blocker is merged to `dev/codestory-next`; focused checks are green; no source or workflow change remains planned | Record the exact dev commit and tree, reconcile the control plane and proof hosts, and begin the merge moratorium |
| Calibration source | One open same-repository release PR into dev contains the approved version bump and current dev ancestry | Exact-head review, hostile-mutation acceptance, and required native microprobes pass on the bumped head |
| Frozen candidate | Exactly three fresh protected Apple-Silicon Metal calibration runs are assembled, and the direct child of the calibration source changes only `crates/codestory-llama-sys/per-user-embedding-server-constant-set.json` | Reaccept the exact generated head, run the broad source proof once, and complete required package, platform, and qualification evidence |
| Promotion ready | The reviewed release PR can fast-forward dev to the exact frozen candidate | Fast-forward dev to the frozen head without an intervening merge commit, accept the graph-declared pre-publish ledger from that live dev head, record the combined approval, then create the sole tree-preserving dev-to-main promotion commit with the frozen head as a direct parent |
| Published | The automatic main release creates the tag, release, archives, checksums, notes, and closeout summary | The graph-declared post-publish ledger accepts current downloads, installed runtimes, and live behavior |
| Closed | Publication and catalog-delivery state are known | Dev exists and matches main, retained evidence is linked, cleanup is complete, and every release child has its acceptance evidence |

Do not skip states or reconstruct them from memory.

## Before the freeze

Confirm all of the following before starting the version change:

- the release PR contains the current `dev/codestory-next` head;
- the worktree is clean and every commit is pushed;
- all planned source, workflow, policy, documentation, and telemetry changes are
  already integrated;
- the intended release claims and explicit non-claims are recorded;
- required proof hosts are reachable before a long dispatch;
- support PRs, reusable evidence, and already-invalidated evidence are listed;
- no queued or running proof belongs to a superseded candidate.

Record the exact commit and tree with:

```sh
git rev-parse HEAD 'HEAD^{tree}'
```

Apply the release version only through:

```sh
node scripts/bump-version.mjs --version <version>
python .github/scripts/check-codestory-release.py --version <version>
node .github/scripts/check-workflow-policy.mjs
```

Do not edit version surfaces by hand.

## Freeze and qualification

Run the calibration-source acceptance on the bumped release-PR head before
calibration. It receives focused hostile mutations and native microprobes, not
the broad workspace source proof. The canonical dispatch uses
`source-proof.yml` with `acceptance_only=true` and
`acceptance_phase=calibration_source`, bound to exact commit `C`.

Collect exactly three fresh protected Apple-Silicon Metal calibration runs with
CPU fallback disabled. Assemble their authenticated bundle, then commit only
the generated per-user embedding-server constant-set file as the direct child
of the accepted calibration source.

Treat that generated head as a new candidate. Record its commit, tree, and
freeze receipt. Re-run exact-head acceptance with
`acceptance_phase=frozen_candidate`, bound to exact commit `F`, then run the
full workspace source proof exactly once on this unchanged frozen candidate.
Run package, protected hardware, installed-candidate, and qualification lanes
from artifacts built from that same head. Qualification is a program gate; the
live release workflow does not yet authenticate an earlier qualification run,
so record its complete run identity in the release-driver receipt rather than
implying the workflow already carries it:

```sh
node .github/scripts/release-driver-receipt.mjs record qualification \
  --receipt release-driver-receipt.json --data-file qualification.json
```

Use the testing matrix and workflow inputs as the command authority. Do not copy
a target checklist from this page; `release-claims.json` declares the current
inventory.

## Evidence handoff

Keep one release record. Initialize it once, record each field group from a JSON
file or inline JSON, and inspect it without reconstructing state from workflow
pages:

```sh
node .github/scripts/release-driver-receipt.mjs init \
  --version 0.17.0 --receipt release-driver-receipt.json
node .github/scripts/release-driver-receipt.mjs record <field-group> \
  --receipt release-driver-receipt.json --data-file <field-group>.json
node .github/scripts/release-driver-receipt.mjs show \
  --receipt release-driver-receipt.json
```

The field-group names are `calibration-source`, `pull-requests`,
`calibration-source-acceptance`, `calibration`, `frozen-candidate`,
`frozen-candidate-acceptance`, `source-proof`, `package`, `hardware`,
`installed-candidate`, `qualification`, `pre-publish-ledger`, `evidence`,
`promotion`, `publication`, `native-release-manifest`,
`post-publish-ledger`, `catalog-delivery`, and `next-action`. Run evidence uses
`lane`, `run_id`, `attempt`, immutable `artifact`, `digest`, `identity`,
`commit`, `tree`, and `conclusion`. Calibration records exactly three such
rows. For v0.17.0, #1179 owns this operator handoff.

The record must carry:

- release version;
- calibration-source commit and tree;
- frozen-candidate commit and tree;
- release PR and integrated support PRs;
- calibration-source acceptance run, attempt, artifact, and digest for `C`;
- frozen-candidate acceptance run, attempt, artifact, and digest for `F`;
- calibration run IDs, artifact name, and digest;
- source-proof run, attempt, artifact, and digest;
- package, hardware, installed-candidate, and qualification run identities;
- accepted pre-publish ledger artifact and digest;
- reusable and invalidated evidence with reasons;
- promotion PR and explicit approver with time;
- published commit, tree, tag, release run, and release URL;
- native release manifest identity once W8.11 has landed and its machine policy
  declares the artifact;
- accepted post-publish ledger artifact and digest;
- catalog-delivery state and installer identity;
- next action or recovery owner.

A run ID without its attempt, immutable artifact name, and digest is not an
evidence handoff. A failed newer attempt cannot fall back to an older successful
attempt. Reuse is valid only when the machine policy accepts the exact commit,
tree, artifact, identity, and evidence window.

Validate the record before each transition. Validation is cumulative, requires
exactly the groups available at that phase, and rejects later-phase groups when
checking an earlier phase:

```sh
node .github/scripts/release-driver-receipt.mjs validate \
  --phase <pre-freeze|frozen|published|closeout> \
  --receipt release-driver-receipt.json
```

## Invalidation

Any unplanned commit after calibration-source acceptance invalidates that
acceptance. The generated constant-set commit is the sole planned transition.

Any commit after frozen-candidate acceptance invalidates the freeze, including
documentation or workflow changes. A changed PR head, advanced dev base,
identity mismatch, expired evidence, failed newer attempt, proof-host identity
change, or conflict-bearing promotion also invalidates the affected evidence.

When invalidated:

1. Cancel queued and running broad, package, hardware, qualification, and release
   work for the old head.
2. Mark the receipt and affected artifacts invalid with the reason and replacing
   SHA.
3. Return to the earliest state whose entry condition remains true.
4. Recalibrate when any path other than the generated constant-set file differs
   from the accepted calibration source.

Record the invalidation before replacing evidence. Repeat `--changed-path` for
every changed path; if the changed-path list is omitted, the tool conservatively
requires recalibration. Use `--event evidence --group <field-group>` for an
identity, expiry, attempt, host, or promotion invalidation that is not a source
commit:

```sh
node .github/scripts/release-driver-receipt.mjs invalidate \
  --event <post-calibration-commit|post-freeze-commit|evidence> \
  --receipt release-driver-receipt.json \
  --reason <reason> --replacing-sha <sha> --changed-path <path>
```

Invalidated groups remain unusable until `record` replaces each affected group.

Do not reorder history or widen the allowed calibration path to preserve old
proof.

## Non-blocking program outcomes

These outcomes do not block v0.17.0 when their owning contract records them
honestly:

- if no vector candidate qualifies, retain the incumbent, keep the degradation
  counter, and record the performance non-claim;
- once W8.11 has landed and machine policy declares the release manifest, if
  the Ed25519 key ceremony is incomplete, ship it as checksum data over TLS and
  state that authentication is not armed; once signature verification is
  armed, there is no unsigned fallback;
- a second-maintainer rehearsal is a tracked ownership goal, not a publication
  gate;
- optional performance or answer-quality evidence is not a standard release
  gate unless the selected public claim requires it.

These are explicit containments or non-claims, not proof that the underlying
risk was eliminated.

## Failure recovery

After a platform-specific package, link, filesystem, cache, or identity failure,
run a native probe that reproduces the failing seam in under 90 seconds before
requesting another full build.

After two equivalent failures, stop. Record the evidence, change the approach,
and only then retry.

Catalog publication happens after the irreversible release and is not a release
gate. Before publication, W8.4 runs the real Codex marketplace resolver against
the candidate-pinned fixture; the live catalog is allowed to keep naming the
previous release until the release exists.

After the catalog push, the release probes the exact live catalog revision
before it may record `published`. If that probe fails, the marketplace-publish
credential automatically restores the recorded previous plugin SHA and version.
The restore is fenced by the just-pushed catalog revision, SHA, and version, so
it is idempotent and refuses to overwrite a catalog that moved independently.
A successful restore records `restored` and
`codex_marketplace_restored_fixture`; downstream installed-runtime proof uses a
candidate-pinned fixture and cannot retain the provisional `published` state.
If restore does not complete, delivery is `unresolved` and closeout rejects the
catalog claim, but the already-published release remains standing.

Record `published`, `deferred`, or `restored` from the authenticated
installed-runtime cells. Recover a catalog whose original push was deferred
with `marketplace-sync.yml`; that manual move records the distinct `recovered`
event. Do not hand-edit the marketplace repository. Missing marketplace
credentials require repository settings work before either automatic restore
or a sync retry can succeed.

## Promotion and closeout

Before promotion, prove that the frozen candidate remains an ancestor, that the
promotion preserves its tree, and that the frozen constant-set commit is a
direct parent of the published head. Tree equality and ancestry alone are not
enough: the calibration-lineage checker permits exactly one tree-preserving
promotion commit after the frozen head.

Do not use the normal merge-commit button for both the release PR into dev and
the later dev-to-main PR. Two merge commits break the direct-parent lineage.
After the release PR is reviewed and its exact head is accepted, fast-forward
`dev/codestory-next` to that frozen head. The dev-to-main merge is then the one
allowed promotion commit. A conflict-bearing merge voids the candidate.

Record the exact relationships as `F^ == C`, `F` as a direct parent of `P`, and
`tree(F) == tree(P)`. Run the calibration-lineage owner with its documented
promotion allowance; do not replace that executable check with this prose.

Pause here for the designated maintainer's explicit combined promotion and
publication approval. Merging the approved dev-to-main PR crosses the release
boundary because the unpublished release version on `main` starts the automatic
release. There is no later environment approval to catch an accidental merge,
and repository settings currently do not enforce this human gate. Recheck the
live settings and record the approval rather than treating the pause as
automatic.

After publication:

- verify the tag, GitHub release, archives, checksums, notes, and closeout
  summary; verify the release manifest only after W8.11 has landed and the
  machine policy declares it;
- run the graph-declared post-publish proof against downloaded bytes and fresh
  installed runtimes;
- record the catalog-delivery state without upgrading `deferred` or `restored`
  to `published`;
- fast-forward `dev/codestory-next` from `F` to the published promotion `P`, or
  recreate the branch at `P` if it was deleted; this is a tree-preserving
  branch reconciliation, not a new commit;
- verify that dev exists and matches main:

```sh
git ls-remote --heads origin main dev/codestory-next
git rev-list --left-right --count origin/main...origin/dev/codestory-next
```

Reconcile release-created branches, worktrees, runners, artifacts, and control
plane entries. Close the release epic only when every child acceptance
criterion and required live proof is present.
