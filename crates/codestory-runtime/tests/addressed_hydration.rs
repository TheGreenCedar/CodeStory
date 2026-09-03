#![cfg(feature = "benchmark-support")]

use codestory_contracts::evidence_address::{
    ByteRangeV1, LineRangeV1, ProjectRelativePath, SourceRangeV1,
};
use codestory_contracts::packet_projection_v3::Sha256DigestV3Dto;
use codestory_runtime::benchmark_support::hydrate_addressed_range;
use sha2::{Digest, Sha256};

fn source_range(source: &str, start: usize, end: usize) -> SourceRangeV1 {
    SourceRangeV1 {
        path: ProjectRelativePath::new("src/module.rs").unwrap(),
        byte_range: ByteRangeV1::new(start as u64, end as u64).unwrap(),
        line_range: LineRangeV1::new(
            source.as_bytes()[..start]
                .iter()
                .filter(|byte| **byte == b'\n')
                .count() as u32
                + 1,
            source.as_bytes()[..end - 1]
                .iter()
                .filter(|byte| **byte == b'\n')
                .count() as u32
                + 1,
        )
        .unwrap(),
        content_digest: Sha256DigestV3Dto::new(format!("{:x}", Sha256::digest(source))).unwrap(),
    }
}

#[test]
fn addressed_hydration_keeps_the_witness_after_a_long_header() {
    let header = "// header only\n".repeat(100);
    let function = "fn target() {\n    next();\n}\n";
    let source = format!("{header}{function}");
    let start = source.find("next()").unwrap();
    let matched = source_range(&source, start, start + "next()".len());
    let syntax = source_range(&source, header.len(), source.len());
    let hydrated = hydrate_addressed_range(&source, &matched, &[syntax], 512).unwrap();
    assert_eq!(hydrated.source, function);
    assert_eq!(hydrated.range.line_range.start(), 101);
    assert_eq!(hydrated.range.line_range.end(), 103);
    assert!(!hydrated.truncated);
    assert!(!hydrated.markdown.contains("header only"));
}

#[test]
fn hostile_source_windows_are_complete_content_bound_and_deterministic() {
    let source = format!(
        "fn enclosing() {{\r\n{}    café();\r\n{}}}\r\n",
        "    alpha();\r\n".repeat(50),
        "    omega();\r\n".repeat(50)
    );
    let start = source.find("café").unwrap();
    let matched = source_range(&source, start, start + "café".len());
    let syntax = source_range(&source, 0, source.len());
    let hydrated = hydrate_addressed_range(&source, &matched, &[syntax.clone()], 512).unwrap();
    assert!(hydrated.source.contains("café();\r\n"));
    assert!(hydrated.source.ends_with("\r\n"));
    assert!(hydrated.markdown.len() <= 512);
    assert!(hydrated.truncated);
    assert_eq!(
        hydrated,
        hydrate_addressed_range(&source, &matched, &[syntax], 512).unwrap()
    );
    assert_eq!(
        hydrated.source,
        source
            [hydrated.range.byte_range.start() as usize..hydrated.range.byte_range.end() as usize]
    );

    let mut changed = matched.clone();
    changed.content_digest = Sha256DigestV3Dto::new("0".repeat(64)).unwrap();
    assert!(hydrate_addressed_range(&source, &changed, &[], 512).is_err());
    changed = matched.clone();
    changed.line_range = LineRangeV1::new(1, 1).unwrap();
    assert!(hydrate_addressed_range(&source, &changed, &[], 512).is_err());
    changed = matched;
    changed.byte_range = ByteRangeV1::new((start + 4) as u64, (start + 5) as u64).unwrap();
    assert!(hydrate_addressed_range(&source, &changed, &[], 512).is_err());

    let giant = format!("{}\n", "é".repeat(600));
    assert!(hydrate_addressed_range(&giant, &source_range(&giant, 0, 2), &[], 512).is_err());
}
