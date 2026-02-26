import { useCallback, useEffect, useMemo, useState } from "react";

import { api } from "../api/client";
import type { BookmarkCategoryDto, BookmarkDto } from "../generated/api";

type BookmarkSeed = {
  nodeId: string;
  label: string;
} | null;

type BookmarkManagerProps = {
  open: boolean;
  seed: BookmarkSeed;
  onClose: () => void;
  onFocusSymbol: (nodeId: string, label: string) => void;
  onStatus: (message: string) => void;
};

const DEFAULT_CATEGORY_NAME = "General";

export function BookmarkManager({
  open,
  seed,
  onClose,
  onFocusSymbol,
  onStatus,
}: BookmarkManagerProps) {
  const [categories, setCategories] = useState<BookmarkCategoryDto[]>([]);
  const [bookmarks, setBookmarks] = useState<BookmarkDto[]>([]);
  const [selectedCategoryId, setSelectedCategoryId] = useState<string | null>(null);
  const [newCategoryName, setNewCategoryName] = useState<string>("");
  const [newBookmarkComment, setNewBookmarkComment] = useState<string>("");
  const [bookmarkCategoryId, setBookmarkCategoryId] = useState<string>("");
  const [seedNode, setSeedNode] = useState<BookmarkSeed>(null);
  const [categoryDrafts, setCategoryDrafts] = useState<Record<string, string>>({});
  const [bookmarkCommentDrafts, setBookmarkCommentDrafts] = useState<Record<string, string>>({});
  const [bookmarkCategoryDrafts, setBookmarkCategoryDrafts] = useState<Record<string, string>>({});
  const [loading, setLoading] = useState<boolean>(false);

  const visibleBookmarks = useMemo(() => {
    if (!selectedCategoryId) {
      return bookmarks;
    }
    return bookmarks.filter((bookmark) => bookmark.category_id === selectedCategoryId);
  }, [bookmarks, selectedCategoryId]);

  const refreshData = useCallback(
    async (categoryFilter: string | null = selectedCategoryId): Promise<void> => {
      setLoading(true);
      try {
        const [loadedCategories, loadedBookmarks] = await Promise.all([
          api.getBookmarkCategories(),
          api.getBookmarks(categoryFilter),
        ]);
        setCategories(loadedCategories);
        setBookmarks(loadedBookmarks);
        setCategoryDrafts(
          Object.fromEntries(loadedCategories.map((category) => [category.id, category.name])),
        );
        setBookmarkCommentDrafts(
          Object.fromEntries(
            loadedBookmarks.map((bookmark) => [bookmark.id, bookmark.comment ?? ""]),
          ),
        );
        setBookmarkCategoryDrafts(
          Object.fromEntries(
            loadedBookmarks.map((bookmark) => [bookmark.id, bookmark.category_id]),
          ),
        );

        if (!categoryFilter && loadedCategories[0] && bookmarkCategoryId.length === 0) {
          setBookmarkCategoryId(loadedCategories[0].id);
        }
      } catch (error) {
        onStatus(error instanceof Error ? error.message : "Failed to load bookmarks.");
      } finally {
        setLoading(false);
      }
    },
    [bookmarkCategoryId.length, onStatus, selectedCategoryId],
  );

  useEffect(() => {
    if (!open) {
      return;
    }
    void refreshData(selectedCategoryId);
  }, [open, refreshData, selectedCategoryId]);

  useEffect(() => {
    if (!open) {
      return;
    }
    setSeedNode(seed);
  }, [open, seed]);

  useEffect(() => {
    if (!open) {
      return;
    }
    if (seed && !seedNode) {
      setSeedNode(seed);
    }
  }, [open, seed, seedNode]);

  const ensureCategory = async (): Promise<string | null> => {
    if (categories.length > 0) {
      return bookmarkCategoryId || categories[0]?.id || null;
    }
    try {
      const created = await api.createBookmarkCategory({ name: DEFAULT_CATEGORY_NAME });
      setCategories([created]);
      setBookmarkCategoryId(created.id);
      setCategoryDrafts({ [created.id]: created.name });
      return created.id;
    } catch (error) {
      onStatus(error instanceof Error ? error.message : "Failed to create default category.");
      return null;
    }
  };

  const createCategory = async () => {
    const name = newCategoryName.trim();
    if (name.length === 0) {
      return;
    }
    try {
      const created = await api.createBookmarkCategory({ name });
      setNewCategoryName("");
      setCategories((previous) => [...previous, created]);
      setCategoryDrafts((previous) => ({ ...previous, [created.id]: created.name }));
      if (!bookmarkCategoryId) {
        setBookmarkCategoryId(created.id);
      }
      onStatus(`Created bookmark category "${created.name}".`);
    } catch (error) {
      onStatus(error instanceof Error ? error.message : "Failed to create bookmark category.");
    }
  };

  const renameCategory = async (categoryId: string) => {
    const name = (categoryDrafts[categoryId] ?? "").trim();
    if (name.length === 0) {
      return;
    }
    try {
      const updated = await api.updateBookmarkCategory(categoryId, { name });
      setCategories((previous) =>
        previous.map((category) => (category.id === categoryId ? updated : category)),
      );
      setCategoryDrafts((previous) => ({ ...previous, [categoryId]: updated.name }));
      onStatus(`Renamed category to "${updated.name}".`);
    } catch (error) {
      onStatus(error instanceof Error ? error.message : "Failed to rename category.");
    }
  };

  const removeCategory = async (categoryId: string) => {
    try {
      await api.deleteBookmarkCategory(categoryId);
      if (selectedCategoryId === categoryId) {
        setSelectedCategoryId(null);
      }
      if (bookmarkCategoryId === categoryId) {
        setBookmarkCategoryId("");
      }
      await refreshData(selectedCategoryId === categoryId ? null : selectedCategoryId);
      onStatus("Bookmark category deleted.");
    } catch (error) {
      onStatus(error instanceof Error ? error.message : "Failed to delete category.");
    }
  };

  const createBookmark = async () => {
    if (!seedNode) {
      return;
    }
    const categoryId = await ensureCategory();
    if (!categoryId) {
      return;
    }
    try {
      await api.createBookmark({
        category_id: categoryId,
        node_id: seedNode.nodeId,
        comment: newBookmarkComment.trim().length > 0 ? newBookmarkComment.trim() : null,
      });
      setNewBookmarkComment("");
      await refreshData(selectedCategoryId);
      onStatus(`Bookmarked "${seedNode.label}".`);
    } catch (error) {
      onStatus(error instanceof Error ? error.message : "Failed to create bookmark.");
    }
  };

  const saveBookmark = async (bookmark: BookmarkDto) => {
    const nextComment = (bookmarkCommentDrafts[bookmark.id] ?? "").trim();
    const nextCategory = bookmarkCategoryDrafts[bookmark.id] ?? bookmark.category_id;
    try {
      const updated = await api.updateBookmark(bookmark.id, {
        comment: nextComment.length > 0 ? nextComment : null,
        category_id: nextCategory,
      });
      setBookmarks((previous) =>
        previous.map((item) => (item.id === bookmark.id ? updated : item)),
      );
      onStatus(`Updated bookmark "${bookmark.node_label}".`);
    } catch (error) {
      onStatus(error instanceof Error ? error.message : "Failed to update bookmark.");
    }
  };

  const removeBookmark = async (bookmarkId: string) => {
    try {
      await api.deleteBookmark(bookmarkId);
      setBookmarks((previous) => previous.filter((bookmark) => bookmark.id !== bookmarkId));
      onStatus("Bookmark deleted.");
    } catch (error) {
      onStatus(error instanceof Error ? error.message : "Failed to delete bookmark.");
    }
  };

  if (!open) {
    return null;
  }

  return (
    <div className="bookmark-manager-overlay" role="presentation">
      <div className="bookmark-manager" role="dialog" aria-modal="true" aria-label="Bookmarks">
        <div className="bookmark-manager-header">
          <h3>Bookmarks</h3>
          <button type="button" onClick={onClose}>
            Close
          </button>
        </div>

        <div className="bookmark-manager-columns">
          <section className="bookmark-section">
            <h4>Categories</h4>
            <div className="bookmark-inline-row">
              <input
                value={newCategoryName}
                onChange={(event) => setNewCategoryName(event.target.value)}
                placeholder="New category name"
              />
              <button type="button" onClick={() => void createCategory()}>
                Add
              </button>
            </div>

            <div className="bookmark-category-filter">
              <button
                type="button"
                className={selectedCategoryId === null ? "bookmark-active" : ""}
                onClick={() => setSelectedCategoryId(null)}
              >
                All
              </button>
              {categories.map((category) => (
                <button
                  type="button"
                  key={category.id}
                  className={selectedCategoryId === category.id ? "bookmark-active" : ""}
                  onClick={() => setSelectedCategoryId(category.id)}
                >
                  {category.name}
                </button>
              ))}
            </div>

            <div className="bookmark-list">
              {categories.map((category) => (
                <div key={category.id} className="bookmark-category-row">
                  <input
                    value={categoryDrafts[category.id] ?? category.name}
                    onChange={(event) =>
                      setCategoryDrafts((previous) => ({
                        ...previous,
                        [category.id]: event.target.value,
                      }))
                    }
                  />
                  <button type="button" onClick={() => void renameCategory(category.id)}>
                    Save
                  </button>
                  <button type="button" onClick={() => void removeCategory(category.id)}>
                    Delete
                  </button>
                </div>
              ))}
            </div>
          </section>

          <section className="bookmark-section">
            <h4>Bookmarks</h4>
            {seedNode ? (
              <div className="bookmark-seed-card">
                <div className="bookmark-seed-title">Bookmark Node</div>
                <div className="bookmark-seed-label">{seedNode.label}</div>
                <div className="bookmark-inline-row">
                  <select
                    value={bookmarkCategoryId}
                    onChange={(event) => setBookmarkCategoryId(event.target.value)}
                  >
                    <option value="">Select category</option>
                    {categories.map((category) => (
                      <option key={category.id} value={category.id}>
                        {category.name}
                      </option>
                    ))}
                  </select>
                </div>
                <textarea
                  value={newBookmarkComment}
                  onChange={(event) => setNewBookmarkComment(event.target.value)}
                  placeholder="Bookmark comment"
                />
                <button type="button" onClick={() => void createBookmark()}>
                  Save Bookmark
                </button>
              </div>
            ) : null}

            <div className="bookmark-list">
              {loading ? <div className="bookmark-empty">Loading...</div> : null}
              {!loading && visibleBookmarks.length === 0 ? (
                <div className="bookmark-empty">No bookmarks in this category.</div>
              ) : null}
              {visibleBookmarks.map((bookmark) => (
                <div key={bookmark.id} className="bookmark-item">
                  <button
                    type="button"
                    className="bookmark-node-link"
                    onClick={() => onFocusSymbol(bookmark.node_id, bookmark.node_label)}
                  >
                    {bookmark.node_label}
                  </button>
                  <div className="bookmark-inline-row">
                    <select
                      value={bookmarkCategoryDrafts[bookmark.id] ?? bookmark.category_id}
                      onChange={(event) =>
                        setBookmarkCategoryDrafts((previous) => ({
                          ...previous,
                          [bookmark.id]: event.target.value,
                        }))
                      }
                    >
                      {categories.map((category) => (
                        <option key={category.id} value={category.id}>
                          {category.name}
                        </option>
                      ))}
                    </select>
                  </div>
                  <textarea
                    value={bookmarkCommentDrafts[bookmark.id] ?? bookmark.comment ?? ""}
                    onChange={(event) =>
                      setBookmarkCommentDrafts((previous) => ({
                        ...previous,
                        [bookmark.id]: event.target.value,
                      }))
                    }
                  />
                  <div className="bookmark-inline-row">
                    <button type="button" onClick={() => void saveBookmark(bookmark)}>
                      Save
                    </button>
                    <button type="button" onClick={() => void removeBookmark(bookmark.id)}>
                      Delete
                    </button>
                  </div>
                </div>
              ))}
            </div>
          </section>
        </div>
      </div>
    </div>
  );
}
