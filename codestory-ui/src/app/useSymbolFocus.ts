import { useCallback, type Dispatch, type SetStateAction } from "react";

import { api } from "../api/client";
import type { PendingSymbolFocus } from "./types";
import type { CodeEdgeContext } from "../components/CodePane";
import type { GraphArtifactDto, NodeDetailsDto, SourceOccurrenceDto } from "../generated/api";
import type { TrailUiConfig } from "../graph/trailConfig";
import type { FocusGraphMode } from "./layoutPersistence";

type UseSymbolFocusArgs = {
  activeFilePath: string | null;
  activeNodeId: string | null;
  isDirty: boolean;
  pendingFocus: PendingSymbolFocus | null;
  savedText: string;
  activeOccurrences: SourceOccurrenceDto[];
  activeOccurrenceIndex: number;
  trailConfig: TrailUiConfig;
  projectOpen: boolean;
  setPendingFocus: Dispatch<SetStateAction<PendingSymbolFocus | null>>;
  setStatus: Dispatch<SetStateAction<string>>;
  setActiveNodeDetails: Dispatch<SetStateAction<NodeDetailsDto | null>>;
  setActiveEdgeContext: Dispatch<SetStateAction<CodeEdgeContext | null>>;
  setActiveOccurrences: Dispatch<SetStateAction<SourceOccurrenceDto[]>>;
  setActiveOccurrenceIndex: Dispatch<SetStateAction<number>>;
  setActiveFilePath: Dispatch<SetStateAction<string | null>>;
  setSavedText: Dispatch<SetStateAction<string>>;
  setDraftText: Dispatch<SetStateAction<string>>;
  setIsTrailRunning: Dispatch<SetStateAction<boolean>>;
  saveCurrentFile: () => Promise<boolean>;
  upsertGraph: (graph: GraphArtifactDto, activate?: boolean) => void;
  openTrailGraph: (
    rootId: string,
    rootLabel: string,
    rootFilePath: string | null,
    config: TrailUiConfig,
  ) => Promise<void>;
};

export type SymbolFocusActions = {
  openNeighborhoodInNewTab: (nodeId: string, title: string) => Promise<void>;
  selectOccurrenceByIndex: (index: number) => Promise<void>;
  selectNextOccurrence: () => Promise<void>;
  selectPreviousOccurrence: () => Promise<void>;
  selectEdge: (selection: {
    id: string;
    edgeIds: string[];
    kind: string;
    sourceNodeId: string;
    targetNodeId: string;
    sourceLabel: string;
    targetLabel: string;
  }) => Promise<void>;
  focusSymbolInternal: (
    symbolId: string,
    label: string,
    graphMode?: FocusGraphMode,
  ) => Promise<void>;
  focusSymbol: (symbolId: string, label: string) => void;
  resolvePendingFocus: (decision: "save" | "discard" | "cancel") => Promise<void>;
};

