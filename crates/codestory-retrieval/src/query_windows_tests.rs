//! UNEXECUTED source-only draft; include as a sibling test module.
//! Integration seam: cfg(test) per_user_embedding::test_engine_identity() must
//! reexport the existing fixture. No expected windows use the splitter helper.

use crate::per_user_embedding::{PerUserEmbeddingError, test_engine_identity as identity};
use crate::query_windows::{QueryWindowBatch, embed_windows};
use std::cell::Cell;
use std::time::{Duration, Instant};

const TOO_LONG: &str = "native_embedding_input_too_long";
const RPC_LIMIT: usize = 128;
const LEAF_LIMIT: usize = 64;

fn native_error(code: &str) -> anyhow::Error {
    PerUserEmbeddingError {
        code: code.to_owned(),
        message: "independent callback oracle".to_owned(),
        retry_class: "terminal".to_owned(),
        retry_after_ms: 0,
        retry_condition: String::new(),
        capacity: None,
    }
    .into()
}

#[derive(Debug)]
struct Call {
    inputs: Vec<String>,
    timeout: Duration,
    replay: bool,
    accepted: bool,
}

fn vector(text: &str) -> Vec<f32> {
    vec![
        text.len() as f32,
        text.bytes().map(u32::from).sum::<u32>() as f32,
        text.bytes().enumerate().fold(0_u32, |acc, (i, b)| {
            acc.wrapping_add((i as u32 + 1) * b as u32)
        }) as f32,
    ]
}

fn vectors(texts: &[String]) -> Vec<Vec<f32>> {
    texts.iter().map(|text| vector(text)).collect()
}

fn assert_physical_limits(calls: &[Call]) {
    assert!(calls.len() <= RPC_LIMIT, "RPC work ceiling exceeded");
    for call in calls {
        assert!(call.inputs.len() <= 8, "physical batch ceiling exceeded");
        assert!(call.inputs.iter().map(String::len).sum::<usize>() <= 1 << 20);
    }
}

fn assert_exact_cover(batch: &QueryWindowBatch, originals: &[String], calls: &[Call]) {
    assert_eq!(batch.groups.len(), originals.len());
    let mut expected = Vec::<(String, Vec<f32>)>::new();
    let mut leaves = 0;
    for (group, original) in batch.groups.iter().zip(originals) {
        assert!(!group.is_empty());
        let mut cursor = 0;
        let mut reconstructed = Vec::new();
        for window in group {
            leaves += 1;
            assert_eq!(
                window.range.start, cursor,
                "gap, overlap, or reordered range"
            );
            assert!(window.range.end > cursor);
            assert!(window.range.end <= original.len());
            assert!(original.is_char_boundary(window.range.start));
            assert!(original.is_char_boundary(window.range.end));
            let raw = &original[window.range.clone()];
            assert_eq!(window.vector, vector(raw));
            reconstructed.extend_from_slice(raw.as_bytes());
            expected.push((raw.to_owned(), window.vector.clone()));
            cursor = window.range.end;
        }
        assert_eq!(cursor, original.len(), "tail omitted");
        assert_eq!(reconstructed, original.as_bytes(), "raw bytes changed");
    }
    assert!(leaves <= LEAF_LIMIT);
    let mut accepted: Vec<_> = calls
        .iter()
        .filter(|call| call.accepted)
        .flat_map(|call| call.inputs.iter().map(|text| (text.clone(), vector(text))))
        .collect();
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    accepted.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        expected, accepted,
        "vector loss, duplication, or rejected input reuse"
    );
    assert_physical_limits(calls);
}

