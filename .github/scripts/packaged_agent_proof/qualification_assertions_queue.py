"""Mixed-queue qualification assertions."""

from __future__ import annotations

from .foundation import HEX_SHA256
from .qualification_scenario_evidence import ScenarioAssertionEvidence

def _mixed_queue_capacity_is_typed(capacity: dict) -> bool:
    for queue_class in ("query", "bulk"):
        record = capacity[f"{queue_class}_65th"]
        pressure = (
            record.get("error", {}).get("capacity")
            if isinstance(record, dict)
            else None
        )
        if not (
            isinstance(pressure, dict)
            and pressure.get("queue_class") == queue_class
            and pressure.get("capacity") == 64
            and pressure.get("depth") == 64
            and bool(pressure.get("retry_condition"))
        ):
            return False
    return True


def _mixed_queue_is_fifo(class_orders: dict) -> bool:
    return all(
        class_orders[f"{queue_class}_expected_queue_insertion_request_ids"]
        == class_orders[f"{queue_class}_native_completed_request_ids"]
        and isinstance(
            class_orders[
                f"{queue_class}_expected_queue_insertion_request_ids"
            ],
            list,
        )
        and bool(
            class_orders[
                f"{queue_class}_expected_queue_insertion_request_ids"
            ]
        )
        and isinstance(
            class_orders[f"{queue_class}_native_completion_sequences"],
            list,
        )
        and bool(class_orders[f"{queue_class}_native_completion_sequences"])
        and all(
            isinstance(sequence, int)
            and not isinstance(sequence, bool)
            and sequence > 0
            for sequence in class_orders[
                f"{queue_class}_native_completion_sequences"
            ]
        )
        and class_orders[f"{queue_class}_native_completion_sequences"]
        == sorted(class_orders[f"{queue_class}_native_completion_sequences"])
        and len(
            set(class_orders[f"{queue_class}_native_completion_sequences"])
        )
        == len(class_orders[f"{queue_class}_native_completion_sequences"])
        for queue_class in ("query", "bulk")
    )


def _mixed_queue_preserves_project_order(project_orders: dict) -> bool:
    return all(
        project_orders[
            f"{queue_class}_expected_queue_insertion_project_identities"
        ]
        == project_orders[f"{queue_class}_native_completed_project_identities"]
        and len(
            set(
                project_orders[
                    f"{queue_class}_expected_queue_insertion_project_identities"
                ]
            )
        )
        == 2
        for queue_class in ("query", "bulk")
    )


def _mixed_queue_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    saturated = evidence.scheduler("queues_saturated")
    selected = evidence.scheduler("query_selected_before_bulk_backlog")
    capacity = evidence.transition(
        "typed_capacity_retry_observed",
        {"query_65th", "bulk_65th"},
    )
    class_orders = evidence.transition(
        "per_class_fifo_observed",
        {
            "query_expected_queue_insertion_request_ids",
            "query_native_completed_request_ids",
            "query_native_completion_sequences",
            "bulk_expected_queue_insertion_request_ids",
            "bulk_native_completed_request_ids",
            "bulk_native_completion_sequences",
        },
    )
    project_orders = evidence.transition(
        "global_fifo_across_projects",
        {
            "query_expected_queue_insertion_project_identities",
            "query_native_completed_project_identities",
            "bulk_expected_queue_insertion_project_identities",
            "bulk_native_completed_project_identities",
        },
    )
    preference = evidence.transition(
        "query_preference_observed",
        {
            "first_query_request_id",
            "first_query_native_completion_sequence",
            "first_bulk_request_id",
            "first_bulk_native_completion_sequence",
        },
    )
    resumed = evidence.transition(
        "bulk_resumed",
        {
            "last_query_request_id",
            "last_query_native_completion_sequence",
            "last_bulk_request_id",
            "last_bulk_native_completion_sequence",
        },
    )
    return {
        "query_and_bulk_capacities_are_64": (
            saturated["query_capacity"] == saturated["query_depth"] == 64
            and saturated["bulk_capacity"] == saturated["bulk_depth"] == 64
        ),
        "fifo_within_each_class": _mixed_queue_is_fifo(class_orders),
        "query_preferred_between_bulk_batches": (
            selected["active_request_class"] == "query"
            and selected["bulk_depth"] > 0
            and preference["first_query_native_completion_sequence"]
            < preference["first_bulk_native_completion_sequence"]
        ),
        "bulk_resumes_when_query_queue_permits": (
            resumed["last_bulk_native_completion_sequence"]
            > resumed["last_query_native_completion_sequence"]
        ),
        "no_project_or_scope_round_robin": (
            _mixed_queue_preserves_project_order(project_orders)
        ),
        "typed_retry_names_useful_condition": (
            _mixed_queue_capacity_is_typed(capacity)
        ),
        "no_project_or_request_text_leakage": all(
            all(HEX_SHA256.fullmatch(str(value)) is not None for value in values)
            for key, values in project_orders.items()
            if key.endswith("_project_identities")
        ),
    }
