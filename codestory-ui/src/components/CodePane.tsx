import Editor, { type OnMount } from "@monaco-editor/react";

import type { NodeDetailsDto } from "../generated/api";

type CodePaneProps = {
  projectOpen: boolean;
  activeFilePath: string | null;
  isDirty: boolean;
  isSaving: boolean;
  onSave: () => Promise<boolean>;
  activeNodeDetails: NodeDetailsDto | null;
  codeLanguage: string;
  draftText: string;
  onDraftChange: (text: string) => void;
  onEditorMount: OnMount;
};

export function CodePane({
  projectOpen,
  activeFilePath,
  isDirty,
  isSaving,
  onSave,
  activeNodeDetails,
  codeLanguage,
  draftText,
  onDraftChange,
  onEditorMount,
}: CodePaneProps) {
  return (
    <section className="pane pane-code">
      <div className="pane-header pane-code-header">
        <h2>Code Context</h2>
        <button
          onClick={() => void onSave()}
          disabled={!projectOpen || !activeFilePath || !isDirty || isSaving}
        >
          {isSaving ? "Saving..." : isDirty ? "Save" : "Saved"}
        </button>
      </div>

      {activeNodeDetails ? (
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
        <div className="graph-empty">Select a graph node to load source context.</div>
      )}

      {activeFilePath ? (
        <div className="monaco-shell">
          <Editor
            key={activeFilePath}
            path={activeFilePath}
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
          <div className="graph-empty">This symbol does not have readable source text.</div>
        )
      )}
    </section>
  );
}
