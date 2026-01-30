use codestory_core::{Node, NodeId, NodeKind};
use codestory_graph::graph::GraphModel;
use codestory_graph::layout::{GridLayouter, Layouter};
use criterion::{Criterion, black_box, criterion_group, criterion_main};

fn bench_grid_layout_1000_nodes(c: &mut Criterion) {
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

    let layouter = GridLayouter { spacing: 100.0 };

    c.bench_function("grid_layout_1000_nodes", |b| {
        b.iter(|| {
            let positions = layouter.execute(black_box(&model));
            black_box(positions);
        })
    });
}

criterion_group!(benches, bench_grid_layout_1000_nodes);
criterion_main!(benches);
