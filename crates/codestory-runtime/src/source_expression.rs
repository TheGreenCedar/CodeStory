//! One observational current-file consumer of the pure local Go compiler.
use codestory_contracts::source_expression::*;
use codestory_indexer::CancellationToken;
#[cfg(unix)]
use codestory_indexer::source_units::SourceUnitBudget;
#[cfg(unix)]
use codestory_indexer::source_units::go_expression_slice::{
    GoExpressionSlice, GoSliceBoundaryKind, GoSliceGap, GoSliceRole, compile_go_expression_slice,
};
#[cfg(unix)]
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::ops::Range;

pub fn analyze_expression(
    request: &SourceExpressionRequest,
    cancellation: &CancellationToken,
) -> SourceExpressionResponse {
    if cancellation.is_cancelled() {
        return SourceExpressionResponse::unavailable(AnalysisDecline::Cancelled);
    }
    if request
        .project
        .len()
        .saturating_add(request.path.len())
        .saturating_add(request.expression.len())
        > MAX_INPUT_BYTES
        || request.project.is_empty()
        || request.path.is_empty()
        || request.expression.is_empty()
    {
        return SourceExpressionResponse::unavailable(AnalysisDecline::InvalidRequest);
    }
    if request.line == 0 || request.line > 1_000_000 {
        return SourceExpressionResponse::unavailable(AnalysisDecline::InvalidLine);
    }
    #[cfg(unix)]
    {
        match analyze_current(request, cancellation) {
            Ok(response) => response,
            Err(reason) => {
                let mut response = SourceExpressionResponse::unavailable(reason);
                if reason == AnalysisDecline::AmbiguousExpression {
                    response.matching_candidates_at_least = Some(2);
                }
                response
            }
        }
    }
    #[cfg(not(unix))]
    {
        // shortcut: Windows needs an equivalent handle-relative, reparse-safe
        // reader and native conformance before cross-platform delivery. Never
        // substitute the weaker pathname reader or claim permanent Unix scope.
        SourceExpressionResponse::unavailable(AnalysisDecline::UnsupportedPlatform)
    }
}

#[cfg(unix)]
fn charge(
    budget: &mut SourceUnitBudget,
    cancellation: &CancellationToken,
    amount: usize,
) -> Result<(), AnalysisDecline> {
    if cancellation.is_cancelled() {
        return Err(AnalysisDecline::Cancelled);
    }
    if amount > budget.remaining_work {
        budget.remaining_work = 0;
        return Err(AnalysisDecline::WorkBudgetExceeded);
    }
    budget.remaining_work -= amount;
    Ok(())
}

#[cfg(unix)]
fn read_reason(
    reason: codestory_contracts::source_context::SourceContextDeclineReason,
) -> AnalysisDecline {
    use codestory_contracts::source_context::SourceContextDeclineReason as R;
    match reason {
        R::Cancelled => AnalysisDecline::Cancelled,
        R::InvalidProject => AnalysisDecline::InvalidProject,
        R::InvalidPath | R::InvalidCwd => AnalysisDecline::InvalidPath,
        R::SourceDrift => AnalysisDecline::SourceDrift,
        R::FileTooLarge => AnalysisDecline::FileTooLarge,
        R::ReadBudgetExceeded => AnalysisDecline::ReadBudgetExceeded,
        _ => AnalysisDecline::UnreadableSource,
    }
}

