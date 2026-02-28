import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

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
  citations: [
    {
      node_id: "node-1",
      display_name: "Controller.handle",
      kind: "METHOD",
      file_path: "src/controller.ts",
      line: 42,
      score: 0.91,
    },
  ],
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
  const onActivateGraph = vi.fn();
  const onFocusSymbol = vi.fn();
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
    onActivateGraph,
    rootSymbols: EMPTY_SYMBOLS,
    childrenByNode: {},
    expandedNodes: {},
    onToggleNode: vi.fn(async () => undefined),
    onFocusSymbol,
    activeSymbolId: null,
    ...overrides,
  };

  const view = render(<ResponsePane {...props} />);
  return {
    ...view,
    onRetrievalProfileChange,
    onActivateGraph,
    onFocusSymbol,
  };
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("ResponsePane", () => {
  it("renders typed response cards and section actions", () => {
    const { onActivateGraph } = renderPane();

    const blocks = document.querySelectorAll(".response-block");
    expect(blocks.length).toBe(3);
    expect(blocks[0]?.textContent).toContain("First block");
    expect(blocks[2]?.textContent).toContain("Last block");
    expect(screen.getByTestId("mermaid-diagram")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Open Related Graph: Overview" }));
    expect(onActivateGraph).toHaveBeenCalledWith("mermaid-overview");
  });

  it("keeps retrieval trace collapsed in advanced settings by default", () => {
    renderPane();

    expect(screen.queryByText(/"request_id": "ask-1"/)).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Show Advanced Settings" }));
    const traceSummary = screen.getByText("Raw Retrieval Trace");
    const traceDetails = traceSummary.closest("details");
    expect(traceDetails).not.toHaveAttribute("open");

    fireEvent.click(traceSummary);
    expect(screen.getByText(/"request_id": "ask-1"/)).toBeInTheDocument();
    expect(screen.getByText(/"policy_mode": "latency_first"/)).toBeInTheDocument();
  });

  it("wires basic profile quick pick changes to callback", () => {
    const { onRetrievalProfileChange } = renderPane();

    fireEvent.change(screen.getByLabelText("Profile quick pick"), {
      target: { value: "preset:impact" },
    });

    expect(onRetrievalProfileChange).toHaveBeenCalledWith({
      kind: "preset",
      preset: "impact",
    });
  });

  it("adds evidence CTA to jump to cited node", () => {
    const { onFocusSymbol } = renderPane();

    fireEvent.click(screen.getByRole("button", { name: "Jump to Cited Node" }));
    expect(onFocusSymbol).toHaveBeenCalledWith("node-1", "Controller.handle");
  });

  it("exports markdown and json summaries from the current answer", async () => {
    const createObjectURL = vi.fn(() => "blob:codestory-summary");
    const revokeObjectURL = vi.fn();
    const anchorClickSpy = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {
      return undefined;
    });

    Object.defineProperty(URL, "createObjectURL", {
      configurable: true,
      writable: true,
      value: createObjectURL,
    });
    Object.defineProperty(URL, "revokeObjectURL", {
      configurable: true,
      writable: true,
      value: revokeObjectURL,
    });

    renderPane();

    fireEvent.click(screen.getByRole("button", { name: "Export Markdown" }));
    fireEvent.click(screen.getByRole("button", { name: "Export JSON" }));

    expect(createObjectURL).toHaveBeenCalledTimes(2);
    expect(anchorClickSpy).toHaveBeenCalledTimes(2);
    expect(revokeObjectURL).toHaveBeenCalledTimes(2);

    const markdownBlob = createObjectURL.mock.calls[0]?.[0] as Blob;
    const markdownText = await markdownBlob.text();
    expect(markdownText).toContain("# Codestory Answer Summary");
    expect(markdownText).toContain("Controller.handle");

    const jsonBlob = createObjectURL.mock.calls[1]?.[0] as Blob;
    const jsonText = await jsonBlob.text();
    const parsed = JSON.parse(jsonText) as { citations: Array<{ display_name: string }> };
    expect(parsed.citations[0]?.display_name).toBe("Controller.handle");
  });
});
