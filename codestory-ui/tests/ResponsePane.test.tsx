import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { AgentAnswerDto, GraphArtifactDto, SymbolSummaryDto } from "../src/generated/api";
import { ResponsePane } from "../src/components/ResponsePane";

vi.mock("../src/components/MermaidDiagram", () => ({
  MermaidDiagram: ({ syntax }: { syntax: string }) => (
    <div data-testid="mermaid-diagram">{syntax}</div>
  ),
}));

const EMPTY_SYMBOLS: SymbolSummaryDto[] = [];

const BASE_GRAPH_MAP: Record<string, GraphArtifactDto> = {
  "mermaid-overview": {
    kind: "mermaid",
    id: "mermaid-overview",
    title: "Overview",
    diagram: "flowchart",
    mermaid_syntax: 'flowchart LR\n  A["A"] --> B["B"]',
  },
};

const ANSWER_FIXTURE: AgentAnswerDto = {
  answer_id: "ask-1",
  prompt: "Explain call flow",
  summary: "Summary",
  sections: [
    {
      id: "analysis",
      title: "Analysis",
      blocks: [
        {
          kind: "markdown",
          markdown: "First block",
        },
        {
          kind: "mermaid",
          graph_id: "mermaid-overview",
        },
        {
          kind: "markdown",
          markdown: "Last block",
        },
      ],
    },
  ],
  citations: [],
  graphs: [BASE_GRAPH_MAP["mermaid-overview"]],
  retrieval_trace: {
    request_id: "ask-1",
    resolved_profile: "callflow",
    policy_mode: "latency_first",
    total_latency_ms: 102,
    sla_target_ms: 18000,
    sla_missed: false,
    annotations: ["test"],
    steps: [],
  },
};

function renderPane(overrides: Partial<Parameters<typeof ResponsePane>[0]> = {}) {
  const onRetrievalProfileChange = vi.fn();
  const props: Parameters<typeof ResponsePane>[0] = {
    selectedTab: "agent",
    onSelectTab: vi.fn(),
    prompt: "Explain call flow",
    onPromptChange: vi.fn(),
    retrievalProfile: { kind: "auto" },
    onRetrievalProfileChange,
    agentBackend: "codex",
    onAgentBackendChange: vi.fn(),
    agentCommand: "",
    onAgentCommandChange: vi.fn(),
    onAskAgent: vi.fn(),
    isBusy: false,
    projectOpen: true,
    agentAnswer: ANSWER_FIXTURE,
    graphMap: BASE_GRAPH_MAP,
    onActivateGraph: vi.fn(),
    rootSymbols: EMPTY_SYMBOLS,
    childrenByNode: {},
    expandedNodes: {},
    onToggleNode: vi.fn(async () => undefined),
    onFocusSymbol: vi.fn(),
    activeSymbolId: null,
    ...overrides,
  };

  const view = render(<ResponsePane {...props} />);
  return {
    ...view,
    onRetrievalProfileChange,
  };
}

describe("ResponsePane", () => {
  it("renders typed response blocks in order with inline mermaid", () => {
    renderPane();

    const blocks = document.querySelectorAll(".response-block");
    expect(blocks.length).toBe(3);
    expect(blocks[0]?.textContent).toContain("First block");
    expect(blocks[2]?.textContent).toContain("Last block");
    expect(screen.getByTestId("mermaid-diagram")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open in Graph Pane" })).toBeInTheDocument();
  });

  it("shows retrieval trace panel with machine-readable JSON", () => {
    renderPane();

    expect(screen.getByText("Retrieval Trace")).toBeInTheDocument();
    expect(screen.getByText(/"request_id": "ask-1"/)).toBeInTheDocument();
    expect(screen.getByText(/"policy_mode": "latency_first"/)).toBeInTheDocument();
  });

  it("wires profile selector changes to callback", () => {
    const { onRetrievalProfileChange } = renderPane();

    fireEvent.change(screen.getByDisplayValue("Auto (latency-first)"), {
      target: { value: "preset" },
    });

    expect(onRetrievalProfileChange).toHaveBeenCalledWith({
      kind: "preset",
      preset: "architecture",
    });
  });
});
