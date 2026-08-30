# Proof availability findings

Qualification: `20260822T143747Z-ff5f8b53f864`

## Reproduced measurements

| Measurement | Observed |
| --- | ---: |
| Positive requests | 120 |
| ContractProven cases with exact authoritative evidence | 29 / 120 |
| Exact positive steps | 107 / 312 |
| Negative mutations reaching ContractProven | 0 |
| Exact authoritative receipts | 107 / 113 |
| Unclassified positive steps | 84 |
| Maximum response bytes | 46868 |

### Raw CALL inventory

| Cohort | Stored | Effective endpoints | Exact resolved | Strictly admitted | Unresolved placeholders |
| --- | ---: | ---: | ---: | ---: | ---: |
| `codestory-rust` | 205575 | 205575 | 76544 | 65855 | 129031 |
| `flask-python` | 4534 | 4534 | 888 | 322 | 3646 |
| `gin-go` | 9081 | 9081 | 1915 | 956 | 7166 |
| `vite-ts-js` | 6630 | 6630 | 2049 | 1158 | 4581 |

### Raw edge-distinct trails

| Cohort | Length | Effective endpoints | Exact resolved | Strictly admitted |
| --- | ---: | ---: | ---: | ---: |
| `codestory-rust` | 1 | 205575 | 76544 | 65855 |
| `codestory-rust` | 2 | 434558 | 116410 | 68447 |
| `codestory-rust` | 3 | 885471 | 281354 | 148273 |
| `codestory-rust` | 4 | 2648587 | 1514959 | 1143215 |
| `codestory-rust` | 5 | 22936082 | 20695671 | 18321879 |
| `codestory-rust` | 6 | 363764351 | 356284496 | 319822545 |
| `flask-python` | 1 | 4534 | 888 | 322 |
| `flask-python` | 2 | 2340 | 1020 | 148 |
| `flask-python` | 3 | 2155 | 161 | 67 |
| `flask-python` | 4 | 362 | 44 | 43 |
| `flask-python` | 5 | 149 | 26 | 26 |
| `flask-python` | 6 | 52 | 10 | 10 |
| `gin-go` | 1 | 9081 | 1915 | 956 |
| `gin-go` | 2 | 10107 | 1996 | 617 |
| `gin-go` | 3 | 8744 | 2138 | 274 |
| `gin-go` | 4 | 8275 | 1868 | 197 |
| `gin-go` | 5 | 8112 | 1773 | 101 |
| `gin-go` | 6 | 5114 | 1283 | 63 |
| `vite-ts-js` | 1 | 6630 | 2049 | 1158 |
| `vite-ts-js` | 2 | 10097 | 3129 | 1016 |
| `vite-ts-js` | 3 | 15647 | 5101 | 991 |
| `vite-ts-js` | 4 | 23121 | 7783 | 1173 |
| `vite-ts-js` | 5 | 35924 | 11663 | 1360 |
| `vite-ts-js` | 6 | 53735 | 17343 | 1241 |

### Full proofs by cohort

| Cohort | ContractProven cases |
| --- | ---: |
| `codestory-rust` | 24 |
| `flask-python` | 0 |
| `gin-go` | 4 |
| `vite-ts-js` | 1 |

## Inferences

- The evaluator selected `keep_proof_dark` from these reproduced measurements and the frozen thresholds below.
- 29 of 120 cases satisfy the report contract's evidence-backed full-proof predicate.
- 91 cases do not satisfy that predicate.

## Frozen thresholds

Threshold set: `proof-availability-v1`  
Methodology SHA-256: `28f11893fc1d0c17c1b1b70aeda74818a311009e24b85d899b2d52fa6c8e0dcf`

Hard gates: `false_proofs<=0; exact_receipts=true; certified_absence<=0; complete_funnel=true; complete_provenance=true; invalid<=0; over_cap<=0; transport_errors<=0; maximum_bytes<=65536; each_cohort=true; disposition_match=true`

| Role | Full proofs min | Cohort min | Full Wilson min milli | Cohort Wilson min milli | Step recall min milli | Full/useful min milli | Actionable gap min milli | Unknown p95 max ms | Transport p95 max ms | Complete p95 max bytes | Unknown p95 max bytes | Absolute max bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| automatic | 96 | 21 | 720 | 500 | 900 | 950 | 950 | 500 | 1500 | 32768 | 16384 | 65536 |
| stable_explicit | 60 | 12 | 410 | 240 | 750 | 800 | 900 | 1000 | 2000 | 32768 | 16384 | 65536 |
| experimental | 24 | 12 | 140 | 0 | 500 | 600 | 800 | 2000 | 3000 | 49152 | 24576 | 65536 |

## Decision

Outcome: `keep_proof_dark`  
Automatic thresholds met: `false`

### Failed gates

