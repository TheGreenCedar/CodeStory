import { beforeEach, describe, expect, it } from "vitest";

import {
  INVESTIGATION_SPACES_STORAGE_KEY,
  createSpace,
  deleteSpace,
  listSpaces,
  loadSpace,
  loadSpaces,
  updateSpace,
} from "../../src/features/spaces";

describe("spaces storage helpers", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("creates and loads a space from localStorage", () => {
    const created = createSpace({
      name: "API Investigation",
      prompt: "Trace request path",
      activeGraphId: "graph-1",
      activeSymbolId: "symbol-1",
      notes: "Start with ingress handlers",
      owner: "alber",
    });

    const spaces = listSpaces();
    const loaded = loadSpace(created.id);

    expect(spaces).toHaveLength(1);
    expect(spaces[0]?.id).toBe(created.id);
    expect(loaded?.name).toBe("API Investigation");
    expect(loaded?.prompt).toBe("Trace request path");
    expect(loaded?.activeGraphId).toBe("graph-1");
    expect(loaded?.activeSymbolId).toBe("symbol-1");
    expect(loaded?.notes).toBe("Start with ingress handlers");
    expect(loaded?.owner).toBe("alber");
    expect(loaded?.createdAt).toBeTruthy();
    expect(loaded?.updatedAt).toBeTruthy();
  });

  it("updates an existing space while preserving immutable fields", () => {
    const created = createSpace({
      name: "Old Name",
      prompt: "Old prompt",
      owner: "owner",
    });

    const updated = updateSpace(created.id, {
      name: "New Name",
      prompt: "New prompt",
      activeGraphId: "graph-2",
      activeSymbolId: "symbol-2",
      notes: "Updated notes",
      owner: "new-owner",
    });

    expect(updated).not.toBeNull();
    expect(updated?.id).toBe(created.id);
    expect(updated?.createdAt).toBe(created.createdAt);
    expect(updated?.name).toBe("New Name");
    expect(updated?.prompt).toBe("New prompt");
    expect(updated?.activeGraphId).toBe("graph-2");
    expect(updated?.activeSymbolId).toBe("symbol-2");
    expect(updated?.notes).toBe("Updated notes");
    expect(updated?.owner).toBe("new-owner");
    expect(updated?.updatedAt >= created.updatedAt).toBe(true);
  });

  it("deletes spaces by id", () => {
    const first = createSpace({
      name: "First",
      prompt: "Prompt",
      owner: "owner",
    });
    const second = createSpace({
      name: "Second",
      prompt: "Prompt",
      owner: "owner",
    });

    expect(deleteSpace(first.id)).toBe(true);
    expect(deleteSpace(first.id)).toBe(false);
    expect(loadSpace(first.id)).toBeNull();
    expect(loadSpace(second.id)).not.toBeNull();
    expect(loadSpaces()).toHaveLength(1);
  });

  it("handles malformed persisted payloads safely", () => {
    window.localStorage.setItem(INVESTIGATION_SPACES_STORAGE_KEY, "{not-json");
    expect(loadSpaces()).toEqual([]);

    window.localStorage.setItem(INVESTIGATION_SPACES_STORAGE_KEY, JSON.stringify({ bad: true }));
    expect(loadSpaces()).toEqual([]);

    window.localStorage.setItem(
      INVESTIGATION_SPACES_STORAGE_KEY,
      JSON.stringify([{ id: "x", name: "broken" }]),
    );
    expect(loadSpaces()).toEqual([]);
  });
});
