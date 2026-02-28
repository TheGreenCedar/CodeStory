import { describe, expect, it } from "vitest";

import type { BookmarkDto } from "../../src/generated/api";
import {
  filterBookmarksByQuery,
  parseBookmarkTags,
  upsertBookmarkMetadata,
} from "../../src/components/bookmarkManagerUtils";

const BOOKMARKS: BookmarkDto[] = [
  {
    id: "bookmark-a",
    category_id: "cat-a",
    node_id: "node-a",
    node_label: "GraphViewport",
    node_kind: "CLASS",
    file_path: "src/graph/GraphViewport.tsx",
    comment: "handles zoom and pan",
  },
  {
    id: "bookmark-b",
    category_id: "cat-b",
    node_id: "node-b",
    node_label: "BookmarkManager",
    node_kind: "CLASS",
    file_path: "src/components/BookmarkManager.tsx",
    comment: "bookmark CRUD entrypoint",
  },
];

describe("bookmarkManagerUtils", () => {
  it("normalizes tag drafts and removes case-insensitive duplicates", () => {
    const tags = parseBookmarkTags(" ui,critical, UI,  rust ,critical ");

    expect(tags).toEqual(["ui", "critical", "rust"]);
  });

  it("upserts and removes bookmark metadata when drafts become empty", () => {
    const seeded = upsertBookmarkMetadata({}, "bookmark-a", "ui,critical", "Investigate this path");
    expect(seeded["bookmark-a"]?.tags).toEqual(["ui", "critical"]);
    expect(seeded["bookmark-a"]?.notesTemplate).toBe("Investigate this path");

    const cleared = upsertBookmarkMetadata(seeded, "bookmark-a", "   ", "   ");
    expect(cleared["bookmark-a"]).toBeUndefined();
  });

  it("filters bookmarks across label/comment/path and local metadata", () => {
    const metadata = upsertBookmarkMetadata({}, "bookmark-a", "hotspot", "track render loops");

    const byCoreFields = filterBookmarksByQuery(BOOKMARKS, "bookmarkmanager crud", metadata);
    expect(byCoreFields).toHaveLength(1);
    expect(byCoreFields[0]?.id).toBe("bookmark-b");

    const byLocalMetadata = filterBookmarksByQuery(BOOKMARKS, "hotspot loops", metadata);
    expect(byLocalMetadata).toHaveLength(1);
    expect(byLocalMetadata[0]?.id).toBe("bookmark-a");
  });
});
