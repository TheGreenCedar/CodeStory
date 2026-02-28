import Editor, { type OnMount } from "@monaco-editor/react";

import type { NodeDetailsDto, SourceOccurrenceDto } from "../generated/api";

export type CodeEdgeContext = {
  id: string;
  kind: string;
  sourceLabel: string;
  targetLabel: string;
};

type CodePaneProps = {
  projectOpen: boolean;
  activeFilePath: string | null;
  monacoModelPath: string | null;
  isDirty: boolean;
  isSaving: boolean;
  onSave: () => Promise<boolean>;
  activeNodeDetails: NodeDetailsDto | null;
  activeEdgeContext: CodeEdgeContext | null;
  occurrences: SourceOccurrenceDto[];
  activeOccurrenceIndex: number;
  onSelectOccurrence: (index: number) => void;
  onNextOccurrence: () => void;
  onPreviousOccurrence: () => void;
  codeLanguage: string;
  draftText: string;
  onDraftChange: (text: string) => void;
  onEditorMount: OnMount;
};

export function CodePane({
  projectOpen,
  activeFilePath,
  monacoModelPath,
  isDirty,
  isSaving,
  onSave,
  activeNodeDetails,
  activeEdgeContext,
  occurrences,
  activeOccurrenceIndex,
  onSelectOccurrence,
  onNextOccurrence,
  onPreviousOccurrence,
  codeLanguage,
  draftText,
  onDraftChange,
  onEditorMount,
}: CodePaneProps) {
  const activeOccurrence =
    occurrences.length > 0
      ? (occurrences[Math.min(activeOccurrenceIndex, occurrences.length - 1)] ?? null)
      : null;

  return (
    <section className="pane pane-code">
      <div className="pane-header pane-code-header">
        <h2>Code</h2>
        <button
          onClick={() => void onSave()}
          disabled={!projectOpen || !activeFilePath || !isDirty || isSaving}
        >
          {isSaving ? "Saving..." : isDirty ? "Save" : "Saved"}
        </button>
      </div>

      {activeEdgeContext ? (
        <div className="node-meta">
          <div>
            <strong>{activeEdgeContext.kind}</strong>
            <span>{activeEdgeContext.sourceLabel}</span>
            <span>→</span>
            <span>{activeEdgeContext.targetLabel}</span>
            {isDirty && <span className="dirty-pill">Unsaved</span>}
          </div>
          <div>
            {activeOccurrence
              ? `${activeOccurrence.file_path}:${activeOccurrence.start_line}`
              : "No source locations"}
          </div>
        </div>
      ) : activeNodeDetails ? (
        <div className="node-meta">
          <div>
            <strong>{activeNodeDetails.display_name}</strong>
            <span>{activeNodeDetails.kind}</span>
            {isDirty && <span className="dirty-pill">Unsaved</span>}
          </div>
          <div>
            {activeNodeDetails.file_path
              ? `${activeNodeDetails.file_path}:${activeNodeDetails.start_line ?? "-"}`
              : "No file location"}
          </div>
        </div>
      ) : (
        <div className="graph-empty">Select a symbol in Graph or Explorer to load code.</div>
      )}

      {occurrences.length > 0 ? (
        <div className="code-occurrence-toolbar">
          <button
            type="button"
            onClick={onPreviousOccurrence}
            disabled={occurrences.length <= 1}
            aria-label="Previous source location"
          >
            Prev
          </button>
          <span className="code-occurrence-summary">
            {Math.min(activeOccurrenceIndex + 1, occurrences.length)} / {occurrences.length}
          </span>
          <button
            type="button"
            onClick={onNextOccurrence}
            disabled={occurrences.length <= 1}
            aria-label="Next source location"
          >
            Next
          </button>
          <div className="code-occurrence-list" role="listbox" aria-label="Source locations">
            {occurrences.slice(0, 24).map((occurrence, idx) => (
              <button
                key={`${occurrence.element_id}-${occurrence.file_path}-${occurrence.start_line}-${occurrence.start_col}-${idx}`}
                type="button"
                className={
                  idx === activeOccurrenceIndex
                    ? "code-occurrence-chip active"
                    : "code-occurrence-chip"
                }
                onClick={() => onSelectOccurrence(idx)}
              >
                {occurrence.file_path}:{occurrence.start_line}
              </button>
            ))}
          </div>
        </div>
      ) : null}

      {activeFilePath ? (
        <div className="monaco-shell">
          <Editor
            key={monacoModelPath ?? activeFilePath}
            path={monacoModelPath ?? activeFilePath}
            language={codeLanguage}
            value={draftText}
            onChange={(next) => onDraftChange(next ?? "")}
            onMount={onEditorMount}
            theme="vs"
            options={{
              minimap: { enabled: true },
              fontFamily: "var(--font-mono)",
              fontSize: 13,
              lineNumbers: "on",
              scrollBeyondLastLine: false,
              automaticLayout: true,
              tabSize: 2,
              renderWhitespace: "selection",
            }}
          />
        </div>
      ) : (
        activeNodeDetails && (
          <div className="graph-empty">No readable source here. Try another occurrence.</div>
        )
      )}
    </section>
  );
}
