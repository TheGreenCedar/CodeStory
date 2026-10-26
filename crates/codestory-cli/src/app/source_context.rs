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

/// Same per-unit identity framing and whole-unit first-seen order as the source
/// prototype. Metadata, including all framing/separators/coverage, shares2KiB.
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
    for unit in &response.units {
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
        if source + unit.body.len() > SOURCE_CONTEXT_BOUNDS.display_source_bytes
            || metadata + header.len() + separator + reserve
                > SOURCE_CONTEXT_BOUNDS.display_metadata_bytes
        {
            omitted += 1;
            continue;
        }
        output.extend_from_slice(header.as_bytes());
        output.extend_from_slice(unit.body.as_bytes());
        if separator != 0 {
            output.push(b'\n');
        }
        metadata += header.len() + separator;
        source += unit.body.len();
        kept += 1;
    }
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