export function useSymbolFocus({
  activeFilePath,
  activeNodeId,
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
}: UseSymbolFocusArgs): SymbolFocusActions {
  const openOccurrenceFile = useCallback(
    async (
      occurrence: SourceOccurrenceDto,
      options?: {
        allowDirtyCrossFile?: boolean;
      },
    ): Promise<boolean> => {
      if (
        !options?.allowDirtyCrossFile &&
        isDirty &&
        activeFilePath &&
        activeFilePath !== occurrence.file_path
      ) {
        setStatus("Save or discard changes before jumping to a different source file.");
        return false;
      }

      const file = await api.readFileText({ path: occurrence.file_path });
      setActiveFilePath(file.path);
      setSavedText(file.text);
      setDraftText(file.text);
      return true;
    },
    [activeFilePath, isDirty, setActiveFilePath, setDraftText, setSavedText, setStatus],
  );

  const loadNodeContext = useCallback(
    async (nodeId: string) => {
      const details = await api.nodeDetails({ id: nodeId });
      setActiveNodeDetails(details);
      setActiveEdgeContext(null);

      const occurrences = await api.nodeOccurrences({ id: nodeId });
      setActiveOccurrences(occurrences);

      if (occurrences.length > 0) {
        const preferredIndex = occurrences.findIndex((occurrence) => {
          if (!details.file_path || !details.start_line) {
            return false;
          }
          return (
            occurrence.file_path === details.file_path &&
            occurrence.start_line === details.start_line
          );
        });
        const nextIndex = preferredIndex >= 0 ? preferredIndex : 0;
        setActiveOccurrenceIndex(nextIndex);
        const nextOccurrence = occurrences[nextIndex];
        if (nextOccurrence) {
          const opened = await openOccurrenceFile(nextOccurrence, { allowDirtyCrossFile: true });
          if (opened) {
            return details;
          }
        }
      } else {
        setActiveOccurrenceIndex(0);
      }

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

      return details;
    },
    [
      openOccurrenceFile,
      setActiveEdgeContext,
      setActiveFilePath,
      setActiveNodeDetails,
      setActiveOccurrenceIndex,
      setActiveOccurrences,
      setDraftText,
      setSavedText,
    ],
  );

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

  const openNeighborhoodInNewTab = useCallback(
    async (nodeId: string, title: string) => {
      try {
        const graph = await api.graphNeighborhood({ center_id: nodeId, max_edges: 260 });
        upsertGraph(
          {
            kind: "uml",
            id: `explore-${nodeId}-${Date.now()}`,
            title: `Neighborhood: ${title}`,
            graph,
          },
          true,
        );
      } catch (error) {
        setStatus(error instanceof Error ? error.message : "Failed to open graph in new tab.");
      }
    },
    [setStatus, upsertGraph],
  );

  const selectOccurrenceByIndex = useCallback(
    async (index: number) => {
      if (activeOccurrences.length === 0) {
        return;
      }
      const boundedIndex =
        ((index % activeOccurrences.length) + activeOccurrences.length) % activeOccurrences.length;
      const occurrence = activeOccurrences[boundedIndex];
      if (!occurrence) {
        return;
      }
      const opened = await openOccurrenceFile(occurrence);
      if (!opened) {
        return;
      }
      setActiveOccurrenceIndex(boundedIndex);
    },
    [activeOccurrences, openOccurrenceFile, setActiveOccurrenceIndex],
  );

  const selectNextOccurrence = useCallback(async () => {
    await selectOccurrenceByIndex(activeOccurrenceIndex + 1);
  }, [activeOccurrenceIndex, selectOccurrenceByIndex]);

  const selectPreviousOccurrence = useCallback(async () => {
    await selectOccurrenceByIndex(activeOccurrenceIndex - 1);
  }, [activeOccurrenceIndex, selectOccurrenceByIndex]);

  const selectEdge = useCallback(
    async (selection: {
      id: string;
      edgeIds: string[];
      kind: string;
      sourceNodeId: string;
      targetNodeId: string;
      sourceLabel: string;
      targetLabel: string;
    }) => {
      if (!projectOpen) {
        return;
      }

      setActiveEdgeContext({
        id: selection.id,
        kind: selection.kind,
        sourceLabel: selection.sourceLabel,
        targetLabel: selection.targetLabel,
      });

      try {
        const sourceEdgeIds = selection.edgeIds.length > 0 ? selection.edgeIds : [selection.id];
        const uniqueEdgeIds = [...new Set(sourceEdgeIds)];
        const occurrenceResults = await Promise.allSettled(
          uniqueEdgeIds.map((edgeId) => api.edgeOccurrences({ id: edgeId })),
        );
        const occurrences = occurrenceResults
          .filter(
            (result): result is PromiseFulfilledResult<SourceOccurrenceDto[]> =>
              result.status === "fulfilled",
          )
          .flatMap((result) => result.value);
        const dedupedOccurrences = [
          ...new Map(
            occurrences.map((occurrence) => [
              `${occurrence.element_id}|${occurrence.kind}|${occurrence.file_path}|${occurrence.start_line}|${occurrence.start_col}|${occurrence.end_line}|${occurrence.end_col}`,
              occurrence,
            ]),
          ).values(),
        ].sort(
          (left, right) =>
            left.file_path.localeCompare(right.file_path) ||
            left.start_line - right.start_line ||
            left.start_col - right.start_col ||
            left.end_line - right.end_line ||
            left.end_col - right.end_col,
        );
        const failedLookups = occurrenceResults.filter(
          (result) => result.status === "rejected",
        ).length;
        if (failedLookups > 0) {
          setStatus(
            `Loaded edge locations with ${failedLookups} lookup failure${failedLookups === 1 ? "" : "s"}.`,
          );
        }

        setActiveOccurrences(dedupedOccurrences);
        if (dedupedOccurrences.length === 0) {
          setActiveOccurrenceIndex(0);
          setStatus(`No source locations recorded for ${selection.kind} edge.`);
          return;
        }

        const firstOccurrence = dedupedOccurrences[0];
        if (!firstOccurrence) {
          setActiveOccurrenceIndex(0);
          return;
        }
        const opened = await openOccurrenceFile(firstOccurrence);
        if (!opened) {
          return;
        }
        setActiveOccurrenceIndex(0);
        setStatus(
          `Selected ${selection.kind} edge (${selection.sourceLabel} -> ${selection.targetLabel}).`,
        );
      } catch (error) {
        setStatus(error instanceof Error ? error.message : "Failed to load edge source locations.");
      }
    },
    [
      openOccurrenceFile,
      projectOpen,
      setActiveEdgeContext,
      setActiveOccurrenceIndex,
      setActiveOccurrences,
      setStatus,
    ],
  );

  const focusSymbolInternal = useCallback(
    async (symbolId: string, label: string, graphMode: FocusGraphMode = "neighborhood") => {
      if (graphMode === "trailDepthOne") {
        const depthOneTrailConfig: TrailUiConfig = { ...trailConfig, depth: 1 };
        setIsTrailRunning(true);
        try {
          const contextPromise = loadNodeContext(symbolId);
          const graphPromise = contextPromise.then((details) =>
            openTrailGraph(symbolId, details.display_name, details.file_path, depthOneTrailConfig),
          );
          const [contextResult, graphResult] = await Promise.allSettled([
            contextPromise,
            graphPromise,
          ]);

          if (contextResult.status === "rejected" && graphResult.status === "rejected") {
            setStatus("Failed to load code context and trail graph for that symbol.");
          } else if (graphResult.status === "rejected") {
            setStatus("Loaded code context, but trail graph failed to load for that symbol.");
          } else if (contextResult.status === "rejected") {
            setStatus("Loaded trail graph, but code context failed to load for that symbol.");
          }
        } finally {
          setIsTrailRunning(false);
        }
        return;
      }

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
    [loadNodeContext, openNeighborhood, openTrailGraph, setIsTrailRunning, setStatus, trailConfig],
  );

  const focusSymbol = useCallback(
    (symbolId: string, label: string) => {
      if (isDirty && activeFilePath && activeNodeId !== symbolId) {
        setPendingFocus({ symbolId, label });
        return;
      }
      void focusSymbolInternal(symbolId, label);
    },
    [activeFilePath, activeNodeId, focusSymbolInternal, isDirty, setPendingFocus],
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
      void focusSymbolInternal(
        pending.symbolId,
        pending.label,
        pending.graphMode ?? "neighborhood",
      );
    },
    [focusSymbolInternal, pendingFocus, saveCurrentFile, savedText, setDraftText, setPendingFocus],
  );

  return {
    openNeighborhoodInNewTab,
    selectOccurrenceByIndex,
    selectNextOccurrence,
    selectPreviousOccurrence,
    selectEdge,
    focusSymbolInternal,
    focusSymbol,
    resolvePendingFocus,
  };
}
