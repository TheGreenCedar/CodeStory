import { useMemo, useState } from "react";

import type { InvestigationSpace } from "./types";

type SpacesPanelProps = {
  spaces: InvestigationSpace[];
  activeSpaceId: string | null;
  onCreateSpace: (name: string, notes: string) => void;
  onLoadSpace: (spaceId: string) => void;
  onDeleteSpace: (spaceId: string) => void;
};

export function SpacesPanel({
  spaces,
  activeSpaceId,
  onCreateSpace,
  onLoadSpace,
  onDeleteSpace,
}: SpacesPanelProps) {
  const [nameDraft, setNameDraft] = useState<string>("");
  const [notesDraft, setNotesDraft] = useState<string>("");
  const [query, setQuery] = useState<string>("");

  const visibleSpaces = useMemo(() => {
    const normalizedQuery = query.trim().toLowerCase();
    if (normalizedQuery.length === 0) {
      return spaces;
    }
    return spaces.filter((space) =>
      `${space.name} ${space.prompt} ${space.notes ?? ""} ${space.owner}`
        .toLowerCase()
        .includes(normalizedQuery),
    );
  }, [query, spaces]);

  return (
    <section className="spaces-panel">
      <header className="spaces-panel-header">
        <h3>Spaces</h3>
        <p>Save current context so you can reopen it fast.</p>
      </header>

      <div className="spaces-create">
        <input
          value={nameDraft}
          onChange={(event) => setNameDraft(event.target.value)}
          placeholder="Space name"
          aria-label="Space name"
        />
        <textarea
          value={notesDraft}
          onChange={(event) => setNotesDraft(event.target.value)}
          placeholder="Notes (optional)"
          aria-label="Space notes"
        />
        <button
          type="button"
          onClick={() => {
            onCreateSpace(nameDraft, notesDraft);
            setNameDraft("");
            setNotesDraft("");
          }}
        >
          Save Space
        </button>
      </div>

      <div className="spaces-filter-row">
        <input
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="Search spaces"
          aria-label="Filter spaces"
        />
      </div>

      <div className="spaces-list">
        {visibleSpaces.length === 0 ? (
          <div className="spaces-empty">No spaces yet. Save the current investigation.</div>
        ) : (
          visibleSpaces.map((space) => (
            <article
              key={space.id}
              className={
                activeSpaceId === space.id ? "spaces-item spaces-item-active" : "spaces-item"
              }
            >
              <h4>{space.name}</h4>
              <p>{space.prompt}</p>
              {space.notes ? <p className="spaces-item-notes">{space.notes}</p> : null}
              <div className="spaces-item-meta">
                <span>{new Date(space.updatedAt).toLocaleString()}</span>
                <span>{space.owner}</span>
              </div>
              <div className="spaces-item-actions">
                <button type="button" onClick={() => onLoadSpace(space.id)}>
                  Open
                </button>
                <button type="button" onClick={() => onDeleteSpace(space.id)}>
                  Delete
                </button>
              </div>
            </article>
          ))
        )}
      </div>
    </section>
  );
}
