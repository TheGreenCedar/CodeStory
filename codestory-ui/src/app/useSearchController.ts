import {
  useCallback,
  useEffect,
  useRef,
  type Dispatch,
  type KeyboardEvent,
  type SetStateAction,
} from "react";

import { api } from "../api/client";
import type { SearchHit } from "../generated/api";
import type { PendingSymbolFocus } from "./types";

type UseSearchControllerArgs = {
  projectOpen: boolean;
  searchQuery: string;
  searchHits: SearchHit[];
  searchIndex: number;
  isDirty: boolean;
  activeFilePath: string | null;
  activeNodeId: string | null;
  focusSymbolInternal: (
    symbolId: string,
    label: string,
    graphMode?: "neighborhood" | "trailDepthOne",
  ) => Promise<void>;
  updateTrailConfig: (patch: { showLegend?: boolean }) => void;
  setSearchHits: Dispatch<SetStateAction<SearchHit[]>>;
  setSearchOpen: Dispatch<SetStateAction<boolean>>;
  setSearchIndex: Dispatch<SetStateAction<number>>;
  setIsSearching: Dispatch<SetStateAction<boolean>>;
  setPendingFocus: Dispatch<SetStateAction<PendingSymbolFocus | null>>;
  setStatus: Dispatch<SetStateAction<string>>;
  setSearchQuery: Dispatch<SetStateAction<string>>;
};

export type SearchController = {
  activateSearchHit: (hit: SearchHit) => void;
  handleSearchKeyDown: (event: KeyboardEvent<HTMLInputElement>) => void;
};

export function useSearchController({
  projectOpen,
  searchQuery,
  searchHits,
  searchIndex,
  isDirty,
  activeFilePath,
  activeNodeId,
  focusSymbolInternal,
  updateTrailConfig,
  setSearchHits,
  setSearchOpen,
  setSearchIndex,
  setIsSearching,
  setPendingFocus,
  setStatus,
  setSearchQuery,
}: UseSearchControllerArgs): SearchController {
  const searchSeqRef = useRef<number>(0);

  useEffect(() => {
    const query = searchQuery.trim();
    if (!projectOpen || query.length < 2) {
      searchSeqRef.current += 1;
      setIsSearching(false);
      setSearchHits([]);
      setSearchOpen(false);
      setSearchIndex(0);
      return;
    }

    const sequence = searchSeqRef.current + 1;
    searchSeqRef.current = sequence;
    setIsSearching(true);

    const timer = window.setTimeout(() => {
      void api
        .search({ query })
        .then((hits) => {
          if (sequence !== searchSeqRef.current) {
            return;
          }
          setSearchHits(hits.slice(0, 14));
          setSearchOpen(true);
          setSearchIndex(0);
        })
        .catch((error) => {
          if (sequence !== searchSeqRef.current) {
            return;
          }
          setSearchHits([]);
          setSearchOpen(false);
          setStatus(error instanceof Error ? error.message : "Search failed.");
        })
        .finally(() => {
          if (sequence === searchSeqRef.current) {
            setIsSearching(false);
          }
        });
    }, 220);

    return () => {
      window.clearTimeout(timer);
    };
  }, [
    projectOpen,
    searchQuery,
    setIsSearching,
    setSearchHits,
    setSearchIndex,
    setSearchOpen,
    setStatus,
  ]);

  const activateSearchHit = useCallback(
    (hit: SearchHit) => {
      setSearchOpen(false);
      setSearchQuery(hit.display_name);
      if (isDirty && activeFilePath && activeNodeId !== hit.node_id) {
        setPendingFocus({
          symbolId: hit.node_id,
          label: hit.display_name,
          graphMode: "trailDepthOne",
        });
        return;
      }
      void focusSymbolInternal(hit.node_id, hit.display_name, "trailDepthOne");
    },
    [
      activeFilePath,
      activeNodeId,
      focusSymbolInternal,
      isDirty,
      setPendingFocus,
      setSearchOpen,
      setSearchQuery,
    ],
  );

  const handleSearchKeyDown = useCallback(
    (event: KeyboardEvent<HTMLInputElement>) => {
      if (event.key === "Enter" && searchQuery.trim().toLowerCase() === "legend") {
        event.preventDefault();
        updateTrailConfig({ showLegend: true });
        setSearchOpen(false);
        setStatus("Legend opened.");
        return;
      }

      if (event.key === "ArrowDown") {
        event.preventDefault();
        if (searchHits.length > 0) {
          setSearchOpen(true);
          setSearchIndex((previous) => Math.min(previous + 1, searchHits.length - 1));
        }
        return;
      }

      if (event.key === "ArrowUp") {
        event.preventDefault();
        if (searchHits.length > 0) {
          setSearchOpen(true);
          setSearchIndex((previous) => Math.max(previous - 1, 0));
        }
        return;
      }

      if (event.key === "Enter") {
        if (searchHits.length > 0) {
          event.preventDefault();
          const selected =
            searchHits[Math.min(searchIndex, searchHits.length - 1)] ?? searchHits[0];
          if (selected) {
            activateSearchHit(selected);
          }
        }
        return;
      }

      if (event.key === "Escape") {
        setSearchOpen(false);
      }
    },
    [
      activateSearchHit,
      searchHits,
      searchIndex,
      searchQuery,
      setSearchIndex,
      setSearchOpen,
      setStatus,
      updateTrailConfig,
    ],
  );

  return {
    activateSearchHit,
    handleSearchKeyDown,
  };
}
