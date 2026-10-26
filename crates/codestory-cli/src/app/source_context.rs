//! Ordinary-tool transport and rendering. Runtime alone admits source units.
use crate::args::{InternalSourceContextCommand, InternalSourceContextMode};
use codestory_contracts::source_context::{
    SOURCE_CONTEXT_BOUNDS, SourceContextRequest, SourceContextResponse, SourceContextStatus,
};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Component, PathBuf};

#[cfg(unix)]
mod rows;
#[cfg(unix)]
mod transport;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Payload {
    version: u32,
    project: PathBuf,
    cwd: PathBuf,
    command: String,
    shim_directory: PathBuf,
}

impl Payload {
    fn decode(hex: &str) -> Option<Self> {
        if hex.len() > 65536 || hex.len() % 2 != 0 {
            return None;
        }
        let raw = hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let digit = |b: u8| (b as char).to_digit(16).map(|v| v as u8);
                Some(digit(pair[0])? * 16 + digit(pair[1])?)
            })
            .collect::<Option<Vec<_>>>()?;
        let value: Self = serde_json::from_slice(&raw).ok()?;
        if value.version != 1
            || value.command.is_empty()
            || value.command.len() > 16384
            || value.command.contains('\0')
        {
            return None;
        }
        for path in [&value.project, &value.cwd, &value.shim_directory] {
            if !path.is_absolute()
                || path
                    .components()
                    .any(|part| matches!(part, Component::ParentDir))
            {
                return None;
            }
        }
        value.cwd.strip_prefix(&value.project).ok()?;
        Some(value)
    }
}

pub(super) fn run(command: InternalSourceContextCommand) -> anyhow::Result<()> {
    match command.mode {
        InternalSourceContextMode::Context => {
            let mut raw = Vec::new();
            std::io::stdin().lock().take(262145).read_to_end(&mut raw)?;
            if raw.len() > 262144 {
                return Ok(());
            }
            let Ok(request) = serde_json::from_slice::<SourceContextRequest>(&raw) else {
                return Ok(());
            };
            let response = codestory_runtime::search_source_context::search_source_context(
                &request,
                &Default::default(),
            );
            std::io::stdout()
                .lock()
                .write_all(&render(&response, false))?;
            Ok(())
        }
        #[cfg(unix)]
        InternalSourceContextMode::Capture { payload } => {
            let payload = Payload::decode(&payload)
                .ok_or_else(|| anyhow::anyhow!("invalid source capture payload"))?;
            transport::capture(&payload)
        }
        #[cfg(unix)]
        InternalSourceContextMode::Rg { operands } => transport::rg(operands),
        #[cfg(unix)]
        InternalSourceContextMode::Setup {
            payload,
            rg,
            original_path,
        } => transport::setup(&payload, &rg, &original_path),
        #[cfg(not(unix))]
        _ => anyhow::bail!("ordinary source capture requires Unix"),
    }
}

