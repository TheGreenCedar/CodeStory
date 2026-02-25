import type {
  AgentAnswerDto,
  AgentAskRequest,
  AppEventPayload,
  GraphRequest,
  GraphResponse,
  NodeDetailsDto,
  NodeDetailsRequest,
  OpenProjectRequest,
  ProjectSummary,
  ReadFileTextRequest,
  ReadFileTextResponse,
  SearchHit,
  SearchRequest,
  SetUiLayoutRequest,
  StartIndexingRequest,
  SymbolSummaryDto,
  TrailConfigDto,
  WriteFileResponse,
  WriteFileTextRequest,
} from "../generated/api";

class ApiClient {
  constructor(
    private readonly baseUrl = (import.meta.env.VITE_API_BASE_URL as string | undefined) ?? "",
  ) {}

  private resolve(path: string): string {
    if (this.baseUrl.length === 0) {
      return path;
    }
    return `${this.baseUrl}${path}`;
  }

  private async request<T>(path: string, init?: RequestInit): Promise<T> {
    const response = await fetch(this.resolve(path), {
      ...init,
      headers: {
        "Content-Type": "application/json",
        ...init?.headers,
      },
    });

    if (!response.ok) {
      const fallback = `${response.status} ${response.statusText}`;
      let message = fallback;
      try {
        const body = (await response.json()) as { message?: string };
        if (typeof body.message === "string") {
          message = body.message;
        }
      } catch {
        // Keep fallback error message.
      }
      throw new Error(message);
    }

    if (response.status === 204) {
      return undefined as T;
    }

    const raw = await response.text();
    if (raw.trim().length === 0) {
      return undefined as T;
    }

    return JSON.parse(raw) as T;
  }

  openProject(req: OpenProjectRequest): Promise<ProjectSummary> {
    return this.request<ProjectSummary>("/api/open-project", {
      method: "POST",
      body: JSON.stringify(req),
    });
  }

  startIndexing(req: StartIndexingRequest): Promise<void> {
    return this.request<void>("/api/index/start", {
      method: "POST",
      body: JSON.stringify(req),
    });
  }

  search(req: SearchRequest): Promise<SearchHit[]> {
    return this.request<SearchHit[]>("/api/search", {
      method: "POST",
      body: JSON.stringify(req),
    });
  }

  ask(req: AgentAskRequest): Promise<AgentAnswerDto> {
    return this.request<AgentAnswerDto>("/api/agent/ask", {
      method: "POST",
      body: JSON.stringify(req),
    });
  }

  graphNeighborhood(req: GraphRequest): Promise<GraphResponse> {
    return this.request<GraphResponse>("/api/graph/neighborhood", {
      method: "POST",
      body: JSON.stringify(req),
    });
  }

  graphTrail(req: TrailConfigDto): Promise<GraphResponse> {
    return this.request<GraphResponse>("/api/graph/trail", {
      method: "POST",
      body: JSON.stringify(req),
    });
  }

  nodeDetails(req: NodeDetailsRequest): Promise<NodeDetailsDto> {
    return this.request<NodeDetailsDto>("/api/node/details", {
      method: "POST",
      body: JSON.stringify(req),
    });
  }

  readFileText(req: ReadFileTextRequest): Promise<ReadFileTextResponse> {
    return this.request<ReadFileTextResponse>("/api/file/read", {
      method: "POST",
      body: JSON.stringify(req),
    });
  }

  writeFileText(req: WriteFileTextRequest): Promise<WriteFileResponse> {
    return this.request<WriteFileResponse>("/api/file/write-text", {
      method: "POST",
      body: JSON.stringify(req),
    });
  }

  getUiLayout(): Promise<string | null> {
    return this.request<string | null>("/api/ui-layout");
  }

  setUiLayout(req: SetUiLayoutRequest): Promise<void> {
    return this.request<void>("/api/ui-layout", {
      method: "POST",
      body: JSON.stringify(req),
    });
  }

  listRootSymbols(limit: number | null = null): Promise<SymbolSummaryDto[]> {
    const query = limit === null ? "" : `?limit=${encodeURIComponent(limit)}`;
    return this.request<SymbolSummaryDto[]>(`/api/explorer/root${query}`);
  }

  listChildrenSymbols(parentId: string): Promise<SymbolSummaryDto[]> {
    return this.request<SymbolSummaryDto[]>(
      `/api/explorer/children/${encodeURIComponent(parentId)}`,
    );
  }

  subscribeEvents(onEvent: (event: AppEventPayload) => void): () => void {
    const source = new EventSource(this.resolve("/api/events"));

    source.addEventListener("app_event", (event) => {
      if (!(event instanceof MessageEvent)) {
        return;
      }

      try {
        const parsed = JSON.parse(event.data) as AppEventPayload;
        onEvent(parsed);
      } catch {
        // Ignore malformed events.
      }
    });

    return () => source.close();
  }
}

export const api = new ApiClient();
