import { useCallback, type Dispatch, type SetStateAction } from "react";

import { api } from "../api/client";
import type { GraphArtifactDto, NodeDetailsDto } from "../generated/api";
import { defaultTrailUiConfig, toTrailConfigDto, type TrailUiConfig } from "../graph/trailConfig";
import { isLikelyTestOrBenchPath, trailModeLabel } from "./layoutPersistence";

type UseTrailActionsArgs = {
  activeNodeDetails: NodeDetailsDto | null;
  setIsTrailRunning: Dispatch<SetStateAction<boolean>>;
  setStatus: Dispatch<SetStateAction<string>>;
  trailConfig: TrailUiConfig;
  trailDisabledReason: string | null;
  setTrailConfig: Dispatch<SetStateAction<TrailUiConfig>>;
  upsertGraph: (graph: GraphArtifactDto, activate?: boolean) => void;
};

export type TrailActions = {
  updateTrailConfig: (patch: Partial<TrailUiConfig>) => void;
  resetTrailConfig: () => void;
  openTrailGraph: (
    rootId: string,
    rootLabel: string,
    rootFilePath: string | null,
    config: TrailUiConfig,
  ) => Promise<void>;
  runTrail: () => Promise<void>;
};

export function useTrailActions({
  activeNodeDetails,
  setIsTrailRunning,
  setStatus,
  trailConfig,
  trailDisabledReason,
  setTrailConfig,
  upsertGraph,
}: UseTrailActionsArgs): TrailActions {
  const updateTrailConfig = useCallback(
    (patch: Partial<TrailUiConfig>) => {
      setTrailConfig((previous) => ({
        ...previous,
        ...patch,
      }));
    },
    [setTrailConfig],
  );

  const resetTrailConfig = useCallback(() => {
    setTrailConfig(defaultTrailUiConfig());
  }, [setTrailConfig]);

  const queryTrailGraph = useCallback(
    async (rootId: string, rootFilePath: string | null, config: TrailUiConfig) => {
      const rootInTestPath = isLikelyTestOrBenchPath(rootFilePath);
      const initialConfig =
        config.callerScope === "ProductionOnly" && rootInTestPath
          ? { ...config, callerScope: "IncludeTestsAndBenches" as const }
          : config;

      let graph = await api.graphTrail(toTrailConfigDto(rootId, initialConfig));
      let usedExpandedCallerScope = initialConfig.callerScope !== config.callerScope;

      if (
        !usedExpandedCallerScope &&
        config.callerScope === "ProductionOnly" &&
        graph.nodes.length <= 1 &&
        graph.edges.length === 0
      ) {
        const fallbackConfig = { ...config, callerScope: "IncludeTestsAndBenches" as const };
        const fallbackGraph = await api.graphTrail(toTrailConfigDto(rootId, fallbackConfig));
        if (
          fallbackGraph.edges.length > graph.edges.length ||
          fallbackGraph.nodes.length > graph.nodes.length
        ) {
          graph = fallbackGraph;
          usedExpandedCallerScope = true;
        }
      }

      return { graph, usedExpandedCallerScope };
    },
    [],
  );

  const openTrailGraph = useCallback(
    async (
      rootId: string,
      rootLabel: string,
      rootFilePath: string | null,
      config: TrailUiConfig,
    ) => {
      const { graph, usedExpandedCallerScope } = await queryTrailGraph(
        rootId,
        rootFilePath,
        config,
      );
      const trailGraphId = `trail-${rootId}-${Date.now()}`;
      upsertGraph(
        {
          kind: "uml",
          id: trailGraphId,
          title: `Trail: ${rootLabel} (${trailModeLabel(config.mode)})`,
          graph,
        },
        true,
      );
      const scopeSuffix = usedExpandedCallerScope ? " using expanded caller scope." : ".";
      setStatus(
        `Trail loaded (${graph.nodes.length} nodes, ${graph.edges.length} edges)${scopeSuffix}`,
      );
    },
    [queryTrailGraph, setStatus, upsertGraph],
  );

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
      await openTrailGraph(
        activeNodeDetails.id,
        activeNodeDetails.display_name,
        activeNodeDetails.file_path,
        trailConfig,
      );
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "Failed to run trail graph query.");
    } finally {
      setIsTrailRunning(false);
    }
  }, [
    activeNodeDetails,
    openTrailGraph,
    setIsTrailRunning,
    setStatus,
    trailConfig,
    trailDisabledReason,
  ]);

  return {
    updateTrailConfig,
    resetTrailConfig,
    openTrailGraph,
    runTrail,
  };
}
