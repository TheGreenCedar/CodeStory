import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { type Monaco, type OnMount } from "@monaco-editor/react";

import { api } from "./api/client";
import type { PendingSymbolFocus, PersistedLayout } from "./app/types";
import { CodePane } from "./components/CodePane";
import { GraphPane } from "./components/GraphPane";
import { PendingFocusDialog } from "./components/PendingFocusDialog";
import { ResponsePane } from "./components/ResponsePane";
import { StatusStrip } from "./components/StatusStrip";
import { TopBar } from "./components/TopBar";
import type {
  AgentAnswerDto,
  AppEventPayload,
  GraphArtifactDto,
  NodeDetailsDto,
  SearchHit,
  SymbolSummaryDto,
} from "./generated/api";
import { isTruncatedUmlGraph, languageForPath } from "./graph/GraphViewport";
import {
  defaultTrailUiConfig,
  normalizeTrailUiConfig,
  toTrailConfigDto,
  type TrailUiConfig,
} from "./graph/trailConfig";

function toMonacoModelPath(path: string | null): string | null {
  if (!path) {
    return null;
  }

  // Monaco URIs are POSIX-like; normalize Windows paths to avoid parser quirks.
  const forwardSlashPath = path.replace(/\\/g, "/");
  return forwardSlashPath.replace(/^([A-Za-z]:)/, "/$1");
}

const LAST_OPENED_PROJECT_KEY = "codestory:last-opened-project";

