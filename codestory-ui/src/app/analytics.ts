const SESSION_STORAGE_KEY = "codestory:analytics-session-id";
const ANALYTICS_CHANNEL = "codestory:analytics";

type AnalyticsEventName =
  | "project_opened"
  | "index_started"
  | "index_completed"
  | "ask_submitted"
  | "node_selected"
  | "file_saved"
  | "trail_run"
  | "command_invoked"
  | "starter_card_cta_clicked"
  | "investigate_mode_switched"
  | "advanced_drawer_opened"
  | "library_space_reopened";

type AnalyticsOptions = {
  projectPath?: string | null;
};

export type AnalyticsEnvelope = {
  event_id: string;
  event: AnalyticsEventName;
  timestamp: string;
  session_id: string;
  project_id: string | null;
  payload: Record<string, unknown>;
};

export type AnalyticsEmitter = (event: AnalyticsEnvelope) => void;

let testEmitter: AnalyticsEmitter | null = null;
let memorySessionId: string | null = null;

function randomHexSegment(length: number): string {
  let result = "";
  for (let index = 0; index < length; index += 1) {
    result += Math.floor(Math.random() * 16).toString(16);
  }
  return result;
}

function fallbackId(prefix: string): string {
  return `${prefix}-${Date.now().toString(36)}-${randomHexSegment(10)}`;
}

function createId(prefix: string): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return `${prefix}-${crypto.randomUUID()}`;
  }
  return fallbackId(prefix);
}

function resolveSessionId(): string {
  if (typeof window === "undefined") {
    if (!memorySessionId) {
      memorySessionId = createId("session");
    }
    return memorySessionId;
  }

  const existing = window.localStorage.getItem(SESSION_STORAGE_KEY);
  if (existing && existing.trim().length > 0) {
    return existing;
  }

  const next = createId("session");
  window.localStorage.setItem(SESSION_STORAGE_KEY, next);
  return next;
}

function fnv1aHash(value: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = (hash * 0x01000193) >>> 0;
  }
  return hash.toString(36);
}

export function toProjectId(projectPath?: string | null): string | null {
  if (!projectPath) {
    return null;
  }

  const normalized = projectPath.trim().replace(/\\/g, "/").toLowerCase();
  if (normalized.length === 0) {
    return null;
  }

  return `project-${fnv1aHash(normalized)}`;
}

function analyticsEnabled(): boolean {
  return import.meta.env.VITE_DISABLE_ANALYTICS !== "true";
}

export function createAnalyticsEvent(
  event: AnalyticsEventName,
  payload: Record<string, unknown>,
  options?: AnalyticsOptions,
): AnalyticsEnvelope {
  return {
    event_id: createId("evt"),
    event,
    timestamp: new Date().toISOString(),
    session_id: resolveSessionId(),
    project_id: toProjectId(options?.projectPath),
    payload,
  };
}

function emitAnalyticsEvent(event: AnalyticsEnvelope): void {
  if (testEmitter) {
    testEmitter(event);
    return;
  }

  if (typeof window !== "undefined") {
    window.dispatchEvent(
      new CustomEvent<AnalyticsEnvelope>(ANALYTICS_CHANNEL, {
        detail: event,
      }),
    );
  }
}

export function trackAnalyticsEvent(
  event: AnalyticsEventName,
  payload: Record<string, unknown>,
  options?: AnalyticsOptions,
): void {
  if (!analyticsEnabled()) {
    return;
  }

  emitAnalyticsEvent(createAnalyticsEvent(event, payload, options));
}

export function setAnalyticsEmitterForTests(emitter: AnalyticsEmitter | null): void {
  testEmitter = emitter;
}
