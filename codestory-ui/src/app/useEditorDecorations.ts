import { useCallback, useEffect, useRef, type MutableRefObject } from "react";
import { type Monaco, type OnMount } from "@monaco-editor/react";

import type { CodeEdgeContext } from "../components/CodePane";
import type { NodeDetailsDto, SourceOccurrenceDto } from "../generated/api";
import { GRAPH_THEME } from "../theme/tokens";

type UseEditorDecorationsArgs = {
  saveCurrentFile: () => Promise<boolean>;
  activeFilePath: string | null;
  activeEdgeContext: CodeEdgeContext | null;
  activeOccurrences: SourceOccurrenceDto[];
  activeOccurrenceIndex: number;
  activeNodeDetails: NodeDetailsDto | null;
  draftText: string;
};

export type EditorDecorations = {
  editorRef: MutableRefObject<Parameters<OnMount>[0] | null>;
  monacoRef: MutableRefObject<Monaco | null>;
  handleEditorMount: OnMount;
};

export function useEditorDecorations({
  saveCurrentFile,
  activeFilePath,
  activeEdgeContext,
  activeOccurrences,
  activeOccurrenceIndex,
  activeNodeDetails,
  draftText,
}: UseEditorDecorationsArgs): EditorDecorations {
  const saveActionRef = useRef<() => Promise<boolean>>(async () => false);
  const editorRef = useRef<Parameters<OnMount>[0] | null>(null);
  const monacoRef = useRef<Monaco | null>(null);
  const decorationIdsRef = useRef<string[]>([]);

  useEffect(() => {
    saveActionRef.current = saveCurrentFile;
  }, [saveCurrentFile]);

  const handleEditorMount = useCallback<OnMount>((editor, monaco) => {
    editorRef.current = editor;
    monacoRef.current = monaco;

    const tsDefaults = monaco.languages.typescript.typescriptDefaults;
    const jsDefaults = monaco.languages.typescript.javascriptDefaults;
    const sharedCompilerOptions = {
      allowNonTsExtensions: true,
      allowJs: true,
      target: monaco.languages.typescript.ScriptTarget.ESNext,
      module: monaco.languages.typescript.ModuleKind.ESNext,
      moduleResolution: monaco.languages.typescript.ModuleResolutionKind.NodeJs,
      jsx: monaco.languages.typescript.JsxEmit.ReactJSX,
    };

    tsDefaults.setEagerModelSync(true);
    jsDefaults.setEagerModelSync(true);
    tsDefaults.setCompilerOptions(sharedCompilerOptions);
    jsDefaults.setCompilerOptions(sharedCompilerOptions);

    const sharedDiagnostics = {
      noSyntaxValidation: false,
      noSemanticValidation: false,
      diagnosticCodesToIgnore: [2307, 2792],
    };

    tsDefaults.setDiagnosticsOptions(sharedDiagnostics);
    jsDefaults.setDiagnosticsOptions(sharedDiagnostics);

    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => {
      void saveActionRef.current();
    });
  }, []);

  useEffect(() => {
    const editor = editorRef.current;
    const monaco = monacoRef.current;
    if (!editor || !monaco) {
      return;
    }

    if (!activeFilePath) {
      decorationIdsRef.current = editor.deltaDecorations(decorationIdsRef.current, []);
      return;
    }

    const activeOccurrence =
      activeOccurrences.length > 0
        ? (activeOccurrences[Math.min(activeOccurrenceIndex, activeOccurrences.length - 1)] ?? null)
        : null;

    const hasEdgeRange = Boolean(activeEdgeContext && activeOccurrence);
    const nodeStartLine = activeNodeDetails?.start_line ?? null;
    if (!hasEdgeRange && !nodeStartLine) {
      decorationIdsRef.current = editor.deltaDecorations(decorationIdsRef.current, []);
      return;
    }
    const startLine = hasEdgeRange
      ? Math.max(1, activeOccurrence?.start_line ?? 1)
      : Math.max(1, nodeStartLine ?? 1);

    const startColumn = hasEdgeRange
      ? Math.max(1, activeOccurrence?.start_col ?? 1)
      : Math.max(1, activeNodeDetails?.start_col ?? 1);
    const endLine = hasEdgeRange
      ? Math.max(startLine, activeOccurrence?.end_line ?? startLine)
      : Math.max(startLine, activeNodeDetails?.end_line ?? startLine);
    const endColumn = hasEdgeRange
      ? endLine === startLine
        ? Math.max(startColumn + 1, activeOccurrence?.end_col ?? startColumn + 1)
        : Math.max(1, activeOccurrence?.end_col ?? 1)
      : endLine === startLine
        ? Math.max(startColumn + 1, activeNodeDetails?.end_col ?? startColumn + 1)
        : Math.max(1, activeNodeDetails?.end_col ?? 1);

    decorationIdsRef.current = editor.deltaDecorations(decorationIdsRef.current, [
      {
        range: new monaco.Range(startLine, 1, startLine, 1),
        options: {
          isWholeLine: true,
          className: "monaco-focus-line",
          overviewRuler: {
            color: GRAPH_THEME.editorOverview,
            position: monaco.editor.OverviewRulerLane.Center,
          },
        },
      },
      {
        range: new monaco.Range(startLine, startColumn, endLine, endColumn),
        options: {
          className: "monaco-focus-range",
          inlineClassName: "monaco-focus-inline",
        },
      },
    ]);

    editor.revealLineInCenter(startLine);
  }, [
    activeEdgeContext,
    activeFilePath,
    activeOccurrenceIndex,
    activeOccurrences,
    activeNodeDetails?.end_col,
    activeNodeDetails?.end_line,
    activeNodeDetails?.start_col,
    activeNodeDetails?.start_line,
    draftText,
  ]);

  return {
    editorRef,
    monacoRef,
    handleEditorMount,
  };
}
