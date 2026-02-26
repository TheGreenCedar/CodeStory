import type { GraphResponse, LayoutDirection } from "../../generated/api";
import { buildDagreLayout } from "./dagreLayout";
import { buildCanonicalLayout } from "./semanticGraph";
import type { LayoutElements } from "./types";

export function buildSemanticLayout(
  graph: GraphResponse,
  layoutDirection: LayoutDirection = "Horizontal",
): LayoutElements {
  const canonical = buildCanonicalLayout(graph);
  return buildDagreLayout(canonical, layoutDirection);
}

export function buildFallbackLayout(
  graph: GraphResponse,
  layoutDirection: LayoutDirection = "Horizontal",
): LayoutElements {
  return buildSemanticLayout(graph, layoutDirection);
}
