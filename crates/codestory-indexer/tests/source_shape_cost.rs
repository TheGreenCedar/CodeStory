//! IDX-PERF (#1820): indexing cost must not depend on a file's *shape*.
//!
//! Every fixture in this crate is written the way people write code — many
//! small functions — and that is why a defect making cost quadratic in the
//! number of statements *inside one function* went unnoticed. At a fixed
//! ~500 KB, moving statements from many small functions into one giant one
//! took `index_file` from 1,219 ms to 134,467 ms: 110x at constant byte count,
//! with no fixture in the tree able to see it.
//!
//! This is a blow-up detector, not a benchmark. The budget is set roughly two
//! orders of magnitude above the measured cost so ordinary CI noise, a loaded
//! runner, or a slower machine cannot trip it — while the quadratic it guards
//! against, which was ~30x worse than the budget, cannot pass.

use std::path::Path;
use std::time::{Duration, Instant};

/// Generous by construction: the shape below measures ~112 ms in a debug build
/// and the quadratic it replaces measured in the tens of seconds.
const BUDGET: Duration = Duration::from_secs(5);

/// One function whose body is a long run of typed `let` bindings, each
/// referring to an earlier one. That last part matters: receiver inference
/// resolves each binding against the bindings before it, so this shape
/// exercises the dependency chain rather than just the statement count.
fn one_giant_function(target_bytes: usize) -> String {
    let mut source = String::with_capacity(target_bytes + 256);
    source.push_str("pub struct Wide {\n    pub field: i64,\n}\n\n");
    source
        .push_str("impl Wide {\n    pub fn get(&self) -> i64 {\n        self.field\n    }\n}\n\n");
    source.push_str("pub fn giant() -> i64 {\n    let base: Wide = Wide { field: 0 };\n");
    let mut index = 0usize;
    while source.len() < target_bytes {
        source.push_str(&format!(
            "    let value_{index}: Wide = Wide {{ field: base.get() + {index} }};\n"
        ));
        index += 1;
    }
    source.push_str("    base.get()\n}\n");
    source
}

#[test]
fn one_giant_function_costs_about_what_many_small_ones_do() {
    let source = one_giant_function(100_000);
    let config = codestory_indexer::get_language_for_ext("rs").expect("rust language config");

    let started = Instant::now();
    let result = codestory_indexer::index_file(Path::new("giant.rs"), &source, &config, None, None)
        .expect("index the giant function");
    let elapsed = started.elapsed();

    assert!(
        !result.nodes.is_empty(),
        "the fixture must actually project nodes, or this guard measures nothing"
    );
    assert!(
        elapsed < BUDGET,
        "indexing {} bytes of one-giant-function Rust took {elapsed:?}, over the \
         {BUDGET:?} budget. Cost has become dependent on statements-per-function \
         again — see #1820. This budget is ~45x the expected cost, so it does not \
         fail for being slow; it fails for being quadratic.",
        source.len()
    );
}
