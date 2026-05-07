# Research Sources: Eliminate user and AI friction in CodeStory skill-first repo explanation across Sourcetrail, rootandruntime, and CodeStory.

| Source | Date Checked | Claim Supported | Confidence |
| --- | --- | --- | --- |
| Manual test forensics: `019e03a7-7053-7fe3-ab81-8ff4033cdc81`, `019e03a1-b49f-7ac1-b502-baf6f28cf043`, `019e03a9-9344-7d93-aaec-d2073e2bcdf4` | 2026-05-07 | Seed findings for semantic failure, doctor ambiguity, broad ask drift, search/symbol ambiguity, trail inconsistency, snippet truncation confusion, output friction, and skill recipe gaps. | High |
| `scripts/codestory-manual-friction-check.mjs` | 2026-05-07 | Deterministic harness runs the skill-approved CLI path across `../Sourcetrail`, `../rootandruntime`, and `.` and emits `METRIC quality_gap=<count>`. | High |