export default function App() {
  const [projectPath, setProjectPath] = useState<string>(() => {
    if (typeof window === "undefined") {
      return ".";
    }
    const saved = window.localStorage.getItem(LAST_OPENED_PROJECT_KEY)?.trim();
    return saved && saved.length > 0 ? saved : ".";
  });
  const [status, setStatus] = useState<string>("Open a project to begin.");
  const [prompt, setPrompt] = useState<string>("Trace how this feature works end-to-end.");
  const [searchQuery, setSearchQuery] = useState<string>("");
  const [includeMermaid, setIncludeMermaid] = useState<boolean>(true);
  const [selectedTab, setSelectedTab] = useState<"agent" | "explorer">("agent");
  const [agentAnswer, setAgentAnswer] = useState<AgentAnswerDto | null>(null);
  const [searchHits, setSearchHits] = useState<SearchHit[]>([]);
  const [searchOpen, setSearchOpen] = useState<boolean>(false);
  const [searchIndex, setSearchIndex] = useState<number>(0);
  const [isSearching, setIsSearching] = useState<boolean>(false);
  const [rootSymbols, setRootSymbols] = useState<SymbolSummaryDto[]>([]);
  const [childrenByNode, setChildrenByNode] = useState<Record<string, SymbolSummaryDto[]>>({});
  const [expandedNodes, setExpandedNodes] = useState<Record<string, boolean>>({});
  const [graphMap, setGraphMap] = useState<Record<string, GraphArtifactDto>>({});
  const [graphOrder, setGraphOrder] = useState<string[]>([]);
  const [activeGraphId, setActiveGraphId] = useState<string | null>(null);
  const [trailConfig, setTrailConfig] = useState<TrailUiConfig>(defaultTrailUiConfig);
  const [isTrailRunning, setIsTrailRunning] = useState<boolean>(false);
  const [activeNodeDetails, setActiveNodeDetails] = useState<NodeDetailsDto | null>(null);
  const [activeFilePath, setActiveFilePath] = useState<string | null>(null);
  const [savedText, setSavedText] = useState<string>("");
  const [draftText, setDraftText] = useState<string>("");
  const [isSaving, setIsSaving] = useState<boolean>(false);
  const [pendingFocus, setPendingFocus] = useState<PendingSymbolFocus | null>(null);
  const [indexProgress, setIndexProgress] = useState<{ current: number; total: number } | null>(
    null,
  );
  const [isBusy, setIsBusy] = useState<boolean>(false);
  const [projectOpen, setProjectOpen] = useState<boolean>(false);

  const searchSeqRef = useRef<number>(0);
  const queuedAutoIndexRef = useRef<boolean>(false);
  const saveActionRef = useRef<() => Promise<boolean>>(async () => false);
  const editorRef = useRef<Parameters<OnMount>[0] | null>(null);
  const monacoRef = useRef<Monaco | null>(null);
  const decorationIdsRef = useRef<string[]>([]);
  const attemptedProjectRestoreRef = useRef<boolean>(false);

  const isDirty = draftText !== savedText;
  const activeGraph = activeGraphId ? (graphMap[activeGraphId] ?? null) : null;
  const codeLanguage = useMemo(() => languageForPath(activeFilePath), [activeFilePath]);
  const monacoModelPath = useMemo(() => toMonacoModelPath(activeFilePath), [activeFilePath]);
  const trailDisabledReason = useMemo(() => {
    if (!projectOpen) {
      return "Open a project first.";
    }
    if (!activeNodeDetails?.id) {
      return "Select a symbol to use as trail root.";
    }
    if (trailConfig.edgeFilter.length === 0) {
      return "Select at least one edge kind.";
    }
    if (trailConfig.mode === "ToTargetSymbol" && !trailConfig.targetId) {
      return "Pick a target symbol for path search.";
    }
    return null;
  }, [
    activeNodeDetails?.id,
    projectOpen,
    trailConfig.edgeFilter.length,
    trailConfig.mode,
    trailConfig.targetId,
  ]);

  const loadRootSymbols = useCallback(async () => {
    const roots = await api.listRootSymbols(400);
    setRootSymbols(roots);
  }, []);

  const saveLayout = useCallback(
    async (layout: PersistedLayout) => {
      if (!projectOpen) {
        return;
      }

      await api.setUiLayout({
        json: JSON.stringify(layout),
      });
    },
    [projectOpen],
  );

  const queueAutoIncrementalIndex = useCallback(async () => {
    if (!projectOpen) {
      return;
    }

    if (indexProgress !== null) {
      queuedAutoIndexRef.current = true;
      setStatus("Saved. Incremental index queued after current run.");
      return;
    }

    await api.startIndexing({ mode: "Incremental" });
    setStatus("Saved. Incremental indexing started.");
  }, [indexProgress, projectOpen]);

  const saveCurrentFile = useCallback(async (): Promise<boolean> => {
    if (!activeFilePath || !projectOpen || !isDirty || isSaving) {
      return true;
    }

    setIsSaving(true);
    try {
      const response = await api.writeFileText({
        path: activeFilePath,
        text: draftText,
      });
      setSavedText(draftText);
      setStatus(`Saved ${activeFilePath} (${response.bytes_written} bytes).`);
      try {
        await queueAutoIncrementalIndex();
      } catch (error) {
        setStatus(
          error instanceof Error
            ? `Saved file, but auto-index failed: ${error.message}`
            : "Saved file, but auto-index failed.",
        );
      }
      return true;
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "Failed to save file.");
      return false;
    } finally {
      setIsSaving(false);
    }
  }, [activeFilePath, draftText, isDirty, isSaving, projectOpen, queueAutoIncrementalIndex]);

  useEffect(() => {
    saveActionRef.current = saveCurrentFile;
  }, [saveCurrentFile]);

  useEffect(() => {
    if (!projectOpen || indexProgress !== null || !queuedAutoIndexRef.current) {
      return;
    }

    queuedAutoIndexRef.current = false;
    void api
      .startIndexing({ mode: "Incremental" })
      .then(() => {
        setStatus("Queued incremental indexing started.");
      })
      .catch((error) => {
        setStatus(
          error instanceof Error
            ? `Queued incremental indexing failed: ${error.message}`
            : "Queued incremental indexing failed.",
        );
      });
  }, [indexProgress, projectOpen]);

  useEffect(() => {
    if (!projectOpen) {
      return;
    }

    const timer = setTimeout(() => {
      void saveLayout({
        activeGraphId,
        expandedNodes,
        selectedTab,
        trailConfig,
      });
    }, 350);

    return () => clearTimeout(timer);
  }, [activeGraphId, expandedNodes, projectOpen, saveLayout, selectedTab, trailConfig]);

  useEffect(() => {
    return api.subscribeEvents((event: AppEventPayload) => {
      switch (event.type) {
        case "IndexingStarted":
          setIndexProgress({ current: 0, total: event.data.file_count });
          setStatus(`Indexing started for ${event.data.file_count} file(s).`);
          break;
        case "IndexingProgress":
          setIndexProgress({ current: event.data.current, total: event.data.total });
          break;
        case "IndexingComplete":
          setIndexProgress(null);
          setStatus(`Indexing complete in ${event.data.duration_ms} ms.`);
          void loadRootSymbols();
          break;
        case "IndexingFailed":
          setIndexProgress(null);
          setStatus(`Indexing failed: ${event.data.error}`);
          break;
        case "StatusUpdate":
          setStatus(event.data.message);
          break;
      }
    });
  }, [loadRootSymbols]);

  const upsertGraph = useCallback((graph: GraphArtifactDto, activate = false) => {
    setGraphMap((prev) => ({
      ...prev,
      [graph.id]: graph,
    }));
    setGraphOrder((prev) => {
      if (prev.includes(graph.id)) {
        return prev;
      }
      return [graph.id, ...prev].slice(0, 24);
    });
    if (activate) {
      setActiveGraphId(graph.id);
    }
  }, []);

  const loadNodeContext = useCallback(async (nodeId: string) => {
    const details = await api.nodeDetails({ id: nodeId });
    setActiveNodeDetails(details);

    if (details.file_path) {
      const file = await api.readFileText({ path: details.file_path });
      setActiveFilePath(file.path);
      setSavedText(file.text);
      setDraftText(file.text);
    } else {
      setActiveFilePath(null);
      setSavedText("");
      setDraftText("");
    }
  }, []);

  const openNeighborhood = useCallback(
    async (nodeId: string, title: string) => {
      const graph = await api.graphNeighborhood({ center_id: nodeId, max_edges: 260 });
      upsertGraph(
        {
          kind: "uml",
          id: `explore-${nodeId}`,
          title: `Neighborhood: ${title}`,
          graph,
        },
        true,
      );
    },
    [upsertGraph],
  );

  const updateTrailConfig = useCallback((patch: Partial<TrailUiConfig>) => {
    setTrailConfig((prev) => ({
      ...prev,
      ...patch,
    }));
  }, []);

  const resetTrailConfig = useCallback(() => {
    setTrailConfig(defaultTrailUiConfig());
  }, []);

  const runTrail = useCallback(async () => {
    if (!activeNodeDetails?.id) {
      setStatus("Select a symbol to use as trail root.");
      return;
    }

    if (trailDisabledReason) {
      setStatus(trailDisabledReason);
      return;
    }

    setIsTrailRunning(true);
    try {
      const graph = await api.graphTrail(toTrailConfigDto(activeNodeDetails.id, trailConfig));
      const trailGraphId = `trail-${activeNodeDetails.id}-${Date.now()}`;
      const modeLabel =
        trailConfig.mode === "Neighborhood"
          ? "Neighborhood"
          : trailConfig.mode === "AllReferenced"
            ? "All Referenced"
            : trailConfig.mode === "AllReferencing"
              ? "All Referencing"
              : "To Target";

      upsertGraph(
        {
          kind: "uml",
          id: trailGraphId,
          title: `Trail: ${activeNodeDetails.display_name} (${modeLabel})`,
          graph,
        },
        true,
      );
      setStatus(`Trail loaded (${graph.nodes.length} nodes, ${graph.edges.length} edges).`);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "Failed to run trail graph query.");
    } finally {
      setIsTrailRunning(false);
    }
  }, [activeNodeDetails, trailConfig, trailDisabledReason, upsertGraph]);

  const focusSymbolInternal = useCallback(
    async (symbolId: string, label: string) => {
      const [contextResult, graphResult] = await Promise.allSettled([
        loadNodeContext(symbolId),
        openNeighborhood(symbolId, label),
      ]);

      if (contextResult.status === "rejected" && graphResult.status === "rejected") {
        setStatus("Failed to load code context and UML graph for that symbol.");
      } else if (graphResult.status === "rejected") {
        setStatus("Loaded code context, but UML graph failed to load for that symbol.");
      } else if (contextResult.status === "rejected") {
        setStatus("Loaded UML graph, but code context failed to load for that symbol.");
      }
    },
    [loadNodeContext, openNeighborhood],
  );

  const focusSymbol = useCallback(
    (symbolId: string, label: string) => {
      if (isDirty && activeFilePath && activeNodeDetails?.id !== symbolId) {
        setPendingFocus({ symbolId, label });
        return;
      }
      void focusSymbolInternal(symbolId, label);
    },
    [activeFilePath, activeNodeDetails?.id, focusSymbolInternal, isDirty],
  );

  const resolvePendingFocus = useCallback(
    async (decision: "save" | "discard" | "cancel") => {
      const pending = pendingFocus;
      if (!pending) {
        return;
      }

      if (decision === "cancel") {
        setPendingFocus(null);
        return;
      }

      if (decision === "save") {
        const saved = await saveCurrentFile();
        if (!saved) {
          return;
        }
      }

      if (decision === "discard") {
        setDraftText(savedText);
      }

      setPendingFocus(null);
      void focusSymbolInternal(pending.symbolId, pending.label);
    },
    [focusSymbolInternal, pendingFocus, saveCurrentFile, savedText],
  );

  const handleOpenProject = useCallback(
    async (pathOverride?: string, restored = false) => {
      const path = (pathOverride ?? projectPath).trim() || ".";
      setIsBusy(true);
      try {
        const summary = await api.openProject({ path });
        setProjectPath(path);
        if (typeof window !== "undefined") {
          window.localStorage.setItem(LAST_OPENED_PROJECT_KEY, path);
        }
        setStatus(restored ? `Restored project: ${summary.root}` : `Project open: ${summary.root}`);
        setProjectOpen(true);
        setTrailConfig(defaultTrailUiConfig());
        await loadRootSymbols();

        const saved = await api.getUiLayout();
        if (saved) {
          try {
            const parsed = JSON.parse(saved) as Partial<PersistedLayout>;
            if (typeof parsed.activeGraphId === "string" || parsed.activeGraphId === null) {
              setActiveGraphId(parsed.activeGraphId ?? null);
            }
            if (parsed.expandedNodes && typeof parsed.expandedNodes === "object") {
              setExpandedNodes(parsed.expandedNodes);
            }
            if (parsed.selectedTab === "agent" || parsed.selectedTab === "explorer") {
              setSelectedTab(parsed.selectedTab);
            }
            setTrailConfig(normalizeTrailUiConfig(parsed.trailConfig));
          } catch {
            // Ignore malformed saved layouts.
          }
        }
      } catch (error) {
        if (restored) {
          setStatus(
            error instanceof Error
              ? `Failed to restore ${path}: ${error.message}`
              : `Failed to restore ${path}.`,
          );
        } else {
          setStatus(error instanceof Error ? error.message : "Failed to open project.");
        }
      } finally {
        setIsBusy(false);
      }
    },
    [loadRootSymbols, projectPath],
  );

  useEffect(() => {
    if (attemptedProjectRestoreRef.current || projectOpen || isBusy) {
      return;
    }
    attemptedProjectRestoreRef.current = true;
    if (typeof window === "undefined") {
      return;
    }

    const saved = window.localStorage.getItem(LAST_OPENED_PROJECT_KEY)?.trim();
    if (!saved || saved.length === 0) {
      return;
    }
    void handleOpenProject(saved, true);
  }, [handleOpenProject, isBusy, projectOpen]);

  const handleIndex = useCallback(async (mode: "Full" | "Incremental") => {
    setIsBusy(true);
    try {
      await api.startIndexing({ mode });
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "Failed to start indexing.");
    } finally {
      setIsBusy(false);
    }
  }, []);

  const handlePrompt = useCallback(async () => {
    if (prompt.trim().length === 0) {
      return;
    }

    setIsBusy(true);
    try {
      const answer = await api.ask({
        prompt,
        include_mermaid: includeMermaid,
        focus_node_id: activeNodeDetails?.id,
        max_results: 10,
      });
      setAgentAnswer(answer);
      setStatus(answer.summary);

      const nextGraphs: Record<string, GraphArtifactDto> = {};
      const nextOrder: string[] = [];
      for (const graph of answer.graphs) {
        nextGraphs[graph.id] = graph;
        nextOrder.push(graph.id);
      }
      setGraphMap((prev) => ({ ...prev, ...nextGraphs }));
      setGraphOrder((prev) => [...nextOrder, ...prev.filter((id) => !(id in nextGraphs))]);
      if (nextOrder[0]) {
        setActiveGraphId(nextOrder[0]);
      }

      if (answer.citations[0]) {
        focusSymbol(answer.citations[0].node_id, answer.citations[0].display_name);
      }
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "Prompt execution failed.");
    } finally {
      setIsBusy(false);
    }
  }, [activeNodeDetails?.id, focusSymbol, includeMermaid, prompt]);

  const toggleNode = useCallback(
    async (node: SymbolSummaryDto) => {
      const nextExpanded = !(expandedNodes[node.id] ?? false);
      setExpandedNodes((prev) => ({
        ...prev,
        [node.id]: nextExpanded,
      }));

      if (nextExpanded && node.has_children && !childrenByNode[node.id]) {
        const children = await api.listChildrenSymbols(node.id);
        setChildrenByNode((prev) => ({
          ...prev,
          [node.id]: children,
        }));
      }
    },
    [childrenByNode, expandedNodes],
  );

  useEffect(() => {
    const query = searchQuery.trim();
    if (!projectOpen || query.length < 2) {
      searchSeqRef.current += 1;
      setIsSearching(false);
      setSearchHits([]);
      setSearchOpen(false);
      setSearchIndex(0);
      return;
    }

    const sequence = searchSeqRef.current + 1;
    searchSeqRef.current = sequence;
    setIsSearching(true);

    const timer = window.setTimeout(() => {
      void api
        .search({ query })
        .then((hits) => {
          if (sequence !== searchSeqRef.current) {
            return;
          }
          setSearchHits(hits.slice(0, 14));
          setSearchOpen(true);
          setSearchIndex(0);
        })
        .catch((error) => {
          if (sequence !== searchSeqRef.current) {
            return;
          }
          setSearchHits([]);
          setSearchOpen(false);
          setStatus(error instanceof Error ? error.message : "Search failed.");
        })
        .finally(() => {
          if (sequence === searchSeqRef.current) {
            setIsSearching(false);
          }
        });
    }, 220);

    return () => {
      window.clearTimeout(timer);
    };
  }, [projectOpen, searchQuery]);

  const activateSearchHit = useCallback(
    (hit: SearchHit) => {
      setSearchOpen(false);
      setSearchQuery(hit.display_name);
      focusSymbol(hit.node_id, hit.display_name);
    },
    [focusSymbol],
  );

  const handleSearchKeyDown = useCallback(
    (event: KeyboardEvent<HTMLInputElement>) => {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        if (searchHits.length > 0) {
          setSearchOpen(true);
          setSearchIndex((prev) => Math.min(prev + 1, searchHits.length - 1));
        }
        return;
      }

      if (event.key === "ArrowUp") {
        event.preventDefault();
        if (searchHits.length > 0) {
          setSearchOpen(true);
          setSearchIndex((prev) => Math.max(prev - 1, 0));
        }
        return;
      }

      if (event.key === "Enter") {
        if (searchHits.length > 0) {
          event.preventDefault();
          const selected =
            searchHits[Math.min(searchIndex, searchHits.length - 1)] ?? searchHits[0];
          if (selected) {
            activateSearchHit(selected);
          }
        }
        return;
      }

      if (event.key === "Escape") {
        setSearchOpen(false);
      }
    },
    [activateSearchHit, searchHits, searchIndex],
  );

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
      // This Monaco instance only loads one file model at a time, so unresolved imports are noise.
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

    if (!activeFilePath || !activeNodeDetails?.start_line) {
      decorationIdsRef.current = editor.deltaDecorations(decorationIdsRef.current, []);
      return;
    }

    const startLine = Math.max(1, activeNodeDetails.start_line ?? 1);
    const startColumn = Math.max(1, activeNodeDetails.start_col ?? 1);
    const endLine = Math.max(startLine, activeNodeDetails.end_line ?? startLine);
    const endColumn =
      endLine === startLine
        ? Math.max(startColumn + 1, activeNodeDetails.end_col ?? startColumn + 1)
        : Math.max(1, activeNodeDetails.end_col ?? 1);

    decorationIdsRef.current = editor.deltaDecorations(decorationIdsRef.current, [
      {
        range: new monaco.Range(startLine, 1, startLine, 1),
        options: {
          isWholeLine: true,
          className: "monaco-focus-line",
          overviewRuler: {
            color: "#f0b42988",
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
    activeFilePath,
    activeNodeDetails?.end_col,
    activeNodeDetails?.end_line,
    activeNodeDetails?.start_col,
    activeNodeDetails?.start_line,
    draftText,
  ]);

  return (
    <div className="app-shell">
      <TopBar
        isBusy={isBusy}
        projectOpen={projectOpen}
        projectPath={projectPath}
        onProjectPathChange={setProjectPath}
        onOpenProject={() => {
          void handleOpenProject();
        }}
        onIndex={(mode) => {
          void handleIndex(mode);
        }}
      />

      <StatusStrip status={status} indexProgress={indexProgress} />

      <main className="workspace">
        <ResponsePane
          selectedTab={selectedTab}
          onSelectTab={setSelectedTab}
          prompt={prompt}
          onPromptChange={setPrompt}
          includeMermaid={includeMermaid}
          onIncludeMermaidChange={setIncludeMermaid}
          onAskAgent={() => {
            void handlePrompt();
          }}
          isBusy={isBusy}
          projectOpen={projectOpen}
          agentAnswer={agentAnswer}
          graphMap={graphMap}
          onActivateGraph={setActiveGraphId}
          rootSymbols={rootSymbols}
          childrenByNode={childrenByNode}
          expandedNodes={expandedNodes}
          onToggleNode={toggleNode}
          onFocusSymbol={focusSymbol}
          activeSymbolId={activeNodeDetails?.id ?? null}
        />

        <GraphPane
          activeGraph={activeGraph}
          isTruncated={isTruncatedUmlGraph(activeGraph)}
          searchQuery={searchQuery}
          onSearchQueryChange={setSearchQuery}
          onSearchKeyDown={handleSearchKeyDown}
          onSearchFocus={() => {
            if (searchHits.length > 0) {
              setSearchOpen(true);
            }
          }}
          onSearchBlur={() => {
            window.setTimeout(() => setSearchOpen(false), 140);
          }}
          isSearching={isSearching}
          searchOpen={searchOpen}
          searchHits={searchHits}
          searchIndex={searchIndex}
          onSearchHitHover={setSearchIndex}
          onSearchHitActivate={activateSearchHit}
          projectOpen={projectOpen}
          graphOrder={graphOrder}
          activeGraphId={activeGraphId}
          graphMap={graphMap}
          onActivateGraph={setActiveGraphId}
          onSelectNode={(nodeId, label) => {
            focusSymbol(nodeId, label);
          }}
          trailConfig={trailConfig}
          trailRunning={isTrailRunning}
          trailDisabledReason={trailDisabledReason}
          hasActiveRoot={Boolean(activeNodeDetails?.id)}
          onTrailConfigChange={updateTrailConfig}
          onRunTrail={() => {
            void runTrail();
          }}
          onResetTrailDefaults={resetTrailConfig}
        />

        <CodePane
          projectOpen={projectOpen}
          activeFilePath={activeFilePath}
          monacoModelPath={monacoModelPath}
          isDirty={isDirty}
          isSaving={isSaving}
          onSave={saveCurrentFile}
          activeNodeDetails={activeNodeDetails}
          codeLanguage={codeLanguage}
          draftText={draftText}
          onDraftChange={setDraftText}
          onEditorMount={handleEditorMount}
        />
      </main>

      <PendingFocusDialog pendingFocus={pendingFocus} onResolve={resolvePendingFocus} />
    </div>
  );
}
