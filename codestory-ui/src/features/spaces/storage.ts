import type {
  CreateInvestigationSpaceInput,
  InvestigationSpace,
  UpdateInvestigationSpaceInput,
} from "./types";

export const INVESTIGATION_SPACES_STORAGE_KEY = "codestory:investigation-spaces:v1";

function getStorage(): Storage | null {
  if (typeof window === "undefined") {
    return null;
  }
  return window.localStorage;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function normalizeOptionalText(value: unknown): string | undefined {
  if (typeof value !== "string") {
    return undefined;
  }
  const next = value.trim();
  return next.length > 0 ? next : undefined;
}

function normalizeText(value: unknown): string | null {
  if (typeof value !== "string") {
    return null;
  }
  return value.trim();
}

function normalizeNullableText(value: unknown): string | null {
  if (typeof value !== "string") {
    return null;
  }
  const next = value.trim();
  return next.length > 0 ? next : null;
}

function createId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  return `space-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 9)}`;
}

function normalizeSpace(value: unknown): InvestigationSpace | null {
  if (!isRecord(value)) {
    return null;
  }

  const id = normalizeOptionalText(value.id);
  const name = normalizeOptionalText(value.name) ?? "Untitled Space";
  const prompt = normalizeText(value.prompt);
  const owner = normalizeOptionalText(value.owner) ?? "unknown";
  const activeGraphId = normalizeNullableText(value.activeGraphId);
  const activeSymbolId = normalizeNullableText(value.activeSymbolId);
  const notes = normalizeOptionalText(value.notes);
  const createdAt = normalizeOptionalText(value.createdAt);
  const updatedAt = normalizeOptionalText(value.updatedAt);

  if (!id || prompt === null || !createdAt || !updatedAt) {
    return null;
  }

  return {
    id,
    name,
    prompt,
    activeGraphId,
    activeSymbolId,
    notes,
    createdAt,
    updatedAt,
    owner,
  };
}

function parseSpaces(raw: string | null): InvestigationSpace[] {
  if (!raw || raw.trim().length === 0) {
    return [];
  }

  try {
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) {
      return [];
    }

    return parsed
      .map((item) => normalizeSpace(item))
      .filter((item): item is InvestigationSpace => item !== null);
  } catch {
    return [];
  }
}

function writeSpaces(spaces: InvestigationSpace[]): void {
  const storage = getStorage();
  if (!storage) {
    return;
  }
  storage.setItem(INVESTIGATION_SPACES_STORAGE_KEY, JSON.stringify(spaces));
}

export function loadSpaces(): InvestigationSpace[] {
  const storage = getStorage();
  if (!storage) {
    return [];
  }
  return parseSpaces(storage.getItem(INVESTIGATION_SPACES_STORAGE_KEY));
}

export function listSpaces(): InvestigationSpace[] {
  return [...loadSpaces()].sort((left, right) => right.updatedAt.localeCompare(left.updatedAt));
}

export function loadSpace(id: string): InvestigationSpace | null {
  return listSpaces().find((space) => space.id === id) ?? null;
}

export function createSpace(input: CreateInvestigationSpaceInput): InvestigationSpace {
  const now = new Date().toISOString();
  const name = input.name.trim();
  const prompt = input.prompt.trim();
  const owner = input.owner.trim();

  const created: InvestigationSpace = {
    id: input.id?.trim() || createId(),
    name: name.length > 0 ? name : "Untitled Space",
    prompt,
    activeGraphId: input.activeGraphId?.trim() || null,
    activeSymbolId: input.activeSymbolId?.trim() || null,
    notes: normalizeOptionalText(input.notes),
    createdAt: now,
    updatedAt: now,
    owner: owner.length > 0 ? owner : "unknown",
  };

  const spaces = loadSpaces();
  writeSpaces([...spaces, created]);
  return created;
}

export function updateSpace(
  id: string,
  updates: UpdateInvestigationSpaceInput,
): InvestigationSpace | null {
  const spaces = loadSpaces();
  const index = spaces.findIndex((space) => space.id === id);
  if (index === -1) {
    return null;
  }

  const current = spaces[index];
  if (!current) {
    return null;
  }

  const updated: InvestigationSpace = {
    ...current,
    name: updates.name?.trim() || current.name,
    prompt: updates.prompt?.trim() || current.prompt,
    activeGraphId:
      updates.activeGraphId === undefined
        ? current.activeGraphId
        : updates.activeGraphId?.trim() || null,
    activeSymbolId:
      updates.activeSymbolId === undefined
        ? current.activeSymbolId
        : updates.activeSymbolId?.trim() || null,
    notes:
      updates.notes === undefined
        ? current.notes
        : normalizeOptionalText(updates.notes ?? undefined),
    owner: updates.owner?.trim() || current.owner,
    updatedAt: new Date().toISOString(),
  };

  const next = [...spaces];
  next[index] = updated;
  writeSpaces(next);
  return updated;
}

export function deleteSpace(id: string): boolean {
  const spaces = loadSpaces();
  const next = spaces.filter((space) => space.id !== id);
  if (next.length === spaces.length) {
    return false;
  }
  writeSpaces(next);
  return true;
}
