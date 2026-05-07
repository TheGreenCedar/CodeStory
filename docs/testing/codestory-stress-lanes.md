# CodeStory Browser Stress Lanes

These lanes exercise large synthetic browser workloads after the warm stdio loop
metrics exist. They are promotion scouts, not product proof by themselves.

## Run

Default smoke scale builds a 1k-file synthetic repo:

```powershell
cargo bench -p codestory-bench --bench browser_stress
```

Larger scales are opt-in:

```powershell
$env:CODESTORY_STRESS_SCALE = "large" # 1k + 10k
$env:CODESTORY_ALLOW_HEAVY_STRESS = "1"
cargo bench -p codestory-bench --bench browser_stress

$env:CODESTORY_STRESS_SCALE = "full" # 1k + 10k + 100k
$env:CODESTORY_ALLOW_HEAVY_STRESS = "1"
$env:CODESTORY_ALLOW_100K_STRESS = "1"
cargo bench -p codestory-bench --bench browser_stress
```

## Covered Scenarios

| Lane | What It Exercises |
| --- | --- |
| `browser_stress_repo_text_modes` | 1k/10k/100k synthetic file sets with repo-text `auto`, `on`, and `off`. |
| `browser_stress_high_degree_trails` | High-fanout graph nodes with trail depths `2`, `4`, and `6` on the smoke fixture. |
| `browser_stress_browser_service_concurrency_proxy` | Shared browser-service search+trail work at concurrency `1`, `4`, and `16`. |

The concurrency lane is deliberately named as a browser-service proxy. It
exercises the shared read-only work that stdio and HTTP requests use, but it is
not a real stdio or HTTP protocol benchmark. Do not use it as transport
promotion proof without a separate real transport run.

The bench emits `[browser_stress_stats]` JSON lines for validation samples. For
repo-text lanes, those lines include truncation, scanned-file, scanned-byte, and
cap telemetry so large synthetic runs do not hide bounded fallback behavior.

## Promotion Thresholds

Treat these as gates for a candidate change, not as guarantees:

- Smoke scale (`1k`) must pass without panics, unbounded artifacts, or protocol
  pollution.
- Large scale (`10k`, with `CODESTORY_ALLOW_HEAVY_STRESS=1`) must keep
  repo-text `auto/on/off` p95 under 2 seconds on the maintainer workstation.
- The smoke high-degree trail lane must keep depths `2`, `4`, and `6` p95 under
  3 seconds on the maintainer workstation.
- Full scale (`100k`) must complete without out-of-memory failures and must keep
  explicit truncation/cap telemetry visible in benchmark stderr or JSON. It must
  require both `CODESTORY_ALLOW_HEAVY_STRESS=1` and
  `CODESTORY_ALLOW_100K_STRESS=1`.
- Concurrency `16` must not produce storage-lock failures, poisoned runtime
  state, or inconsistent hit/trail counts across repeated browser-service proxy
  samples. Real stdio/HTTP promotion still requires real transport evidence.
- A candidate is not promotion-eligible until at least one real repository run
  is appended with the same commit, machine class, and command shape.

## Reporting Template

Append results here only when they are decision-grade:

| Date | Commit | Scale | Machine | Lane | Result | p50 ms | p95 ms | p99 ms | Notes |
| --- | --- | --- | --- | --- | --- | ---: | ---: | ---: | --- |
| pending | pending | smoke | local | browser_stress | not run | | | | Initial lane definition only. |

## Synthetic Evidence Rules

- Synthetic repos are useful for scale shape, high-fanout behavior, and cap
  regressions.
- Synthetic results are not evidence that CodeStory is ready for arbitrary
  large monorepos.
- Any default or threshold promotion must include at least one real repo run and
  should mention where that run is recorded.
- Keep raw Criterion output out of this file; summarize only the comparison row
  and the decision.