| Gate | Kind |
| --- | --- |
| `hard.false_contract_proven` | `false_contract_proven` |
| `hard.authoritative_receipt_mismatch` | `receipt_mismatch` |
| `hard.unclassified_positive_steps` | `failure_funnel` |
| `hard.product_disposition_mismatch` | `product_disposition_mismatch` |
| `automatic.full_proofs.count` | `automatic_threshold` |
| `automatic.full_proofs.wilson_lower_milli` | `automatic_threshold` |
| `automatic.cohort.flask-python.count` | `automatic_threshold` |
| `automatic.cohort.flask-python.wilson_lower_milli` | `automatic_threshold` |
| `automatic.cohort.gin-go.count` | `automatic_threshold` |
| `automatic.cohort.gin-go.wilson_lower_milli` | `automatic_threshold` |
| `automatic.cohort.vite-ts-js.count` | `automatic_threshold` |
| `automatic.cohort.vite-ts-js.wilson_lower_milli` | `automatic_threshold` |
| `automatic.cohort.requirement` | `automatic_threshold` |
| `automatic.positive_step_recall_milli` | `automatic_threshold` |
| `automatic.full_or_useful_partial_milli` | `automatic_threshold` |
| `automatic.actionable_incomplete_gap_milli` | `automatic_threshold` |
| `automatic.complete_response_p95_bytes` | `automatic_threshold` |
| `automatic.unknown_response_p95_bytes` | `automatic_threshold` |
| `stable.full_proofs.count` | `stable_threshold` |
| `stable.full_proofs.wilson_lower_milli` | `stable_threshold` |
| `stable.cohort.flask-python.count` | `stable_threshold` |
| `stable.cohort.flask-python.wilson_lower_milli` | `stable_threshold` |
| `stable.cohort.gin-go.count` | `stable_threshold` |
| `stable.cohort.gin-go.wilson_lower_milli` | `stable_threshold` |
| `stable.cohort.vite-ts-js.count` | `stable_threshold` |
| `stable.cohort.vite-ts-js.wilson_lower_milli` | `stable_threshold` |
| `stable.cohort.requirement` | `stable_threshold` |
| `stable.positive_step_recall_milli` | `stable_threshold` |
| `stable.full_or_useful_partial_milli` | `stable_threshold` |
| `stable.complete_response_p95_bytes` | `stable_threshold` |
| `stable.unknown_response_p95_bytes` | `stable_threshold` |
| `experimental.cohort.flask-python.count` | `experimental_usefulness` |
| `experimental.cohort.gin-go.count` | `experimental_usefulness` |
| `experimental.cohort.vite-ts-js.count` | `experimental_usefulness` |
| `experimental.positive_step_recall_milli` | `experimental_usefulness` |
| `experimental.full_or_useful_partial_milli` | `experimental_usefulness` |
| `experimental.unknown_response_p95_bytes` | `experimental_usefulness` |

### Provenance

| Identity | Value |
| --- | --- |
| Source commit | `ff5f8b53f864225244281a7d76382d50589b130e` |
| Source tree | `0926587beb10d475c59453059af57bd56adb2643` |
| Binary SHA-256 | `3c6a89d695bd84639406ee2a2a7c5552dd237448ce18e1de0db8c25d13dfabf6` |
| Corpus SHA-256 | `5a507490554ce4bf9ebe37d380906885feca84bbe07cbb4be5519a1d752ddf31` |
| Thresholds SHA-256 | `c10242b9bd3d288070a50493af890ec9180cab3f16bb0df7762a7f6db5f74bca` |
| Results SHA-256 | `96dfe3814dfc80ab5247cd576a0ad10868fabb7806777591ed49ef13c23e1b51` |

### Nonclaims

- This qualification does not prove runtime execution, temporal order, arbitrary reachability, ownership, data flow, extraction completeness, or subsystem non-participation.
- It is source-built benchmark evidence for the dark exact-call-path kernel. It is not installed-host qualification, public proof availability, or release evidence.

## Recomputed decision observations

| Metric | Raw | Presentation |
| --- | ---: | ---: |
| Full proofs | 29 / 120 | 242 milli |
| Full-proof Wilson 95% | 29 / 120 | lower 0.17385841217535250, upper 0.32550149047431098, floor 173 milli |
| Positive-step recall | 107 / 312 | 343 milli |
| Full or useful partial | 43 / 120 | 358 milli |
| Actionable incomplete gap | 85 / 91 | 934 milli |
| Unknown warm p95 | - | 182 ms |
| Complete response p95 | - | 46218 bytes |
| Unknown response p95 | - | 28302 bytes |
| Maximum response | - | 46868 bytes |

### Cohort Wilson observations

| Cohort | Full proofs | Wilson 95% |
| --- | ---: | ---: |
| `codestory-rust` | 24 / 30 (800 milli) | lower 0.62694303586851752, upper 0.90494892822710127, floor 626 milli |
| `flask-python` | 0 / 30 (0 milli) | lower 0.00000000000000000, upper 0.11351339317396876, floor 0 milli |
| `gin-go` | 4 / 30 (133 milli) | lower 0.05309655484054746, upper 0.29681326682036302, floor 53 milli |
| `vite-ts-js` | 1 / 30 (33 milli) | lower 0.00590859038161246, upper 0.16670390991409173, floor 5 milli |

### Transport p95

| Revision | Nanoseconds |
| --- | ---: |
| `2024-11-05` | 785708 |
| `2025-03-26` | 695584 |
| `2025-06-18` | 711417 |
| `2025-11-25` | 709583 |
