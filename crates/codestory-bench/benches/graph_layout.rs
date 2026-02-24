use std::hint::black_box;

use codestory_core::{Node, NodeId, NodeKind};
use codestory_graph::graph::GraphModel;
use codestory_graph::layout::{Layouter, NestingLayouter};
use criterion::{Criterion, criterion_group, criterion_main};

fn bench_nesting_layout_1000_nodes(c: &mut Criterion) {
    let mut model = GraphModel::new();
    let node_count = 1000;

    // Add 1k nodes
    for i in 0..node_count {
        model.add_node(Node {
            id: NodeId(i as i64),
            kind: NodeKind::CLASS,
            serialized_name: format!("Node_{}", i),
            ..Default::default()
        });
    }

    let layouter = NestingLayouter {
        inner_padding: NestingLayouter::DEFAULT_INNER_PADDING,
        child_spacing: NestingLayouter::DEFAULT_CHILD_SPACING,
        direction: codestory_core::LayoutDirection::Vertical,
    };

    c.bench_function("nesting_layout_1000_nodes", |b| {
        b.iter(|| {
            let (positions, sizes) = layouter.execute(black_box(&model));
            black_box((positions, sizes));
        })
    });
}

criterion_group!(benches, bench_nesting_layout_1000_nodes);
criterion_main!(benches);
