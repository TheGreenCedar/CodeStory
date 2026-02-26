import type { EdgeKind } from "../../generated/api";

type EdgeOffsetProfile = {
  originOffsetX: number;
  targetOffsetX: number;
  originOffsetY: number;
  targetOffsetY: number;
  verticalOffset: number;
};

export type ParityStyleProfile = {
  rasterStep: number;
  bundling: {
    minEdgesForBundling: number;
    laneBandBaseHeight: number;
    laneBandDenseHeight: number;
    laneBandDenseThreshold: number;
    minGroupSizeThresholds: Array<{
      minDepth: number;
      minDensity: number;
      minGroupSize: number;
    }>;
    minTrunkGap: number;
    maxTrunkGap: number;
    corridorPadding: number;
    trunkGutter: number;
    sharedTrunkPadding: number;
  };
  routing: {
    obstaclePadding: number;
    sourceExit: number;
    targetEntry: number;
    branchStub: number;
    trunkPenaltyWeight: number;
    channelLaneStep: number;
    channelLaneMaxOffset: number;
    channelLanePenaltyWeight: number;
    channelTrunkInset: number;
    yDetourStep: number;
    xDetourStep: number;
    scoreWeights: {
      collision: number;
      turnBase: number;
      turnBundleScale: number;
      turnBundleCap: number;
      length: number;
      candidateOrder: number;
    };
    edgeOffsets: {
      default: EdgeOffsetProfile;
      bundled: EdgeOffsetProfile;
      kindOverrides: Partial<Record<EdgeKind, Partial<EdgeOffsetProfile>>>;
    };
  };
  rendering: {
    cornerRadius: number;
    trunkElbowGutter: number;
    directElbowGutter: number;
    trunkJoinHookRadius: number;
    trunkJoinHookDepth: number;
    trunkJoinMinRadius: number;
    trunkJoinMinDepth: number;
    strokeAmplification: {
      bundledLogMultiplier: number;
      bundledMaxBoost: number;
      multiplicityStep: number;
      multiplicityMaxBoost: number;
      hierarchyBoost: number;
    };
    interactionWidth: {
      default: number;
      hierarchy: number;
      bundledBase: number;
      bundledScale: number;
      bundledMaxExtra: number;
    };
    certainty: {
      uncertainDash: string;
      uncertainOpacity: number;
      probableOpacity: number;
      hierarchyOpacityBias: number;
    };
  };
  markers: {
    default: { width: number; height: number };
    bundled: { width: number; height: number };
    inheritance: { width: number; height: number };
    templateSpecialization: { width: number; height: number };
  };
};

