import { afterEach, describe, expect, it, vi } from "vitest";

import {
  createAnalyticsEvent,
  setAnalyticsEmitterForTests,
  toProjectId,
  trackAnalyticsEvent,
  type AnalyticsEnvelope,
} from "../../src/app/analytics";

type AnalyticsEventName = Parameters<typeof trackAnalyticsEvent>[0];

const UX_RESET_EVENTS: readonly AnalyticsEventName[] = [
  "starter_card_cta_clicked",
  "investigate_mode_switched",
  "advanced_drawer_opened",
  "library_space_reopened",
];

afterEach(() => {
  setAnalyticsEmitterForTests(null);
  vi.unstubAllEnvs();
});

describe("analytics", () => {
  it("builds events with stable session and project identifiers", () => {
    const first = createAnalyticsEvent(
      "project_opened",
      { source: "manual" },
      { projectPath: "C:\\Repo\\CodeStory" },
    );
    const second = createAnalyticsEvent(
      "index_started",
      { mode: "Full" },
      { projectPath: "c:/repo/codestory" },
    );

    expect(first.session_id).toBeTruthy();
    expect(first.session_id).toBe(second.session_id);
    expect(first.project_id).toBe(second.project_id);
    expect(first.timestamp).toMatch(/T/);
    expect(first.event_id).toMatch(/^evt-/);
  });

  it("emits events through the configured emitter", () => {
    const events: AnalyticsEnvelope[] = [];
    setAnalyticsEmitterForTests((event) => {
      events.push(event);
    });

    trackAnalyticsEvent(
      "ask_submitted",
      { prompt_length: 24, tab: "agent", is_first: true },
      { projectPath: "/workspace/codestory" },
    );

    expect(events).toHaveLength(1);
    expect(events[0]?.event).toBe("ask_submitted");
    expect(events[0]?.payload.prompt_length).toBe(24);
    expect(events[0]?.project_id).toBe(toProjectId("/workspace/codestory"));
  });

  it("emits ux reset events through the configured emitter", () => {
    const events: AnalyticsEnvelope[] = [];
    setAnalyticsEmitterForTests((event) => {
      events.push(event);
    });

    for (const eventName of UX_RESET_EVENTS) {
      trackAnalyticsEvent(
        eventName,
        { source: "ux_reset", state: "opened" },
        { projectPath: "/workspace/codestory" },
      );
    }

    expect(events).toHaveLength(UX_RESET_EVENTS.length);
    expect(events.map((event) => event.event)).toEqual(UX_RESET_EVENTS);
    expect(events.every((event) => event.project_id === toProjectId("/workspace/codestory"))).toBe(
      true,
    );
    expect(events.every((event) => event.payload.source === "ux_reset")).toBe(true);
  });

  it("does not emit events when analytics is disabled", () => {
    const events: AnalyticsEnvelope[] = [];
    setAnalyticsEmitterForTests((event) => {
      events.push(event);
    });
    vi.stubEnv("VITE_DISABLE_ANALYTICS", "true");

    trackAnalyticsEvent(
      "starter_card_cta_clicked",
      { source: "starter_card", cta: "quickstart" },
      { projectPath: "/workspace/codestory" },
    );

    expect(events).toHaveLength(0);
  });

  it("returns null project ids for empty paths", () => {
    expect(toProjectId("")).toBeNull();
    expect(toProjectId("   ")).toBeNull();
    expect(toProjectId(null)).toBeNull();
    expect(toProjectId(undefined)).toBeNull();
  });
});