/// Same per-unit identity framing as the source prototype. Runtime owns optional
/// bundle ordering; all framing/separators/coverage share the existing2KiB.
fn render(response: &SourceContextResponse, captured: bool) -> Vec<u8> {
    if response.status != SourceContextStatus::BoundedCurrentSource || response.units.is_empty() {
        return Vec::new();
    }
    let provenance = if captured {
        "bounded private actual-rg rows; outer original streams retained"
    } else {
        "supplied physical-row observations"
    };
    let summary = format!(
        "\n[exact enclosing source context; current post-search snapshot; {provenance}; omitted/dropped/incomplete observations UNKNOWN; not atomic, exhaustive or indexed proof]\n"
    );
    let mut reasons: Vec<(String, usize)> = Vec::new();
    for decline in &response.declines {
        let reason = serde_json::to_string(&decline.reason).unwrap_or_default();
        if let Some((_, count)) = reasons.iter_mut().find(|(key, _)| *key == reason) {
            *count += 1;
        } else {
            reasons.push((reason, 1));
        }
    }
    let reason_text = reasons
        .iter()
        .map(|(reason, count)| format!("{reason}:{count}"))
        .collect::<Vec<_>>()
        .join(",");
    let footer = |kept: usize, omitted: usize, source: usize| {
        format!(
            "\n[context coverage: {}/{} hits; {kept} complete units/header contexts; {} declines (display_budget={omitted}; reasons={reason_text}); limits64hits/16files/256KiBfile/2MiBsharedreads/8Mwork/depth256; source={source}/12288B metadata<=2048B; private IPC<=262144B/32children/65536Bchild; bounded post-search, not atomic/exhaustive/indexed; host truncation unknown]\n",
            response.coverage.observations_considered,
            response.coverage.observations_received,
            response.declines.len() + omitted
        )
    };
    let reserve = footer(
        response.units.len(),
        response.units.len(),
        SOURCE_CONTEXT_BOUNDS.display_source_bytes,
    )
    .len();
    let mut metadata = summary.len();
    if metadata + reserve > SOURCE_CONTEXT_BOUNDS.display_metadata_bytes {
        return Vec::new();
    }
    let mut output = summary.into_bytes();
    let mut source = 0usize;
    let mut kept = 0usize;
    let mut omitted = 0usize;
    // Ordinary responses keep their exact existing per-unit first-seen path.
    // Eligible responses carry whole-primary/partial-header dependencies from
    // runtime; admission is atomic under BOTH display allowances.
    let groups: Vec<Vec<usize>> = if let Some(selection) = &response.selection {
        let mut groups = Vec::new();
        for bundle in &selection.bundles {
            let Some(primary) = response.units.get(bundle.primary_unit_index) else {
                return Vec::new();
            };
            if primary.kind == "ancestor_header_context" {
                return Vec::new();
            }
            let mut group = vec![bundle.primary_unit_index];
            for &index in &bundle.required_header_indices {
                if response
                    .units
                    .get(index)
                    .is_none_or(|unit| unit.kind != "ancestor_header_context")
                {
                    return Vec::new();
                }
                if !group.contains(&index) {
                    group.push(index);
                }
            }
            groups.push(group);
        }
        groups
    } else {
        (0..response.units.len()).map(|index| vec![index]).collect()
    };
    let mut emitted = std::collections::BTreeSet::new();
    for group in groups {
        let fresh: Vec<_> = group
            .into_iter()
            .filter(|index| !emitted.contains(index))
            .collect();
        let mut framed = Vec::new();
        let mut group_source = 0usize;
        let mut group_metadata = 0usize;
        for &index in &fresh {
            let unit = &response.units[index];
            let path = serde_json::to_string(&unit.path).unwrap_or_default();
            let header = format!(
                "\n[source {path}; sha256={}; physical_lines={}-{}; {}; exact_bytes={}]\n",
                unit.source_sha256,
                unit.start_line,
                unit.end_line,
                unit.kind,
                unit.body.len()
            );
            let separator = usize::from(!unit.body.ends_with('\n'));
            group_source += unit.body.len();
            group_metadata += header.len() + separator;
            framed.push((index, header, separator));
        }
        if source + group_source > SOURCE_CONTEXT_BOUNDS.display_source_bytes
            || metadata + group_metadata + reserve > SOURCE_CONTEXT_BOUNDS.display_metadata_bytes
        {
            continue;
        }
        for (index, header, separator) in framed {
            let unit = &response.units[index];
            output.extend_from_slice(header.as_bytes());
            output.extend_from_slice(unit.body.as_bytes());
            if separator != 0 {
                output.push(b'\n');
            }
            emitted.insert(index);
            kept += 1;
        }
        metadata += group_metadata;
        source += group_source;
    }
    omitted += response.units.len() - kept;
    if kept == 0 {
        return Vec::new();
    }
    let tail = footer(kept, omitted, source);
    if metadata + tail.len() > SOURCE_CONTEXT_BOUNDS.display_metadata_bytes {
        return Vec::new();
    }
    output.extend_from_slice(tail.as_bytes());
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::source_context::{
        SourceContextBundle, SourceContextCoverage, SourceContextSelection, SourceContextUnit,
    };
    use sha2::{Digest, Sha256};

    fn unit(path: &str, source: &str, first: usize, last: usize, kind: &str) -> SourceContextUnit {
        let lines: Vec<_> = source.split_inclusive('\n').collect();
        let start = lines[..first - 1].iter().map(|s| s.len()).sum();
        let end = lines[..last].iter().map(|s| s.len()).sum();
        let body = &source[start..end];
        SourceContextUnit {
            path: path.into(),
            source_sha256: format!("{:x}", Sha256::digest(source.as_bytes())),
            start_line: first,
            end_line: last,
            start_byte: start,
            end_byte: end,
            kind: kind.into(),
            body: body.into(),
            body_sha256: format!("{:x}", Sha256::digest(body.as_bytes())),
        }
    }
    fn response(units: Vec<SourceContextUnit>) -> SourceContextResponse {
        SourceContextResponse {
            status: SourceContextStatus::BoundedCurrentSource,
            units,
            declines: Vec::new(),
            coverage: SourceContextCoverage {
                observations_received: 2,
                observations_considered: 2,
                ..Default::default()
            },
            bounds: SOURCE_CONTEXT_BOUNDS,
            current_post_search_snapshot: true,
            exhaustive_search: false,
            atomic_publication: false,
            indexed_proof: false,
            selection: None,
        }
    }

    #[test]
    fn runtime_unit_cost_equals_the_actual_existing_formatter_with_utf8_and_no_final_lf() {
        let source = "fn build() {\n    let value = \"雪🙂\";\n}";
        let item = unit("quoted\"/雪.rs", source, 1, 3, "complete_function_item");
        let bytes = render(&response(vec![item.clone()]), false);
        let begin = bytes.windows(9).position(|s| s == b"\n[source ").unwrap();
        let end = begin + bytes[begin..].windows(2).position(|s| s == b"]\n").unwrap() + 2;
        let actual = end - begin + source.len() + 1;
        assert_eq!(
            codestory_runtime::search_source_context::source_context_unit_cost(&item),
            actual,
            "rank cost must track the real formatter, not a separate framing convention"
        );
    }

    #[test]
    fn eligible_bundle_admission_never_leaves_a_header_when_its_primary_cannot_fit() {
        let large = format!(
            "fn large() {{\n    let value = \"{}\";\n}}\n",
            "x".repeat(11000)
        );
        let header = "impl Spare {\n";
        let small = format!(
            "    fn small() {{\n        let value = \"{}\";\n    }}\n",
            "y".repeat(2000)
        );
        let spare = format!("{header}{small}}}\n");
        let mut packet = response(vec![
            unit("large.rs", &large, 1, 3, "complete_function_item"),
            unit("spare.rs", &spare, 1, 1, "ancestor_header_context"),
            unit("spare.rs", &spare, 2, 4, "complete_function_item"),
        ]);
        packet.selection = Some(SourceContextSelection {
            observation_indices: vec![0, 1],
            bundles: vec![
                SourceContextBundle {
                    primary_unit_index: 0,
                    required_header_indices: vec![],
                },
                SourceContextBundle {
                    primary_unit_index: 2,
                    required_header_indices: vec![1],
                },
            ],
        });
        let bytes = render(&packet, false);
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains(&large));
        assert!(
            !text.contains(header),
            "partial header must stay with its declined primary"
        );
        assert!(!text.contains(&small));
        assert!(large.len() <= 12288 && text.len() - large.len() <= 2048);

        // All bodies are tiny: only the shared metadata allowance can refuse
        // the second bundle. Its header alone still fits after a valid bundle.
        let valid = "fn valid() {}\n";
        let meta_header = "impl Pending {\n";
        let pending = "    fn pending() {}\n";
        let combined = format!("{meta_header}{pending}}}\n");
        let first_path = format!("{}.rs", "a".repeat(400));
        let second_path = format!("{}.rs", "b".repeat(400));
        let items = vec![
            unit(&first_path, valid, 1, 1, "complete_function_item"),
            unit(&second_path, &combined, 1, 1, "ancestor_header_context"),
            unit(&second_path, &combined, 2, 2, "complete_function_item"),
        ];
        let control = String::from_utf8(render(&response(items[..2].to_vec()), false)).unwrap();
        assert!(
            control.contains(valid) && control.contains(meta_header),
            "valid bundle plus bare header fits metadata; no blanket-empty oracle"
        );
        let mut metadata_packet = response(items);
        metadata_packet.selection = packet.selection.clone();
        let metadata_text = String::from_utf8(render(&metadata_packet, false)).unwrap();
        assert!(
            metadata_text.contains(valid),
            "the earlier valid bundle must survive metadata exhaustion"
        );
        assert!(
            !metadata_text.contains(meta_header),
            "metadata refusal must not leave an orphan header"
        );
        assert!(!metadata_text.contains(pending));
        assert!(valid.len() + meta_header.len() + pending.len() < 12288);
        assert!(metadata_text.len() - valid.len() <= 2048);
    }
}
