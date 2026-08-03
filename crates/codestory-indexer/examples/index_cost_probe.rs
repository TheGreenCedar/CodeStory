//! Time `index_file` on synthetic Rust sources of a fixed shape.
//!
//! #1820 asks for a before/after measurement on the same fixture, so this is
//! deliberately reproducible rather than clever: one generator, one shape, the
//! minimum of three runs. Run it on both revisions and compare.
//!
//! `cargo run --release -p codestory-indexer --example index_cost_probe`

use std::path::Path;
use std::time::Instant;

fn dense_rust_source(target_bytes: usize) -> String {
    let mut source = String::with_capacity(target_bytes + 512);
    let mut index = 0usize;
    while source.len() < target_bytes {
        source.push_str(&format!(
            "pub struct Holder{index} {{\n    pub field_{index}: i64,\n}}\n\n\
             impl Holder{index} {{\n    pub fn read_{index}(&self) -> i64 {{\n\
             \x20       let local = self.field_{index};\n        local + {index}\n    }}\n\
             \x20   fn private_{index}(&self) -> i64 {{\n        self.read_{index}()\n    }}\n}}\n\n"
        ));
        index += 1;
    }
    source
}

fn main() {
    for target in [100_000usize, 250_000, 500_000, 1_000_000] {
        let source = dense_rust_source(target);
        let path = Path::new("probe.rs");
        let config = codestory_indexer::get_language_for_ext("rs").expect("rust config");
        let mut best = u128::MAX;
        let mut nodes = 0usize;
        for _ in 0..3 {
            let started = Instant::now();
            let result =
                codestory_indexer::index_file(path, &source, &config, None, None).expect("index");
            best = best.min(started.elapsed().as_millis());
            nodes = result.nodes.len();
        }
        println!(
            "{:>9} bytes  {:>7} ms  {:>6} nodes",
            source.len(),
            best,
            nodes
        );
    }
}