#[cfg(unix)]
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(unix)]
fn analyze_current(
    request: &SourceExpressionRequest,
    cancellation: &CancellationToken,
) -> Result<SourceExpressionResponse, AnalysisDecline> {
    use crate::search_source_context::io::Directory;
    use std::path::Path;
    let mut budget = SourceUnitBudget {
        remaining_source_bytes: MAX_SOURCE_BYTES,
        remaining_work: MAX_WORK,
        max_depth: MAX_DEPTH,
    };
    // Reserve bounded final profile serialization/escaping work before analysis.
    charge(&mut budget, cancellation, MAX_RESULT_BYTES * 8)?;
    charge(
        &mut budget,
        cancellation,
        request
            .project
            .len()
            .saturating_add(request.path.len())
            .saturating_add(request.expression.len()),
    )?;
    let root = Directory::pin(Path::new(&request.project), cancellation).map_err(read_reason)?;
    let relative = root
        .source_path(Path::new(""), Path::new(&request.path))
        .map_err(read_reason)?;
    if relative.extension().and_then(|s| s.to_str()) != Some("go") {
        return Err(AnalysisDecline::UnsupportedSource);
    }
    let mut remaining_reads = MAX_READ_BYTES;
    // Precharge the fixed maximum before the native bounded allocation/read.
    charge(&mut budget, cancellation, MAX_SOURCE_BYTES)?;
    let read = root
        .read(
            &relative,
            None,
            MAX_SOURCE_BYTES,
            &mut remaining_reads,
            cancellation,
        )
        .map_err(read_reason)?;
    let text = std::str::from_utf8(&read.bytes).map_err(|_| AnalysisDecline::InvalidUtf8)?;
    if text.is_empty() {
        return Err(AnalysisDecline::InvalidLine);
    }
    charge(
        &mut budget,
        cancellation,
        text.len()
            .saturating_mul(3)
            .saturating_add(request.expression.len()),
    )?;
    let mut starts = vec![0usize];
    let mut ends = Vec::new();
    for (at, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            ends.push(at + 1);
            if at + 1 < text.len() {
                starts.push(at + 1);
            }
        }
    }
    if ends.len() < starts.len() {
        ends.push(text.len());
    }
    let first = request
        .line
        .checked_sub(1)
        .and_then(|line| starts.get(line))
        .copied()
        .ok_or(AnalysisDecline::InvalidLine)?;
    let line_end = *ends
        .get(request.line - 1)
        .ok_or(AnalysisDecline::InvalidLine)?;
    // Two linear substring searches also detect overlapping matches; a
    // non-overlapping match iterator could silently choose one occurrence.
    let at = text[first..]
        .find(request.expression.as_str())
        .map(|offset| first + offset)
        .filter(|&at| at < line_end)
        .ok_or(AnalysisDecline::MissingExpression)?;
    let next = at
        + text[at..]
            .chars()
            .next()
            .ok_or(AnalysisDecline::MissingExpression)?
            .len_utf8();
    if next < line_end
        && text[next..]
            .find(request.expression.as_str())
            .is_some_and(|offset| next + offset < line_end)
    {
        return Err(AnalysisDecline::AmbiguousExpression);
    }
    let criterion = at..at + request.expression.len();
    let analysis = compile_go_expression_slice(text, criterion, &mut budget, cancellation)
        .map_err(|error| {
            use codestory_indexer::source_units::SourceUnitError as E;
            match error {
                E::Cancelled => AnalysisDecline::Cancelled,
                E::DepthExceeded => AnalysisDecline::DepthExceeded,
                E::WorkBudgetExceeded => AnalysisDecline::WorkBudgetExceeded,
                E::SourceByteBudgetExceeded => AnalysisDecline::ReadBudgetExceeded,
                E::MalformedSource => AnalysisDecline::MalformedSource,
                E::ParserUnavailable | E::UnsupportedSyntax => AnalysisDecline::UnsupportedSource,
            }
        })?;
    if analysis.gaps.contains(&GoSliceGap::InvalidCriterion) {
        return Err(AnalysisDecline::InvalidCriterion);
    }
    let path = relative
        .to_str()
        .ok_or(AnalysisDecline::InvalidPath)?
        .to_owned();
    charge(
        &mut budget,
        cancellation,
        read.bytes
            .len()
            .saturating_add(path.len())
            .saturating_add(request.project.len()),
    )?;
    let snapshot = AnalysisSnapshot {
        project: request.project.clone(),
        path,
        source_sha256: digest(&read.bytes),
        source_kind: "current_workspace".into(),
        revalidated: true,
        indexed_proof: false,
        atomic_publication: false,
    };
    let result = project(
        text,
        &starts,
        &ends,
        analysis,
        snapshot,
        &mut budget,
        cancellation,
    )?;
    canonical_fits(&result, &mut budget, cancellation)?;
    #[cfg(test)]
    crate::search_source_context::source_context_test_stage(
        crate::search_source_context::SourceContextTestStage::BeforeFinalRecheck,
    );
    charge(&mut budget, cancellation, MAX_SOURCE_BYTES)?;
    let again = root
        .read(
            &relative,
            Some(&read.identity),
            MAX_SOURCE_BYTES,
            &mut remaining_reads,
            cancellation,
        )
        .map_err(read_reason)?;
    charge(&mut budget, cancellation, again.bytes.len())?;
    if again.bytes != read.bytes {
        return Err(AnalysisDecline::SourceDrift);
    }
    root.validate_root(cancellation).map_err(read_reason)?;
    charge(&mut budget, cancellation, 0)?;
    Ok(result)
}