export const PARITY_CONSTANTS: ParityStyleProfile = {
  // Sourcetrail-style routes are drawn on a strict pixel raster for stable elbows.
  rasterStep: 8,
  bundling: {
    // Enable adaptive bundling only when trail density is visually meaningful.
    minEdgesForBundling: 8,
    // Lane spacing used for sparse call trails.
    laneBandBaseHeight: 56,
    // Wider lane spacing for dense graphs to reduce overlap.
    laneBandDenseHeight: 74,
    // Density score threshold where dense lane spacing should activate.
    laneBandDenseThreshold: 2.2,
    // Deterministic density/depth thresholds used to adapt min bundle size.
    minGroupSizeThresholds: [
      { minDepth: 4, minDensity: 2.8, minGroupSize: 2 },
      { minDepth: 3, minDensity: 2.0, minGroupSize: 3 },
      { minDepth: 0, minDensity: 0, minGroupSize: 4 },
    ],
    // Sourcetrail-like trunk separation band from the origin handle column.
    minTrunkGap: 56,
    // Prevent runaway trunk placement in sparse long edges.
    maxTrunkGap: 176,
    // Keep trunk coordinates inside the source-target corridor.
    corridorPadding: 42,
    // Fallback offset when corridor clamp is too tight.
    trunkGutter: 34,
    // Extend channel trunk endpoints beyond members for clearer branch fan-out.
    sharedTrunkPadding: 24,
  },
  routing: {
    // Obstacle padding around nodes for collision-avoidance scoring.
    obstaclePadding: 18,
    // Horizontal source-side exit for fallback orthogonal candidates.
    sourceExit: 40,
    // Horizontal target-side entry for fallback orthogonal candidates.
    targetEntry: 40,
    // Short branch stubs used by bundled-trunk candidates.
    branchStub: 24,
    // Keep routed interior points close to configured trunk coordinates.
    trunkPenaltyWeight: 0.08,
    // Per-channel branch spacing in pixels for fan-in/fan-out separation.
    channelLaneStep: 14,
    // Prevent lane offsets from drifting too far from source/target anchors.
    channelLaneMaxOffset: 56,
    // Bias path selection toward the assigned channel lane when bundled.
    channelLanePenaltyWeight: 0.09,
    // Keep assigned lanes slightly inside shared trunk span boundaries.
    channelTrunkInset: 10,
    // Vertical detour search spacing for dense routing fallback.
    yDetourStep: 96,
    // Horizontal detour search spacing for dense routing fallback.
    xDetourStep: 72,
    scoreWeights: {
      // Collisions are dominant: non-overlap is preferred over short paths.
      collision: 100_000,
      // Baseline bend penalty for cleaner orthogonal routes.
      turnBase: 12,
      // Slightly amplify bend penalty for heavier bundled channels.
      turnBundleScale: 0.8,
      // Cap turn amplification to avoid over-penalizing dense groups.
      turnBundleCap: 8,
      // Use path length as a tie-breaker after collisions and turns.
      length: 0.035,
      // Keep candidate ordering deterministic while preserving style-first ranking.
      candidateOrder: 0.002,
    },
    edgeOffsets: {
      // Sourcetrail-inspired default edge offsets around node boundaries.
      default: {
        originOffsetX: 17,
        targetOffsetX: 17,
        originOffsetY: 5,
        targetOffsetY: -5,
        verticalOffset: 2,
      },
      // Bundled channels use straighter source/target offsets and no vertical wobble.
      bundled: {
        originOffsetX: 24,
        targetOffsetX: 24,
        originOffsetY: 0,
        targetOffsetY: 0,
        verticalOffset: 0,
      },
      // Kind-specific tuning to mimic Sourcetrail edge-family personality.
      kindOverrides: {
        CALL: { originOffsetY: 3, targetOffsetY: -3, verticalOffset: 4 },
        USAGE: { originOffsetY: 1, targetOffsetY: -1, verticalOffset: 6 },
        OVERRIDE: { originOffsetY: 1, targetOffsetY: -1, verticalOffset: 6 },
        INHERITANCE: {
          originOffsetX: 7,
          targetOffsetX: 34,
          originOffsetY: 10,
          targetOffsetY: -10,
          verticalOffset: 0,
        },
        TEMPLATE_SPECIALIZATION: { targetOffsetX: 25 },
        INCLUDE: { originOffsetY: 0, targetOffsetY: 0 },
        MACRO_USAGE: { originOffsetY: 0, targetOffsetY: 0 },
      },
    },
  },
  rendering: {
    // Corner roundness used to match Sourcetrail-like orthogonal smoothing.
    cornerRadius: 10,
    // Required gutter for trunk elbows to keep branch exits legible.
    trunkElbowGutter: 42,
    // Direct-edge elbow gutter for non-bundled flow.
    directElbowGutter: 10,
    // Hook radius for trunk-join decoration near branch exits.
    trunkJoinHookRadius: 8,
    // Hook depth for trunk-join decoration near branch exits.
    trunkJoinHookDepth: 10,
    // Minimum hook radius/depth to avoid tiny decorative loops.
    trunkJoinMinRadius: 4,
    trunkJoinMinDepth: 4,
    strokeAmplification: {
      bundledLogMultiplier: 0.9,
      bundledMaxBoost: 3.4,
      multiplicityStep: 0.15,
      multiplicityMaxBoost: 0.72,
      hierarchyBoost: 0.2,
    },
    interactionWidth: {
      default: 18,
      hierarchy: 20,
      bundledBase: 24,
      bundledScale: 2.4,
      bundledMaxExtra: 16,
    },
    certainty: {
      uncertainDash: "6 5",
      uncertainOpacity: 0.85,
      probableOpacity: 0.95,
      hierarchyOpacityBias: 0.14,
    },
  },
  markers: {
    default: { width: 10, height: 10 },
    bundled: { width: 12, height: 12 },
    inheritance: { width: 18, height: 16 },
    templateSpecialization: { width: 14, height: 13 },
  },
};

type RequiredBundlingFields = Pick<
  ParityStyleProfile["bundling"],
  | "minEdgesForBundling"
  | "laneBandBaseHeight"
  | "laneBandDenseHeight"
  | "laneBandDenseThreshold"
  | "minGroupSizeThresholds"
  | "minTrunkGap"
  | "maxTrunkGap"
  | "corridorPadding"
  | "trunkGutter"
  | "sharedTrunkPadding"
>;

type RequiredRoutingFields = Pick<
  ParityStyleProfile["routing"],
  | "obstaclePadding"
  | "sourceExit"
  | "targetEntry"
  | "branchStub"
  | "trunkPenaltyWeight"
  | "channelLaneStep"
  | "channelLaneMaxOffset"
  | "channelLanePenaltyWeight"
  | "channelTrunkInset"
  | "yDetourStep"
  | "xDetourStep"
  | "scoreWeights"
  | "edgeOffsets"
>;

type RequiredRenderingFields = Pick<
  ParityStyleProfile["rendering"],
  | "cornerRadius"
  | "trunkElbowGutter"
  | "directElbowGutter"
  | "trunkJoinHookRadius"
  | "trunkJoinHookDepth"
  | "trunkJoinMinRadius"
  | "trunkJoinMinDepth"
  | "strokeAmplification"
  | "interactionWidth"
  | "certainty"
>;

const _profileFieldAssertions: {
  bundling: RequiredBundlingFields;
  routing: RequiredRoutingFields;
  rendering: RequiredRenderingFields;
} = {
  bundling: PARITY_CONSTANTS.bundling,
  routing: PARITY_CONSTANTS.routing,
  rendering: PARITY_CONSTANTS.rendering,
};
void _profileFieldAssertions;
