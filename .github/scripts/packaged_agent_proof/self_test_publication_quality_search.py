"""Hermetic publication quality-search tests for legacy and schema-3 shapes."""

from __future__ import annotations

import hashlib
from pathlib import Path
from unittest.mock import patch

from . import publication_protocol
from .foundation import ProofFailure, require
from .subprocess_control import (
    quality_search_hit_matches,
    resolve_quality_search_hits,
)

_ANCHOR = "qualification_anchor_00"
_OTHER = "qualification_anchor_01"


def _legacy_search(*, display_name: str = _ANCHOR) -> dict:
    return {
        "query": display_name,
        "indexed_symbol_hits": [
            {"display_name": _OTHER, "node_id": "node-other"},
            {"display_name": display_name, "node_id": "node-anchor"},
        ],
        "repo_text_hits": [],
    }


def _schema3_search(*, excerpt: object = f"pub fn {_ANCHOR}() -> &'static str {{}}") -> dict:
    return {
        "kind": "complete",
        "schema_version": 3,
        "status": "available",
        "evidence": [
            {
                "identity": {"evidence_id": "search-0-other"},
                "path": "lib.rs",
                "symbol_id": "node-other",
                "start_line": 1,
                "end_line": 1,
                "excerpt": f"pub fn {_OTHER}() -> &'static str {{}}",
            },
            {
                "identity": {"evidence_id": "search-1-anchor"},
                "path": "lib.rs",
                "symbol_id": "node-anchor",
                "start_line": 2,
                "end_line": 2,
                "excerpt": excerpt,
            },
        ],
        "retrieval": {"state": "full", "generation_id": "retrieval-self-test"},
        "gaps": [],
        "continuation": None,
        "diagnostics": {"availability": "unavailable"},
    }


def _legacy_hits_resolve_test() -> None:
    hits = resolve_quality_search_hits(_legacy_search())
    require(
        isinstance(hits, list) and len(hits) == 2,
        f"legacy indexed_symbol_hits resolve drifted: {hits!r}",
    )
    require(
        quality_search_hit_matches(hits[1], _ANCHOR),
        f"legacy display_name match failed: {hits[1]!r}",
    )
    require(
        not quality_search_hit_matches(hits[0], _ANCHOR),
        f"legacy non-anchor hit incorrectly matched: {hits[0]!r}",
    )


def _schema3_evidence_resolve_test() -> None:
    search = _schema3_search()
    require(
        "indexed_symbol_hits" not in search,
        "schema-3 fixture accidentally included legacy indexed_symbol_hits",
    )
    hits = resolve_quality_search_hits(search)
    require(
        isinstance(hits, list) and len(hits) == 2,
        f"schema-3 evidence resolve drifted: {hits!r}",
    )
    require(
        quality_search_hit_matches(hits[1], _ANCHOR),
        f"schema-3 excerpt match failed: {hits[1]!r}",
    )
    require(
        not quality_search_hit_matches(hits[0], _ANCHOR),
        f"schema-3 non-anchor evidence incorrectly matched: {hits[0]!r}",
    )


def _hostile_resolve_tests() -> None:
    hostile_cases = [
        (
            "non-object payload",
            ["not", "an", "object"],
            "non-object projection",
        ),
        (
            "legacy missing indexed_symbol_hits",
            {"query": _ANCHOR, "repo_text_hits": []},
            "omitted indexed symbol hits",
        ),
        (
            "legacy non-array indexed_symbol_hits",
            {"indexed_symbol_hits": {"display_name": _ANCHOR}},
            "omitted indexed symbol hits",
        ),
        (
            "schema3 incomplete kind",
            {**_schema3_search(), "kind": "budget_exceeded"},
            "complete schema-3 evidence projection",
        ),
        (
            "schema3 non-array evidence",
            {**_schema3_search(), "evidence": {"path": "lib.rs"}},
            "non-array evidence",
        ),
    ]
    for label, payload, expected_diagnostic in hostile_cases:
        try:
            resolve_quality_search_hits(payload)  # type: ignore[arg-type]
        except ProofFailure as exc:
            require(
                expected_diagnostic in str(exc),
                f"{label} failure omitted its diagnostics: {exc}",
            )
        else:
            raise ProofFailure(
                f"{label} search projection was accepted as quality-search hits"
            )


def _rank_from_payload(payload: dict, expected: str = _ANCHOR) -> int | None:
    stdout = '{"self_test":true}\n'
    with patch.object(
        publication_protocol,
        "json_command",
        return_value=({"stdout": stdout}, payload),
    ):
        rank, digest = publication_protocol.run_quality_search(
            cli=Path("/self-test/codestory-cli"),
            env={},
            project=Path("/self-test/project"),
            run_id="publication-qualification",
            query=expected,
            expected=expected,
            timeout=1,
        )
    require(
        digest == hashlib.sha256(stdout.encode("utf-8")).hexdigest(),
        f"quality-search digest drifted for payload={payload!r}",
    )
    return rank


def _legacy_rank_test() -> None:
    rank = _rank_from_payload(_legacy_search())
    require(rank == 2, f"legacy quality-search rank drifted: {rank!r}")


def _schema3_rank_test() -> None:
    rank = _rank_from_payload(_schema3_search())
    require(rank == 2, f"schema-3 quality-search rank drifted: {rank!r}")


def _schema3_miss_rank_test() -> None:
    rank = _rank_from_payload(_schema3_search(excerpt="pub fn unrelated() {}"))
    require(rank is None, f"schema-3 miss unexpectedly ranked: {rank!r}")


def run_publication_quality_search_self_tests() -> None:
    _legacy_hits_resolve_test()
    _schema3_evidence_resolve_test()
    _hostile_resolve_tests()
    _legacy_rank_test()
    _schema3_rank_test()
    _schema3_miss_rank_test()