#[test]
fn mixed_queries_preserve_unicode_whitespace_and_final_tail() {
    let originals = vec![
        "unchanged short query".to_owned(),
        format!(
            "  \t{}\n\n{}\r\nFINAL-TAIL-猫  ",
            "café 猫🙂 \t".repeat(50),
            "βeta  ".repeat(50)
        ),
        "\tanother short\n".to_owned(),
        "猫".repeat(100),
    ];
    let started = Instant::now();
    let timeout = Duration::from_secs(3);
    let mut calls = Vec::new();
    let batch = embed_windows(
        &originals,
        started,
        timeout,
        &|| false,
        |texts, budget, replay| {
            let accepted = texts.iter().all(|text| text.len() <= 128);
            calls.push(Call {
                inputs: texts.to_vec(),
                timeout: budget,
                replay,
                accepted,
            });
            if !accepted {
                return Err(native_error(TOO_LONG));
            }
            Ok((vectors(texts), identity()))
        },
    )
    .expect("a whitespace-delimited oversized query should recover");
    assert_eq!(calls[0].inputs, originals, "initial raw request changed");
    assert!(calls[0].replay);
    assert!(calls.iter().skip(1).all(|call| !call.replay));
    assert!(batch.expanded);
    assert_eq!(batch.deadline, started + timeout);
    assert_eq!(batch.identity, identity());
    assert_exact_cover(&batch, &originals, &calls);
    for i in [0, 2] {
        assert_eq!(
            batch.groups[i].len(),
            1,
            "short query unnecessarily subdivided"
        );
        assert_eq!(batch.groups[i][0].range, 0..originals[i].len());
    }
    assert!(batch.groups[1].len() > 1);
    assert!(batch.groups[3].len() > 1);
}

#[test]
fn short_query_remains_one_rpc_and_identical_vector() {
    let originals = vec![" short café\tquery \n".to_owned()];
    let started = Instant::now();
    let timeout = Duration::from_secs(2);
    let sentinel = vec![91.0, -7.0, 0.25];
    let mut calls = 0;
    let batch = embed_windows(
        &originals,
        started,
        timeout,
        &|| false,
        |texts, budget, replay| {
            calls += 1;
            assert_eq!(texts, originals.as_slice());
            assert!(replay);
            assert!(budget <= timeout);
            Ok((vec![sentinel.clone()], identity()))
        },
    )
    .unwrap();
    assert_eq!(calls, 1);
    assert!(!batch.expanded);
    assert_eq!(batch.groups.len(), 1);
    assert_eq!(batch.groups[0].len(), 1);
    assert_eq!(batch.groups[0][0].range, 0..originals[0].len());
    assert_eq!(batch.groups[0][0].vector, sentinel);
    assert_eq!(batch.deadline, started + timeout);
}

#[test]
fn only_the_typed_native_too_long_error_allows_subdivision() {
    let originals = vec!["word ".repeat(100)];
    for typed in [false, true] {
        let mut calls = 0;
        let result = embed_windows(
            &originals,
            Instant::now(),
            Duration::from_secs(2),
            &|| false,
            |_, _, replay| {
                calls += 1;
                assert!(replay);
                Err(if typed {
                    native_error("native_embedding_other_failure")
                } else {
                    anyhow::anyhow!(TOO_LONG)
                })
            },
        );
        assert!(result.is_err());
        assert_eq!(calls, 1, "unrelated error was retried/subdivided");
    }
}

#[test]
fn accepted_vector_cardinality_mismatch_aborts() {
    let originals = vec!["first".to_owned(), "second".to_owned()];
    for count in [0, 1, 3] {
        let result = embed_windows(
            &originals,
            Instant::now(),
            Duration::from_secs(2),
            &|| false,
            |_, _, _| Ok((vec![vec![1.0]; count], identity())),
        );
        assert!(result.is_err(), "wrong number of vectors accepted: {count}");
    }
}

#[test]
fn engine_drift_aborts_but_execution_counter_growth_is_allowed() {
    let originals = vec!["word ".repeat(180)];
    for drift in 0..4 {
        let mut accepted = 0;
        let mut calls = 0;
        let result = embed_windows(
            &originals,
            Instant::now(),
            Duration::from_secs(3),
            &|| false,
            |texts, _, _| {
                calls += 1;
                assert!(calls <= RPC_LIMIT);
                if texts.iter().any(|text| text.len() > 64) {
                    return Err(native_error(TOO_LONG));
                }
                accepted += 1;
                let mut id = identity();
                if accepted > 1 {
                    match drift {
                        0 => id.server_instance_id.push_str("_changed"),
                        1 => id.load_generation += 1,
                        2 => id.model_load_count += 1,
                        _ => {
                            id.encode_count += accepted as u64;
                            id.execution_node_count += accepted as u64;
                        }
                    }
                }
                Ok((vectors(texts), id))
            },
        );
        assert!(accepted >= 2, "fixture did not reach identity comparison");
        assert_eq!(
            result.is_err(),
            drift != 3,
            "engine/load drift must abort; execution counters may advance"
        );
    }
}