#[cfg(unix)]
fn project(
    text: &str,
    starts: &[usize],
    ends: &[usize],
    analysis: GoExpressionSlice,
    snapshot: AnalysisSnapshot,
    budget: &mut SourceUnitBudget,
    cancellation: &CancellationToken,
) -> Result<SourceExpressionResponse, AnalysisDecline> {
    let count = analysis
        .witnesses
        .len()
        .saturating_add(analysis.boundaries.len());
    charge(
        budget,
        cancellation,
        count.saturating_mul(32).saturating_add(text.len()),
    )?;
    let envelope = |range: Range<usize>| -> Result<Range<usize>, AnalysisDecline> {
        if range.start >= range.end || text.get(range.clone()).is_none() {
            return Err(AnalysisDecline::MalformedSource);
        }
        let first = starts
            .partition_point(|&p| p <= range.start)
            .checked_sub(1)
            .ok_or(AnalysisDecline::MalformedSource)?;
        let last = starts
            .partition_point(|&p| p < range.end)
            .checked_sub(1)
            .ok_or(AnalysisDecline::MalformedSource)?;
        Ok(starts[first]..ends[last])
    };
    let mut windows = Vec::new();
    for witness in &analysis.witnesses {
        windows.push(envelope(witness.unit.start_byte..witness.unit.end_byte)?);
    }
    for boundary in &analysis.boundaries {
        windows.push(envelope(boundary.range.clone())?);
    }
    let log = usize::BITS as usize - windows.len().saturating_add(1).leading_zeros() as usize + 1;
    charge(budget, cancellation, windows.len().saturating_mul(log))?;
    windows.sort_unstable_by_key(|range| (range.start, range.end));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in windows {
        charge(budget, cancellation, 1)?;
        if let Some(previous) = merged.last_mut() {
            if range.start <= previous.end {
                previous.end = previous.end.max(range.end);
                continue;
            }
        }
        merged.push(range);
    }
    let mut result = SourceExpressionResponse::unavailable(AnalysisDecline::InvalidCriterion);
    result.status = if analysis.complete_local {
        AnalysisStatus::Available
    } else {
        AnalysisStatus::Partial
    };
    result.decline = None;
    result.complete_local = analysis.complete_local;
    result.snapshot = Some(snapshot);
    result.criterion = Some(AnalysisRange {
        start_byte: analysis.criterion.start,
        end_byte: analysis.criterion.end,
    });
    for range in &merged {
        charge(
            budget,
            cancellation,
            range.len().saturating_mul(8).saturating_add(log * 2),
        )?;
        let source = &text[range.clone()];
        result.segments.push(AnalysisSegment {
            start_byte: range.start,
            end_byte: range.end,
            start_line: starts.partition_point(|&p| p <= range.start),
            end_line: starts.partition_point(|&p| p < range.end),
            body_sha256: digest(source.as_bytes()),
            source: source.to_owned(),
        });
    }
    for witness in analysis.witnesses {
        charge(budget, cancellation, merged.len().saturating_add(1))?;
        let segment = merged
            .iter()
            .position(|s| s.start <= witness.range.start && witness.range.end <= s.end)
            .ok_or(AnalysisDecline::MalformedSource)?;
        let role = match witness.role {
            GoSliceRole::Criterion => AnalysisRole::Criterion,
            GoSliceRole::Binding => AnalysisRole::Binding,
            GoSliceRole::Definition => AnalysisRole::Definition,
            GoSliceRole::Control => AnalysisRole::Control,
        };
        result.witnesses.push(AnalysisWitness {
            role,
            range: AnalysisRange {
                start_byte: witness.range.start,
                end_byte: witness.range.end,
            },
            segment,
        });
    }
    for boundary in analysis.boundaries {
        charge(budget, cancellation, merged.len().saturating_add(1))?;
        let segment = merged
            .iter()
            .position(|s| s.start <= boundary.range.start && boundary.range.end <= s.end)
            .ok_or(AnalysisDecline::MalformedSource)?;
        let kind = match boundary.kind {
            GoSliceBoundaryKind::ParameterInput => AnalysisBoundaryKind::ParameterInput,
            GoSliceBoundaryKind::PackageInput => AnalysisBoundaryKind::UnresolvedBinding,
            GoSliceBoundaryKind::OpaqueCall => AnalysisBoundaryKind::OpaqueCall,
            GoSliceBoundaryKind::IndirectWrite => AnalysisBoundaryKind::PossibleOpaqueMutation,
            GoSliceBoundaryKind::ClosureWrite => AnalysisBoundaryKind::PossibleClosureMutation,
            GoSliceBoundaryKind::UnsupportedConstruct => AnalysisBoundaryKind::UnsupportedConstruct,
        };
        result.boundaries.push(AnalysisBoundary {
            kind,
            range: AnalysisRange {
                start_byte: boundary.range.start,
                end_byte: boundary.range.end,
            },
            segment,
        });
    }
    for gap in analysis.gaps {
        charge(budget, cancellation, 1)?;
        result.gaps.push(match gap {
            GoSliceGap::InvalidCriterion => AnalysisGap::InvalidCriterion,
            GoSliceGap::UnsupportedConstruct => AnalysisGap::UnsupportedConstruct,
            GoSliceGap::LoopCarriedState => AnalysisGap::LoopCarriedState,
            GoSliceGap::UnknownMutation => AnalysisGap::UnknownMutation,
        });
    }
    Ok(result)
}

