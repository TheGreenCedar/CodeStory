# Segment 5 Metric Report

Segment 5 is a measurement-contract segment, not another CodeStory product
iteration.

The old dashboard focus, `quality_gap`, is still useful as an open-checklist
state. It is not a throughput signal. Once the accepted checklist reaches zero
open items, a flat zero can mean either "nothing changed" or "new gaps were
accepted and closed before the packet was logged."

That is what happened in the current run. Packets 18-23 changed the product and
verification surface while `quality_gap` stayed at zero. The actual progress
signal was cumulative accepted gaps closed:

| Runs | Old open-gap metric | Accepted / closed movement |
| --- | --- | --- |
| 18 | `quality_gap=0` | `quality_total=24`, `quality_closed=24` |
| 19 | `quality_gap=0` | `quality_total=25`, `quality_closed=25` |
| 20 | `quality_gap=0` | `quality_total=26`, `quality_closed=26` |
| 21 | `quality_gap=0` | `quality_total=27`, `quality_closed=27` |
| 22 | `quality_gap=0` | `quality_total=28`, `quality_closed=28` |
| 23 | `quality_gap=0` | `quality_total=29`, `quality_closed=29` |

Segment 5 run 24 switches the primary chart to `quality_closed` and records the
current state as a measurement-only baseline: `quality_closed=29`,
`quality_total=29`, `quality_gap=0`, `quality_newly_accepted=0`, and
`quality_stagnating=1`.

Run 25 then logs a fresh accepted gap before fixing it:
`quality_closed=29`, `quality_total=30`, `quality_gap=1`,
`quality_newly_accepted=1`, and `quality_stagnating=0`.

Run 26 closes that accepted gap after the suite bridge-label fix:
`quality_closed=30`, `quality_total=30`, `quality_gap=0`,
`quality_newly_closed=1`, and `quality_stagnating=0`. This is the dashboard
signal to watch: the open-gap line returns to zero, but the accepted/closed
count advances.

The same rule applies to the current source-verification handoff packet. Run 33
accepted four fresh gaps before fixing them, so the open-gap metric moved to
`quality_gap=4` with `quality_newly_accepted=4`. Packet 34 then closes those
accepted gaps; the useful visual movement is cumulative accepted gaps closed
advancing from 37 to 41, not a flat return to zero open gaps.

Fresh Round 18 followed the same discipline. Run 35 logged two new accepted
gaps open before implementation: isolated `--cache-dir` semantic setup still
fell back to `MissingEmbeddingRuntime`, and the newest handoff/report behavior
lacked deterministic regression coverage. Packet 36 then closed both:
`quality_closed=43`, `quality_total=43`, `quality_gap=0`,
`quality_newly_closed=2`, `quality_stagnating=0`, and
`quality_plateau=0`. The dashboard is now on Segment 9 with
`quality_closed` as the primary metric so this movement appears as progress
rather than as another flat zero.

Fresh Round 19 used the same guard instead of treating flat zero as permission
to keep polishing. Run 37 logged two fresh accepted gaps open before fixing
them: no-path bridge graphs could say `truncated=true` even with zero edges and
zero omitted edges, and repo-configured `.codestory.toml cache_dir` behavior
needed deterministic managed-asset-root coverage. Packet 38 then closed those
accepted gaps: `quality_closed=45`, `quality_total=45`, `quality_gap=0`,
`quality_newly_closed=2`, `quality_stagnating=0`, and `quality_plateau=0`.

Run 39 is intentionally a measurement-only plateau marker, not another product
improvement packet: `quality_closed=45`, `quality_total=45`, `quality_gap=0`,
`quality_newly_accepted=0`, `quality_newly_closed=0`,
`quality_stagnating=1`, and `quality_plateau=1`. This is the expected signal
when the loop has no fresh open candidate left. The correct interpretation is
"stop or scout," not "zero open gaps means continued iteration is useful."

Going forward, the visual focus should be:

- Primary: `quality_closed` (higher is better).
- Secondary: `quality_gap`, `quality_total`, `quality_newly_accepted`,
  `quality_newly_closed`, `quality_stagnating`, and `quality_plateau`.
- Stop rule: if `quality_gap=0` and no fresh candidate is newly accepted, stop
  product iteration unless the next packet is a promotion/holdout gate or a
  fresh candidate is logged open before it is fixed.
