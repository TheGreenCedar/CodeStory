import { useCallback, useEffect, useRef, type Dispatch, type SetStateAction } from "react";

import { api } from "../api/client";
import type {
  AgentRetrievalProfileSelectionDto,
  AppEventPayload,
  SymbolSummaryDto,
} from "../generated/api";
import {
  defaultTrailUiConfig,
  normalizeTrailUiConfig,
  type TrailUiConfig,
} from "../graph/trailConfig";
import type { PersistedLayout } from "./types";
import {
  DEFAULT_AGENT_CONNECTION,
  DEFAULT_RETRIEVAL_PROFILE,
  LAST_OPENED_PROJECT_KEY,
  normalizeAgentConnection,
  normalizeRetrievalProfile,
  type AgentConnectionState,
} from "./layoutPersistence";

type IndexProgressState = { current: number; total: number } | null;

type UseProjectLifecycleArgs = {
  projectPath: string;
  projectOpen: boolean;
  indexProgress: IndexProgressState;
  isBusy: boolean;
  activeGraphId: string | null;
  expandedNodes: Record<string, boolean>;
  selectedTab: "agent" | "explorer";
  trailConfig: TrailUiConfig;
  agentConnection: AgentConnectionState;
  retrievalProfile: AgentRetrievalProfileSelectionDto;
  setProjectPath: Dispatch<SetStateAction<string>>;
  setStatus: Dispatch<SetStateAction<string>>;
  setProjectOpen: Dispatch<SetStateAction<boolean>>;
  setProjectRevision: Dispatch<SetStateAction<number>>;
  setTrailConfig: Dispatch<SetStateAction<TrailUiConfig>>;
  setAgentConnection: Dispatch<SetStateAction<AgentConnectionState>>;
  setRetrievalProfile: Dispatch<SetStateAction<AgentRetrievalProfileSelectionDto>>;
  setRootSymbols: Dispatch<SetStateAction<SymbolSummaryDto[]>>;
  setIndexProgress: Dispatch<SetStateAction<IndexProgressState>>;
  setIsBusy: Dispatch<SetStateAction<boolean>>;
  setActiveGraphId: Dispatch<SetStateAction<string | null>>;
  setExpandedNodes: Dispatch<SetStateAction<Record<string, boolean>>>;
  setSelectedTab: Dispatch<SetStateAction<"agent" | "explorer">>;
};

export type ProjectLifecycleActions = {
  loadRootSymbols: () => Promise<void>;
  queueAutoIncrementalIndex: () => Promise<void>;
  handleOpenProject: (pathOverride?: string, restored?: boolean) => Promise<void>;
  handleIndex: (mode: "Full" | "Incremental") => Promise<void>;
};

export function useProjectLifecycle({
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
}: UseProjectLifecycleArgs): ProjectLifecycleActions {
  const queuedAutoIndexRef = useRef<boolean>(false);
  const attemptedProjectRestoreRef = useRef<boolean>(false);

  const loadRootSymbols = useCallback(async () => {
    const roots = await api.listRootSymbols(400);
    setRootSymbols(roots);
  }, [setRootSymbols]);

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
  }, [indexProgress, projectOpen, setStatus]);

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
  }, [indexProgress, projectOpen, setStatus]);

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
        agentConnection,
        retrievalProfile,
      });
    }, 350);

    return () => clearTimeout(timer);
  }, [
    activeGraphId,
    agentConnection,
    expandedNodes,
    projectOpen,
    retrievalProfile,
    saveLayout,
    selectedTab,
    trailConfig,
  ]);

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
        case "IndexingComplete": {
          const phases = event.data.phase_timings;
          setIndexProgress(null);
          setStatus(
            `Indexing complete in ${event.data.duration_ms} ms (parse ${phases.parse_index_ms} ms, flush ${phases.projection_flush_ms} ms, resolve ${phases.edge_resolution_ms} ms, cache ${phases.cache_refresh_ms ?? 0} ms).`,
          );
          void loadRootSymbols();
          break;
        }
        case "IndexingFailed":
          setIndexProgress(null);
          setStatus(`Indexing failed: ${event.data.error}`);
          break;
        case "StatusUpdate":
          setStatus(event.data.message);
          break;
      }
    });
  }, [loadRootSymbols, setIndexProgress, setStatus]);

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
        setProjectRevision((previous) => previous + 1);
        setTrailConfig(defaultTrailUiConfig());
        setAgentConnection(DEFAULT_AGENT_CONNECTION);
        setRetrievalProfile(DEFAULT_RETRIEVAL_PROFILE);
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
            setAgentConnection(normalizeAgentConnection(parsed.agentConnection));
            setRetrievalProfile(normalizeRetrievalProfile(parsed.retrievalProfile));
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
    [
      loadRootSymbols,
      projectPath,
      setActiveGraphId,
      setAgentConnection,
      setExpandedNodes,
      setIsBusy,
      setProjectOpen,
      setProjectPath,
      setProjectRevision,
      setRetrievalProfile,
      setSelectedTab,
      setStatus,
      setTrailConfig,
    ],
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

  const handleIndex = useCallback(
    async (mode: "Full" | "Incremental") => {
      setIsBusy(true);
      try {
        await api.startIndexing({ mode });
      } catch (error) {
        setStatus(error instanceof Error ? error.message : "Failed to start indexing.");
      } finally {
        setIsBusy(false);
      }
    },
    [setIsBusy, setStatus],
  );

  return {
    loadRootSymbols,
    queueAutoIncrementalIndex,
    handleOpenProject,
    handleIndex,
  };
}
