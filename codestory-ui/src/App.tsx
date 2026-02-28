import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import { api } from "./api/client";
import {
  DEFAULT_AGENT_CONNECTION,
  DEFAULT_RETRIEVAL_PROFILE,
  LAST_OPENED_PROJECT_KEY,
  toMonacoModelPath,
  type AgentConnectionState,
} from "./app/layoutPersistence";
import { loadFeatureFlags, saveFeatureFlags, type FeatureFlagState } from "./app/featureFlags";
import { trackAnalyticsEvent } from "./app/analytics";
import { UI_CONTRACT, UI_LAYOUT_SCHEMA_STORAGE_KEY } from "./app/uiContract";
import type { PendingSymbolFocus } from "./app/types";
import { useEditorDecorations } from "./app/useEditorDecorations";
import { useProjectLifecycle } from "./app/useProjectLifecycle";
import { useSearchController } from "./app/useSearchController";
import { useSymbolFocus } from "./app/useSymbolFocus";
import { useTrailActions } from "./app/useTrailActions";
import { BookmarkManager } from "./components/BookmarkManager";
import { CodePane, type CodeEdgeContext } from "./components/CodePane";
import { CommandPalette, type CommandPaletteCommand } from "./components/CommandPalette";
import { GraphPane } from "./components/GraphPane";
import { InvestigateFocusSwitcher } from "./components/InvestigateFocusSwitcher";
import { PendingFocusDialog } from "./components/PendingFocusDialog";
import { ResponsePane } from "./components/ResponsePane";
import { StatusStrip } from "./components/StatusStrip";
import { TopBar } from "./components/TopBar";
import { StarterCard } from "./features/onboarding/StarterCard";
import { SettingsPage } from "./features/settings/SettingsPage";
import { SpacesPanel } from "./features/spaces/SpacesPanel";
import {
  createSpace,
  deleteSpace,
  listSpaces,
  loadSpace,
  updateSpace,
  type InvestigationSpace,
} from "./features/spaces";
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
import { AppShell, type AppShellSection } from "./layout/AppShell";
import {
  INVESTIGATE_FOCUS_MODE_KEY,
  LEGACY_WORKSPACE_LAYOUT_PRESET_KEY,
  migrateLegacyWorkspacePreset,
  normalizeInvestigateFocusMode,
  investigateFocusModeLabel,
  type InvestigateFocusMode,
} from "./layout/layoutPresets";

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
  const [activeSection, setActiveSection] = useState<AppShellSection>("investigate");
  const [commandPaletteOpen, setCommandPaletteOpen] = useState<boolean>(false);
  const [featureFlags, setFeatureFlags] = useState<FeatureFlagState>(() => loadFeatureFlags());
  const [investigateMode, setInvestigateMode] = useState<InvestigateFocusMode>(() => {
    if (typeof window === "undefined") {
      return "graph";
    }
    const current = window.localStorage.getItem(INVESTIGATE_FOCUS_MODE_KEY);
    if (current) {
      return normalizeInvestigateFocusMode(current);
    }
    const legacy = migrateLegacyWorkspacePreset(
      window.localStorage.getItem(LEGACY_WORKSPACE_LAYOUT_PRESET_KEY),
    );
    return legacy ?? "graph";
  });
  const [hasCompletedIndex, setHasCompletedIndex] = useState<boolean>(false);
  const [askedFirstQuestion, setAskedFirstQuestion] = useState<boolean>(false);
  const [inspectedSource, setInspectedSource] = useState<boolean>(false);
  const [spaces, setSpaces] = useState<InvestigationSpace[]>(() => listSpaces());
  const [activeSpaceId, setActiveSpaceId] = useState<string | null>(null);

  const firstAskTrackedRef = useRef<boolean>(false);
  const firstNodeSelectTrackedRef = useRef<boolean>(false);
  const firstSaveTrackedRef = useRef<boolean>(false);
  const firstTrailTrackedRef = useRef<boolean>(false);
  const previousIndexProgressRef = useRef<{ current: number; total: number } | null>(null);

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

  useEffect(() => {
    window.localStorage.setItem(INVESTIGATE_FOCUS_MODE_KEY, investigateMode);
    window.localStorage.setItem(UI_LAYOUT_SCHEMA_STORAGE_KEY, String(UI_CONTRACT.schemaVersion));
  }, [investigateMode]);

  useEffect(() => {
    saveFeatureFlags(featureFlags);
  }, [featureFlags]);

  useEffect(() => {
    const previous = previousIndexProgressRef.current;
    if (previous !== null && indexProgress === null) {
      setHasCompletedIndex(true);
    }
    previousIndexProgressRef.current = indexProgress;
  }, [indexProgress]);

  useEffect(() => {
    if (activeFilePath && projectOpen) {
      setInspectedSource(true);
    }
  }, [activeFilePath, projectOpen]);

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
      const isFirstSave = !firstSaveTrackedRef.current;
      if (isFirstSave) {
        firstSaveTrackedRef.current = true;
      }
      trackAnalyticsEvent(
        "file_saved",
        {
          file_path: activeFilePath,
          bytes_written: response.bytes_written,
          is_first: isFirstSave,
        },
        {
          projectPath,
        },
      );
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
  }, [
    activeFilePath,
    draftText,
    isDirty,
    isSaving,
    projectOpen,
    projectPath,
    queueAutoIncrementalIndex,
  ]);

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
    const trimmedPrompt = prompt.trim();
    if (trimmedPrompt.length === 0) {
      return;
    }

    const isFirstAsk = !firstAskTrackedRef.current;
    if (isFirstAsk) {
      firstAskTrackedRef.current = true;
    }
    setAskedFirstQuestion(true);
    trackAnalyticsEvent(
      "ask_submitted",
      {
        prompt_length: trimmedPrompt.length,
        tab: selectedTab,
        is_first: isFirstAsk,
      },
      {
        projectPath,
      },
    );

    const command = agentConnection.command?.trim();
    const connection: AgentConnectionSettingsDto = {
      backend: agentConnection.backend,
      command: command && command.length > 0 ? command : null,
    };

    setIsBusy(true);
    try {
      const answer = await api.ask({
        prompt: trimmedPrompt,
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
  }, [
    activeNodeDetails?.id,
    agentConnection,
    focusSymbol,
    projectPath,
    prompt,
    retrievalProfile,
    selectedTab,
  ]);

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

  const focusSymbolFromUi = useCallback(
    (symbolId: string, label: string, source: "graph" | "explorer" | "bookmark") => {
      const isFirstNodeSelection = !firstNodeSelectTrackedRef.current;
      if (isFirstNodeSelection) {
        firstNodeSelectTrackedRef.current = true;
      }

      trackAnalyticsEvent(
        "node_selected",
        {
          node_id: symbolId,
          source,
          is_first: isFirstNodeSelection,
        },
        {
          projectPath,
        },
      );

      focusSymbol(symbolId, label);
    },
    [focusSymbol, projectPath],
  );

  const handleRunTrailWithAnalytics = useCallback(async () => {
    const shouldTrack = Boolean(activeNodeDetails?.id) && !trailDisabledReason;
    await runTrail();
    if (!shouldTrack) {
      return;
    }

    const isFirstTrailRun = !firstTrailTrackedRef.current;
    if (isFirstTrailRun) {
      firstTrailTrackedRef.current = true;
    }

    trackAnalyticsEvent(
      "trail_run",
      {
        mode: trailConfig.mode,
        edge_filter_count: trailConfig.edgeFilter.length,
        has_target_symbol: Boolean(trailConfig.targetId),
        is_first: isFirstTrailRun,
      },
      {
        projectPath,
      },
    );
  }, [activeNodeDetails?.id, projectPath, runTrail, trailConfig, trailDisabledReason]);

  const openProjectFromUi = useCallback(async () => {
    setHasCompletedIndex(false);
    setAskedFirstQuestion(false);
    setInspectedSource(false);
    await handleOpenProject();
    setActiveSection("investigate");
    setInvestigateMode("graph");
  }, [handleOpenProject]);

  const runIndexFromUi = useCallback(
    async (mode: "Full" | "Incremental") => {
      await handleIndex(mode);
      setActiveSection("investigate");
      setInvestigateMode("graph");
    },
    [handleIndex],
  );

  const runRecommendedIndex = useCallback(async () => {
    await runIndexFromUi("Incremental");
  }, [runIndexFromUi]);

  const seedFirstQuestion = useCallback(async () => {
    setPrompt("Give me a quick architecture walkthrough of this repository.");
    setActiveSection("investigate");
    setInvestigateMode("ask");
  }, []);

  const jumpToSourceInspection = useCallback(async () => {
    setActiveSection("investigate");
    setInvestigateMode("code");
    if (activeNodeDetails?.id) {
      focusSymbol(activeNodeDetails.id, activeNodeDetails.display_name);
      return;
    }
    const firstRoot = rootSymbols[0];
    if (firstRoot) {
      focusSymbol(firstRoot.id, firstRoot.label);
    }
  }, [activeNodeDetails?.display_name, activeNodeDetails?.id, focusSymbol, rootSymbols]);

  const focusGraphSearchInput = useCallback(() => {
    const searchInput = document.querySelector<HTMLInputElement>(".graph-search-input");
    searchInput?.focus();
    setInvestigateMode("graph");
    setActiveSection("investigate");
  }, []);

  const setAgentBackend = useCallback((backend: AgentConnectionState["backend"]) => {
    setAgentConnection((previous) => ({
      ...previous,
      backend,
    }));
  }, []);

  const updateFeatureFlag = useCallback((flag: keyof FeatureFlagState, value: boolean) => {
    setFeatureFlags((previous) => ({
      ...previous,
      [flag]: value,
    }));
  }, []);

  const refreshSpaces = useCallback(() => {
    setSpaces(listSpaces());
  }, []);

  const createSpaceFromCurrentContext = useCallback(
    (name: string, notes: string) => {
      const created = createSpace({
        name: name.trim().length > 0 ? name : `Investigation ${new Date().toLocaleString()}`,
        prompt: prompt.trim().length > 0 ? prompt : "Untitled investigation prompt",
        activeGraphId,
        activeSymbolId: activeNodeDetails?.id ?? null,
        notes,
        owner: "local-user",
      });
      setActiveSpaceId(created.id);
      refreshSpaces();
      setStatus(`Saved space "${created.name}".`);
    },
    [activeGraphId, activeNodeDetails?.id, prompt, refreshSpaces],
  );

  const loadSpaceIntoWorkspace = useCallback(
    (spaceId: string) => {
      const space = loadSpace(spaceId);
      if (!space) {
        setStatus("Requested space was not found.");
        return;
      }
      setPrompt(space.prompt);
      if (space.activeGraphId && graphMap[space.activeGraphId]) {
        setActiveGraphId(space.activeGraphId);
      }
      if (space.activeSymbolId) {
        focusSymbol(space.activeSymbolId, space.activeSymbolId);
      }
      setActiveSpaceId(space.id);
      setActiveSection("investigate");
      setStatus(`Loaded space "${space.name}".`);
      trackAnalyticsEvent(
        "library_space_reopened",
        {
          space_id: space.id,
        },
        {
          projectPath,
        },
      );
      updateSpace(space.id, { notes: space.notes ?? "" });
      refreshSpaces();
    },
    [focusSymbol, graphMap, projectPath, refreshSpaces],
  );

  const removeSpaceById = useCallback(
    (spaceId: string) => {
      const removed = deleteSpace(spaceId);
      if (!removed) {
        setStatus("Space was already removed.");
        return;
      }
      if (activeSpaceId === spaceId) {
        setActiveSpaceId(null);
      }
      refreshSpaces();
      setStatus("Deleted saved space.");
    },
    [activeSpaceId, refreshSpaces],
  );

  const invokeCommand = useCallback(
    (commandId: string, run: () => void | Promise<void>) => {
      trackAnalyticsEvent(
        "command_invoked",
        {
          command_id: commandId,
        },
        {
          projectPath,
        },
      );
      return run();
    },
    [projectPath],
  );

  const handleInvestigateModeChange = useCallback(
    (mode: InvestigateFocusMode) => {
      if (mode === investigateMode) {
        return;
      }
      trackAnalyticsEvent(
        "investigate_mode_switched",
        {
          from_mode: investigateMode,
          to_mode: mode,
        },
        {
          projectPath,
        },
      );
      setInvestigateMode(mode);
      setActiveSection("investigate");
      setStatus(`Focus mode set to ${investigateFocusModeLabel(mode)}.`);
    },
    [investigateMode, projectPath],
  );

  const commandPaletteCommands = useMemo<CommandPaletteCommand[]>(
    () => [
      {
        id: "open-project",
        label: "Open Project",
        detail: "Open the path currently in the top bar",
        keywords: ["project", "setup", "workspace"],
        disabled: isBusy,
        run: () => invokeCommand("open-project", openProjectFromUi),
      },
      {
        id: "index-incremental",
        label: "Run Incremental Index",
        detail: "Refresh changed files only",
        keywords: ["index", "incremental", "refresh"],
        disabled: isBusy || !projectOpen,
        run: () => invokeCommand("index-incremental", () => runIndexFromUi("Incremental")),
      },
      {
        id: "index-full",
        label: "Run Full Index",
        detail: "Rebuild graph and symbol index",
        keywords: ["index", "full", "rebuild"],
        disabled: isBusy || !projectOpen,
        run: () => invokeCommand("index-full", () => runIndexFromUi("Full")),
      },
      {
        id: "ask-agent",
        label: "Ask Agent",
        detail: "Submit the active prompt",
        keywords: ["ask", "agent", "prompt"],
        disabled: isBusy || prompt.trim().length === 0,
        run: () =>
          invokeCommand("ask-agent", () => {
            setInvestigateMode("ask");
            void handlePrompt();
          }),
      },
      {
        id: "focus-graph-search",
        label: "Focus Graph Search",
        detail: "Jump to graph search input",
        keywords: ["graph", "search", "find"],
        run: () =>
          invokeCommand("focus-graph-search", () => {
            setInvestigateMode("graph");
            focusGraphSearchInput();
          }),
      },
      {
        id: "run-trail",
        label: "Run Trail Query",
        detail: "Execute trail from selected root symbol",
        keywords: ["trail", "graph", "path"],
        disabled: Boolean(trailDisabledReason),
        run: () =>
          invokeCommand("run-trail", () => {
            setInvestigateMode("graph");
            void handleRunTrailWithAnalytics();
          }),
      },
      {
        id: "focus-ask",
        label: "Focus Ask Mode",
        detail: "Show only the Ask pane",
        keywords: ["ask", "focus", "mode"],
        disabled: investigateMode === "ask",
        run: () =>
          invokeCommand("focus-ask", () => {
            handleInvestigateModeChange("ask");
          }),
      },
      {
        id: "focus-graph",
        label: "Focus Graph Mode",
        detail: "Show only the Graph pane",
        keywords: ["graph", "focus", "mode"],
        disabled: investigateMode === "graph",
        run: () =>
          invokeCommand("focus-graph", () => {
            handleInvestigateModeChange("graph");
          }),
      },
      {
        id: "focus-code",
        label: "Focus Code Mode",
        detail: "Show only the Code pane",
        keywords: ["code", "focus", "mode"],
        disabled: investigateMode === "code",
        run: () =>
          invokeCommand("focus-code", () => {
            handleInvestigateModeChange("code");
          }),
      },
      {
        id: "open-bookmarks",
        label: "Open Bookmark Manager",
        detail: "Browse and manage saved symbols",
        keywords: ["bookmark", "library", "saved"],
        run: () =>
          invokeCommand("open-bookmarks", () => {
            setBookmarkManagerOpen(true);
            setActiveSection("investigate");
          }),
      },
      {
        id: "goto-investigate",
        label: "Open Investigate Section",
        detail: "Navigate shell to Investigate",
        keywords: ["investigate", "workspace", "section"],
        disabled: activeSection === "investigate",
        run: () =>
          invokeCommand("goto-investigate", () => {
            setActiveSection("investigate");
          }),
      },
      {
        id: "goto-library",
        label: "Open Library Section",
        detail: "Navigate shell to Library",
        keywords: ["library", "spaces", "section"],
        disabled: activeSection === "library",
        run: () =>
          invokeCommand("goto-library", () => {
            setActiveSection("library");
          }),
      },
      {
        id: "goto-settings",
        label: "Open Settings Section",
        detail: "Navigate shell to Settings",
        keywords: ["settings", "preferences", "section"],
        disabled: activeSection === "settings",
        run: () =>
          invokeCommand("goto-settings", () => {
            setActiveSection("settings");
          }),
      },
      {
        id: "save-space",
        label: "Save Investigation Space",
        detail: "Store current prompt and focus for reuse",
        keywords: ["space", "save", "library"],
        disabled: !featureFlags.spacesLibrary,
        run: () =>
          invokeCommand("save-space", () => {
            createSpaceFromCurrentContext("", "");
            setActiveSection("library");
          }),
      },
      {
        id: "toggle-ux-reset",
        label: featureFlags.uxResetV2 ? "Disable UX Reset" : "Enable UX Reset",
        detail: "Rollback switch for staged rollout",
        keywords: ["feature flag", "rollback", "shell"],
        run: () =>
          invokeCommand("toggle-ux-reset", () => {
            updateFeatureFlag("uxResetV2", !featureFlags.uxResetV2);
          }),
      },
    ],
    [
      activeSection,
      createSpaceFromCurrentContext,
      featureFlags.spacesLibrary,
      featureFlags.uxResetV2,
      focusGraphSearchInput,
      handleInvestigateModeChange,
      handlePrompt,
      handleRunTrailWithAnalytics,
      investigateMode,
      invokeCommand,
      isBusy,
      openProjectFromUi,
      projectOpen,
      prompt,
      runIndexFromUi,
      trailDisabledReason,
      updateFeatureFlag,
    ],
  );

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setCommandPaletteOpen((previous) => !previous);
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, []);

  const responsePaneView = (
    <ResponsePane
      selectedTab={selectedTab}
      onSelectTab={setSelectedTab}
      prompt={prompt}
      onPromptChange={setPrompt}
      retrievalProfile={retrievalProfile}
      onRetrievalProfileChange={setRetrievalProfile}
      agentBackend={agentConnection.backend}
      onAgentBackendChange={setAgentBackend}
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
      onFocusSymbol={(symbolId, label) => {
        focusSymbolFromUi(symbolId, label, "explorer");
      }}
      activeSymbolId={activeNodeDetails?.id ?? null}
    />
  );

  const graphPaneView = (
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
        focusSymbolFromUi(nodeId, label, "graph");
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
        void handleRunTrailWithAnalytics();
      }}
      onResetTrailDefaults={resetTrailConfig}
    />
  );

  const codePaneView = (
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
  );

  const legacyWorkspaceView = (
    <div className="workspace">
      {responsePaneView}
      {graphPaneView}
      {codePaneView}
    </div>
  );

  const focusedPane =
    investigateMode === "graph"
      ? graphPaneView
      : investigateMode === "code"
        ? codePaneView
        : responsePaneView;

  const focusedWorkspaceView = (
    <div className="investigate-layout">
      {featureFlags.onboardingStarter ? (
        <StarterCard
          className="starter-card"
          projectPath={projectPath}
          projectOpen={projectOpen}
          indexComplete={hasCompletedIndex || rootSymbols.length > 0}
          askedFirstQuestion={askedFirstQuestion}
          inspectedSource={inspectedSource}
          onOpenProject={openProjectFromUi}
          onRunIndex={runRecommendedIndex}
          onSeedQuestion={seedFirstQuestion}
          onInspectSource={jumpToSourceInspection}
          onPrimaryAction={(action) => {
            trackAnalyticsEvent(
              "starter_card_cta_clicked",
              {
                action,
              },
              {
                projectPath,
              },
            );
          }}
        />
      ) : null}

      <div className="investigate-toolbar">
        <InvestigateFocusSwitcher
          mode={investigateMode}
          onModeChange={handleInvestigateModeChange}
        />
        <div className="investigate-toolbar-actions">
          <button
            type="button"
            onClick={() => {
              createSpaceFromCurrentContext("", "");
              setActiveSection("library");
            }}
            disabled={!featureFlags.spacesLibrary}
          >
            Save Space
          </button>
          <button
            type="button"
            onClick={() => {
              setBookmarkManagerOpen(true);
            }}
          >
            Bookmarks
          </button>
        </div>
      </div>

      <div className="investigate-pane">{focusedPane}</div>
    </div>
  );

  const workspaceView = featureFlags.singlePaneInvestigate
    ? focusedWorkspaceView
    : legacyWorkspaceView;

  const sectionContent: Partial<Record<AppShellSection, ReactNode>> = {
    library: featureFlags.spacesLibrary ? (
      <SpacesPanel
        spaces={spaces}
        activeSpaceId={activeSpaceId}
        onCreateSpace={createSpaceFromCurrentContext}
        onLoadSpace={loadSpaceIntoWorkspace}
        onDeleteSpace={removeSpaceById}
      />
    ) : (
      <section className="shell-card">
        <h3>Spaces Disabled</h3>
        <p>Enable spaces in Settings to save and reopen investigations.</p>
      </section>
    ),
    settings: <SettingsPage featureFlags={featureFlags} onUpdateFlag={updateFeatureFlag} />,
  };

  return (
    <div className="app-shell">
      <TopBar
        isBusy={isBusy}
        projectOpen={projectOpen}
        projectPath={projectPath}
        onProjectPathChange={setProjectPath}
        onOpenProject={() => {
          void openProjectFromUi();
        }}
        onIndex={(mode) => {
          void runIndexFromUi(mode);
        }}
      />

      <StatusStrip status={status} indexProgress={indexProgress} />

      {featureFlags.uxResetV2 ? (
        <AppShell
          activeSection={activeSection}
          onSelectSection={setActiveSection}
          workspace={workspaceView}
          sectionContent={sectionContent}
        />
      ) : (
        legacyWorkspaceView
      )}

      <BookmarkManager
        open={bookmarkManagerOpen}
        seed={bookmarkSeed}
        onClose={() => setBookmarkManagerOpen(false)}
        onFocusSymbol={(nodeId, label) => {
          setBookmarkManagerOpen(false);
          focusSymbolFromUi(nodeId, label, "bookmark");
        }}
        onStatus={setStatus}
        onPromoteBookmarkToSpace={(bookmark) => {
          createSpaceFromCurrentContext(
            `Bookmark - ${bookmark.node_label}`,
            bookmark.comment ?? "",
          );
          setStatus(`Promoted "${bookmark.node_label}" to a space.`);
          setActiveSection("library");
        }}
      />

      <PendingFocusDialog pendingFocus={pendingFocus} onResolve={resolvePendingFocus} />
      <CommandPalette
        open={commandPaletteOpen}
        commands={commandPaletteCommands}
        onClose={() => setCommandPaletteOpen(false)}
      />
    </div>
  );
}
