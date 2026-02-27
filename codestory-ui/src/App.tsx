import { useCallback, useMemo, useState } from "react";

import { api } from "./api/client";
import {
  DEFAULT_AGENT_CONNECTION,
  DEFAULT_RETRIEVAL_PROFILE,
  LAST_OPENED_PROJECT_KEY,
  toMonacoModelPath,
  type AgentConnectionState,
} from "./app/layoutPersistence";
import type { PendingSymbolFocus } from "./app/types";
import { useEditorDecorations } from "./app/useEditorDecorations";
import { useProjectLifecycle } from "./app/useProjectLifecycle";
import { useSearchController } from "./app/useSearchController";
import { useSymbolFocus } from "./app/useSymbolFocus";
import { useTrailActions } from "./app/useTrailActions";
import { BookmarkManager } from "./components/BookmarkManager";
import { CodePane, type CodeEdgeContext } from "./components/CodePane";
import { GraphPane } from "./components/GraphPane";
import { PendingFocusDialog } from "./components/PendingFocusDialog";
import { ResponsePane } from "./components/ResponsePane";
import { StatusStrip } from "./components/StatusStrip";
import { TopBar } from "./components/TopBar";
import type {
  AgentAnswerDto,
  AgentConnectionSettingsDto,
  GraphArtifactDto,
  AgentRetrievalProfileSelectionDto,
  NodeDetailsDto,
  SearchHit,
  SourceOccurrenceDto,
  SymbolSummaryDto,
} from "./generated/api";
import { isTruncatedUmlGraph, languageForPath } from "./graph/GraphViewport";
import { defaultTrailUiConfig, type TrailUiConfig } from "./graph/trailConfig";

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
  const [retrievalProfile, setRetrievalProfile] =
    useState<AgentRetrievalProfileSelectionDto>(DEFAULT_RETRIEVAL_PROFILE);
  const [agentConnection, setAgentConnection] =
    useState<AgentConnectionState>(DEFAULT_AGENT_CONNECTION);
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
  const [activeEdgeContext, setActiveEdgeContext] = useState<CodeEdgeContext | null>(null);
  const [activeOccurrences, setActiveOccurrences] = useState<SourceOccurrenceDto[]>([]);
  const [activeOccurrenceIndex, setActiveOccurrenceIndex] = useState<number>(0);
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
  const [projectRevision, setProjectRevision] = useState<number>(0);
  const [bookmarkManagerOpen, setBookmarkManagerOpen] = useState<boolean>(false);
  const [bookmarkSeed, setBookmarkSeed] = useState<{ nodeId: string; label: string } | null>(null);

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

  const { queueAutoIncrementalIndex, handleOpenProject, handleIndex } = useProjectLifecycle({
    projectPath,
    projectOpen,
    indexProgress,
    isBusy,
    activeGraphId,
    expandedNodes,
    selectedTab,
    trailConfig,
    agentConnection,
    retrievalProfile,
    setProjectPath,
    setStatus,
    setProjectOpen,
    setProjectRevision,
    setTrailConfig,
    setAgentConnection,
    setRetrievalProfile,
    setRootSymbols,
    setIndexProgress,
    setIsBusy,
    setActiveGraphId,
    setExpandedNodes,
    setSelectedTab,
  });

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

  const { updateTrailConfig, resetTrailConfig, openTrailGraph, runTrail } = useTrailActions({
    activeNodeDetails,
    setIsTrailRunning,
    setStatus,
    trailConfig,
    trailDisabledReason,
    setTrailConfig,
    upsertGraph,
  });

  const {
    openNeighborhoodInNewTab,
    selectOccurrenceByIndex,
    selectNextOccurrence,
    selectPreviousOccurrence,
    selectEdge,
    focusSymbolInternal,
    focusSymbol,
    resolvePendingFocus,
  } = useSymbolFocus({
    activeFilePath,
    activeNodeId: activeNodeDetails?.id ?? null,
    isDirty,
    pendingFocus,
    savedText,
    activeOccurrences,
    activeOccurrenceIndex,
    trailConfig,
    projectOpen,
    setPendingFocus,
    setStatus,
    setActiveNodeDetails,
    setActiveEdgeContext,
    setActiveOccurrences,
    setActiveOccurrenceIndex,
    setActiveFilePath,
    setSavedText,
    setDraftText,
    setIsTrailRunning,
    saveCurrentFile,
    upsertGraph,
    openTrailGraph,
  });

  const navigateGraphBack = useCallback(() => {
    if (!activeGraphId) {
      return;
    }
    const index = graphOrder.indexOf(activeGraphId);
    if (index < 0) {
      return;
    }
    const next = graphOrder[index + 1];
    if (next) {
      setActiveGraphId(next);
    }
  }, [activeGraphId, graphOrder]);

  const navigateGraphForward = useCallback(() => {
    if (!activeGraphId) {
      return;
    }
    const index = graphOrder.indexOf(activeGraphId);
    if (index <= 0) {
      return;
    }
    const next = graphOrder[index - 1];
    if (next) {
      setActiveGraphId(next);
    }
  }, [activeGraphId, graphOrder]);

  const showDefinitionInIde = useCallback(async (nodeId: string) => {
    try {
      const response = await api.openDefinition({ node_id: nodeId });
      setStatus(response.message);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "Failed to open definition in IDE.");
    }
  }, []);

  const openContainingFolder = useCallback(async (path: string) => {
    try {
      const response = await api.openContainingFolder({ path });
      setStatus(response.message);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "Failed to open containing folder.");
    }
  }, []);
  const handlePrompt = useCallback(async () => {
    if (prompt.trim().length === 0) {
      return;
    }

    const command = agentConnection.command?.trim();
    const connection: AgentConnectionSettingsDto = {
      backend: agentConnection.backend,
      command: command && command.length > 0 ? command : null,
    };

    setIsBusy(true);
    try {
      const answer = await api.ask({
        prompt,
        retrieval_profile: retrievalProfile,
        focus_node_id: activeNodeDetails?.id,
        max_results: 10,
        connection,
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
  }, [activeNodeDetails?.id, agentConnection, focusSymbol, prompt, retrievalProfile]);

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

  const { activateSearchHit, handleSearchKeyDown } = useSearchController({
    projectOpen,
    searchQuery,
    searchHits,
    searchIndex,
    isDirty,
    activeFilePath,
    activeNodeId: activeNodeDetails?.id ?? null,
    focusSymbolInternal,
    updateTrailConfig,
    setSearchHits,
    setSearchOpen,
    setSearchIndex,
    setIsSearching,
    setPendingFocus,
    setStatus,
    setSearchQuery,
  });

  const { handleEditorMount } = useEditorDecorations({
    saveCurrentFile,
    activeFilePath,
    activeEdgeContext,
    activeOccurrences,
    activeOccurrenceIndex,
    activeNodeDetails,
    draftText,
  });

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
          retrievalProfile={retrievalProfile}
          onRetrievalProfileChange={setRetrievalProfile}
          agentBackend={agentConnection.backend}
          onAgentBackendChange={(backend) => {
            setAgentConnection((prev) => ({
              ...prev,
              backend,
            }));
          }}
          agentCommand={agentConnection.command ?? ""}
          onAgentCommandChange={(command) => {
            setAgentConnection((prev) => ({
              ...prev,
              command,
            }));
          }}
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
          projectRevision={projectRevision}
          graphOrder={graphOrder}
          activeGraphId={activeGraphId}
          graphMap={graphMap}
          onActivateGraph={setActiveGraphId}
          onSelectNode={(nodeId, label) => {
            focusSymbol(nodeId, label);
          }}
          onSelectEdge={(selection) => {
            void selectEdge(selection);
          }}
          trailConfig={trailConfig}
          trailRunning={isTrailRunning}
          trailDisabledReason={trailDisabledReason}
          hasActiveRoot={Boolean(activeNodeDetails?.id)}
          activeRootLabel={activeNodeDetails?.display_name ?? null}
          onOpenNodeInNewTab={(nodeId, label) => {
            void openNeighborhoodInNewTab(nodeId, label);
          }}
          onNavigateBack={navigateGraphBack}
          onNavigateForward={navigateGraphForward}
          onShowDefinitionInIde={(nodeId) => {
            void showDefinitionInIde(nodeId);
          }}
          onBookmarkNode={(nodeId, label) => {
            setBookmarkSeed({ nodeId, label });
            setBookmarkManagerOpen(true);
          }}
          onOpenContainingFolder={(path) => {
            void openContainingFolder(path);
          }}
          onOpenBookmarkManager={() => {
            if (activeNodeDetails?.id) {
              setBookmarkSeed({
                nodeId: activeNodeDetails.id,
                label: activeNodeDetails.display_name,
              });
            }
            setBookmarkManagerOpen(true);
          }}
          onGraphStatusMessage={setStatus}
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
          activeEdgeContext={activeEdgeContext}
          occurrences={activeOccurrences}
          activeOccurrenceIndex={activeOccurrenceIndex}
          onSelectOccurrence={(index) => {
            void selectOccurrenceByIndex(index);
          }}
          onNextOccurrence={() => {
            void selectNextOccurrence();
          }}
          onPreviousOccurrence={() => {
            void selectPreviousOccurrence();
          }}
          codeLanguage={codeLanguage}
          draftText={draftText}
          onDraftChange={setDraftText}
          onEditorMount={handleEditorMount}
        />
      </main>

      <BookmarkManager
        open={bookmarkManagerOpen}
        seed={bookmarkSeed}
        onClose={() => setBookmarkManagerOpen(false)}
        onFocusSymbol={(nodeId, label) => {
          setBookmarkManagerOpen(false);
          focusSymbol(nodeId, label);
        }}
        onStatus={setStatus}
      />

      <PendingFocusDialog pendingFocus={pendingFocus} onResolve={resolvePendingFocus} />
    </div>
  );
}