#[test]
fn one_deadline_budget_decreases_across_rejected_and_accepted_rpcs() {
    let originals = vec!["word ".repeat(120)];
    let started = Instant::now();
    let timeout = Duration::from_secs(3);
    let mut calls = Vec::new();
    let batch = embed_windows(
        &originals,
        started,
        timeout,
        &|| false,
        |texts, budget, replay| {
            assert!(
                budget
                    <= (started + timeout).saturating_duration_since(Instant::now())
                        + Duration::from_millis(2)
            );
            std::thread::sleep(Duration::from_millis(3));
            let accepted = texts.iter().all(|text| text.len() <= 80);
            calls.push(Call {
                inputs: texts.to_vec(),
                timeout: budget,
                replay,
                accepted,
            });
            if !accepted {
                return Err(native_error(TOO_LONG));
            }
            Ok((vectors(texts), identity()))
        },
    )
    .unwrap();
    assert!(calls.iter().any(|call| !call.accepted));
    assert!(calls.iter().any(|call| call.accepted));
    assert!(
        calls
            .windows(2)
            .all(|pair| pair[1].timeout < pair[0].timeout),
        "per-RPC timeout reset instead of deducting prior work"
    );
    assert_eq!(batch.deadline, started + timeout);
    assert_exact_cover(&batch, &originals, &calls);
}

#[test]
fn cancellation_before_and_after_native_acceptance_returns_no_batch() {
    let originals = vec!["one short query".to_owned()];
    for initially_cancelled in [true, false] {
        let cancelled = Cell::new(initially_cancelled);
        let mut calls = 0;
        let result = embed_windows(
            &originals,
            Instant::now(),
            Duration::from_secs(2),
            &|| cancelled.get(),
            |texts, _, _| {
                calls += 1;
                cancelled.set(true);
                Ok((vectors(texts), identity()))
            },
        );
        assert!(
            result.is_err(),
            "cancelled operation returned a completed batch"
        );
        assert_eq!(calls, if initially_cancelled { 0 } else { 1 });
    }
}

#[test]
fn elapsed_deadline_before_or_during_callback_returns_no_batch() {
    let originals = vec!["short".to_owned()];
    let mut calls = 0;
    let expired = Instant::now() - Duration::from_secs(1);
    let result = embed_windows(
        &originals,
        expired,
        Duration::from_millis(1),
        &|| false,
        |_, _, _| {
            calls += 1;
            Ok((vec![vec![1.0]], identity()))
        },
    );
    assert!(result.is_err());
    assert_eq!(calls, 0);
    let result = embed_windows(
        &originals,
        Instant::now(),
        Duration::from_millis(1),
        &|| false,
        |texts, _, _| {
            std::thread::sleep(Duration::from_millis(5));
            Ok((vectors(texts), identity()))
        },
    );
    assert!(
        result.is_err(),
        "late accepted native result escaped deadline check"
    );
}

#[test]
fn work_limits_and_one_visible_character_refuse_finitely() {
    for (text, ceiling, always_refuse) in [
        ("w ".repeat(200), 4, false),
        ("word ".repeat(2048), 0, true),
        (
            format!("{}猫{}", " ".repeat(900), "\t".repeat(900)),
            64,
            false,
        ),
    ] {
        let mut calls = 0;
        let mut accepted_inputs = 0;
        let result = embed_windows(
            &[text],
            Instant::now(),
            Duration::from_secs(3),
            &|| false,
            |texts, _, _| {
                calls += 1;
                assert!(calls <= RPC_LIMIT, "RPC ceiling exceeded inside callback");
                assert!(texts.len() <= 8);
                if always_refuse || texts.iter().any(|text| text.len() > ceiling) {
                    return Err(native_error(TOO_LONG));
                }
                accepted_inputs += texts.len();
                assert!(
                    accepted_inputs <= LEAF_LIMIT,
                    "accepted leaf ceiling exceeded"
                );
                Ok((vectors(texts), identity()))
            },
        );
        assert!(calls > 0, "native typed failure not reached");
        assert!(
            result.is_err(),
            "work-limit or indivisible query returned partial success"
        );
    }
}

#[test]
fn original_aggregate_cap_refuses_without_embedding() {
    let originals = vec!["w ".repeat((1 << 19) + 1)];
    let mut calls = 0;
    let result = embed_windows(
        &originals,
        Instant::now(),
        Duration::from_secs(2),
        &|| false,
        |_, _, _| {
            calls += 1;
            Ok((vec![vec![1.0]], identity()))
        },
    );
    assert!(result.is_err());
    assert_eq!(calls, 0, "aggregate cap checked too late");
}
