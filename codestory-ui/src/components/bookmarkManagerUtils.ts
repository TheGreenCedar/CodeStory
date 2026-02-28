import type { BookmarkDto } from "../generated/api";

export type BookmarkLocalMetadata = {
  tags: string[];
  notesTemplate: string;
  updatedAt: string;
};

export type BookmarkLocalMetadataMap = Record<string, BookmarkLocalMetadata>;

export const BOOKMARK_METADATA_STORAGE_KEY = "codestory:bookmark-metadata:v1";

function getStorage(): Storage | null {
  if (typeof window === "undefined") {
    return null;
  }
  return window.localStorage;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function parseBookmarkTags(value: string): string[] {
  const segments = value
    .split(",")
    .map((segment) => segment.trim())
    .filter((segment) => segment.length > 0);
  const seen = new Set<string>();
  const tags: string[] = [];
  for (const segment of segments) {
    const key = segment.toLocaleLowerCase();
    if (seen.has(key)) {
      continue;
    }
    seen.add(key);
    tags.push(segment);
  }
  return tags;
}

export function bookmarkTagsToDraft(tags: string[]): string {
  return tags.join(", ");
}

function normalizeMetadata(value: unknown): BookmarkLocalMetadata | null {
  if (!isRecord(value)) {
    return null;
  }

  const tags = Array.isArray(value.tags)
    ? value.tags
        .filter((item): item is string => typeof item === "string")
        .map((item) => item.trim())
        .filter((item) => item.length > 0)
    : [];
  const notesTemplate = typeof value.notesTemplate === "string" ? value.notesTemplate.trim() : "";
  const updatedAt =
    typeof value.updatedAt === "string" ? value.updatedAt : new Date().toISOString();
  if (tags.length === 0 && notesTemplate.length === 0) {
    return null;
  }
  return {
    tags,
    notesTemplate,
    updatedAt,
  };
}

export function loadBookmarkMetadataMap(): BookmarkLocalMetadataMap {
  const storage = getStorage();
  if (!storage) {
    return {};
  }

  const raw = storage.getItem(BOOKMARK_METADATA_STORAGE_KEY);
  if (!raw || raw.trim().length === 0) {
    return {};
  }

  try {
    const parsed = JSON.parse(raw) as unknown;
    if (!isRecord(parsed)) {
      return {};
    }
    const entries = Object.entries(parsed)
      .map(([bookmarkId, value]) => {
        const metadata = normalizeMetadata(value);
        if (!metadata) {
          return null;
        }
        return [bookmarkId, metadata] as const;
      })
      .filter((entry): entry is readonly [string, BookmarkLocalMetadata] => entry !== null);

    return Object.fromEntries(entries);
  } catch {
    return {};
  }
}

export function saveBookmarkMetadataMap(map: BookmarkLocalMetadataMap): void {
  const storage = getStorage();
  if (!storage) {
    return;
  }
  storage.setItem(BOOKMARK_METADATA_STORAGE_KEY, JSON.stringify(map));
}

export function upsertBookmarkMetadata(
  map: BookmarkLocalMetadataMap,
  bookmarkId: string,
  tagsDraft: string,
  notesTemplateDraft: string,
): BookmarkLocalMetadataMap {
  const tags = parseBookmarkTags(tagsDraft);
  const notesTemplate = notesTemplateDraft.trim();
  const next = { ...map };
  if (tags.length === 0 && notesTemplate.length === 0) {
    delete next[bookmarkId];
    return next;
  }

  next[bookmarkId] = {
    tags,
    notesTemplate,
    updatedAt: new Date().toISOString(),
  };
  return next;
}

export function removeBookmarkMetadata(
  map: BookmarkLocalMetadataMap,
  bookmarkId: string,
): BookmarkLocalMetadataMap {
  if (!(bookmarkId in map)) {
    return map;
  }
  const next = { ...map };
  delete next[bookmarkId];
  return next;
}

export function filterBookmarksByQuery(
  bookmarks: BookmarkDto[],
  query: string,
  metadataMap: BookmarkLocalMetadataMap,
): BookmarkDto[] {
  const terms = query
    .toLocaleLowerCase()
    .trim()
    .split(/\s+/)
    .filter((term) => term.length > 0);
  if (terms.length === 0) {
    return bookmarks;
  }

  return bookmarks.filter((bookmark) => {
    const metadata = metadataMap[bookmark.id];
    const haystack = [
      bookmark.node_label,
      bookmark.comment ?? "",
      bookmark.file_path ?? "",
      metadata?.tags.join(" ") ?? "",
      metadata?.notesTemplate ?? "",
    ]
      .join("\n")
      .toLocaleLowerCase();
    return terms.every((term) => haystack.includes(term));
  });
}