#[cfg(unix)]
fn canonical_fits(
    response: &SourceExpressionResponse,
    budget: &mut SourceUnitBudget,
    cancellation: &CancellationToken,
) -> Result<(), AnalysisDecline> {
    // A canonical DTO larger than the complete result cap cannot fit any MCP
    // profile. This bounded lower-bound check is not a second output quota.
    struct Counter<'a, 'b> {
        count: usize,
        reason: Option<AnalysisDecline>,
        budget: &'a mut SourceUnitBudget,
        cancellation: &'b CancellationToken,
    }
    impl std::io::Write for Counter<'_, '_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if let Err(reason) = charge(self.budget, self.cancellation, bytes.len()) {
                self.reason = Some(reason);
                return Err(std::io::Error::other("analysis budget"));
            }
            if self.count.saturating_add(bytes.len()) > MAX_RESULT_BYTES {
                self.reason = Some(AnalysisDecline::ResultBudgetExceeded);
                return Err(std::io::Error::other("result budget"));
            }
            self.count += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter {
        count: 0,
        reason: None,
        budget,
        cancellation,
    };
    if serde_json::to_writer(&mut counter, response).is_err() {
        return Err(counter
            .reason
            .unwrap_or(AnalysisDecline::ResultBudgetExceeded));
    }
    Ok(())
}

#[cfg(all(test, unix))]
#[path = "source_expression_tests.rs"]
mod tests;

#[cfg(all(test, not(unix)))]
mod platform_tests {
    use super::*;
    #[test]
    fn missing_native_adapter_never_fabricates_snapshot() {
        let response = analyze_expression(
            &SourceExpressionRequest {
                project: "/repo".into(),
                path: "unit.go".into(),
                line: 1,
                expression: "value".into(),
            },
            &CancellationToken::new(),
        );
        assert_eq!(response.decline, Some(AnalysisDecline::UnsupportedPlatform));
        assert!(response.snapshot.is_none() && response.segments.is_empty());
    }
}
