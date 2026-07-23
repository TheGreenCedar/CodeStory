# Documentation maintenance

Keep one owner for each kind of truth. Update the owning page and link to it
instead of copying the same commands, status fields, or first-use story into
every host guide.

## Content owners

| Content | Canonical home | Rule |
| --- | --- | --- |
| Product overview and entry path | Root `README.md` | Short enough for a new reader; link deeper detail |
| Shared install and first-use behavior | `docs/users/README.md` | Host-neutral and task-first |
| Host setup | `docs/users/codex.md`, `cursor.md`, `claude-code.md`, `copilot.md` | Only host-specific install, update, and limitations |
| Prompt examples | `docs/users/prompt-patterns.md` | Do not repeat prompt catalogs in host pages |
| Evidence semantics | `docs/users/trust-and-readiness.md` | Plain language; link wire fields to the skill contract |
| Recovery | `docs/users/troubleshooting.md` | Symptom to action; no architecture tutorial |
| CLI flags | Generated `codestory-cli --help` | Docs group workflows and trust boundaries, not every option |
| Architecture | `docs/architecture/` | Components, ownership, data flow, invariants, failure boundaries |
| Contributor workflow | `docs/contributors/` | Current branch/worktree, owning crate, smallest proof |
| Test and release claims | `docs/contributors/testing-matrix.md` and `docs/testing/` | Exact commands, proof tiers, evidence records |
| Maintainer operations | `docs/ops/` | Diagnostics and bounded recovery; link user-facing recovery back to troubleshooting |
| Agent behavior | `plugins/codestory/skills/codestory-grounding/` | MCP intent, retry, evidence, and failure contracts |

## Review checklist

- Start with the reader's task or the system relationship they need to
  understand.
- State what success looks like and what the evidence does not prove.
- Use current product language: one executable package, one automatically
  managed per-user embedding server, automatic preparation, and
  project-scoped requests.
- Do not revive retired external helper, user-selected endpoint, port, PID,
  repair-worker, Docker-runtime, or consent flows in current guidance.
- Distinguish plugin CLI package download from runtime behavior: the plugin may
  install a signed CLI, while the installed CLI contains its model and backend.
- Keep repository examples portable. CodeStory-internal paths belong only in
  contributor or architecture docs.
- Link generated help for unstable option lists.
- Use one small diagram when a lifecycle, ownership boundary, or multi-step
  publication is harder to understand in prose.
- Preserve historical changelog wording for old releases; update only current
  release notes and active guidance.

## Verification

For docs-only changes:

```sh
node .github/scripts/check-doc-links.mjs
git diff --check
```

Read every changed page after editing. The link checker proves relative links
and anchors, not prose accuracy.

When plugin package or skill files change, also run:

```sh
node --test plugins/codestory/tests/plugin-static.test.mjs
```

Do not add tests that assert prose. Escalate to code, package, platform, or
release proofs only when the documentation change accompanies behavior in that
lane; use the [testing matrix](testing-matrix.md).

## New pages

Prefer extending a current owner over adding a page. When a new page is
necessary, use the relevant template under `docs/templates/`, add it to the
appropriate routing page, and make clear which existing topic it now owns.
