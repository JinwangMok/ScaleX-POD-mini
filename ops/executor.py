"""
ops/executor.py — ScaleX-POD-mini P2 Operational Hardening

Dependency-graph-enforced task executor with:

  1. Topological sort of causal edges → execution order; blocked tasks never start
     until all causal predecessors have verdict PASS.

  2. Staleness detection on evidential edges → before executing any task, check
     each evidential predecessor's latest evidence.  If stale (> TTL), emit a
     RECHECK_TRIGGERED structured-log event and re-execute the predecessor.

  3. Dry-run mode → no user-supplied action is invoked; all tasks complete with
     PASS (simulated).  Both BLOCK and RECHECK events are captured in the
     structured event log as real enforcement decisions.

Structured log events (all serialisable as JSON dicts):

  {
    "event":     "TASK_STARTED" | "TASK_PASSED" | "TASK_FAILED"
               | "TASK_BLOCKED" | "RECHECK_TRIGGERED" | "BLOCK_RESOLVED"
               | "STALENESS_DETECTED" | "DRY_RUN_SUMMARY",
    "task_id":  <str>,
    "timestamp": <ISO-8601 string>,
    "detail":   <str>   # human-readable context
    ...                 # event-specific extra fields
  }
"""

from __future__ import annotations

import json
import time
from collections import defaultdict
from datetime import datetime, timezone
from typing import Callable

from .dep_graph import DepGraph, EdgeType
from .task_model import DegradationItem, Evidence, Task, Verdict


# ---------------------------------------------------------------------------
# Event definitions
# ---------------------------------------------------------------------------

def _now_iso() -> str:
    return datetime.now(tz=timezone.utc).isoformat()


def _evt(event: str, task_id: str, detail: str, **extra) -> dict:
    return {
        "event": event,
        "task_id": task_id,
        "timestamp": _now_iso(),
        "detail": detail,
        **extra,
    }


# ---------------------------------------------------------------------------
# Executor
# ---------------------------------------------------------------------------

class Executor:
    """
    Runs a set of Tasks according to a DepGraph, enforcing:
      - causal deps block execution
      - evidential deps trigger re-check when stale

    Parameters:
        graph:             DepGraph instance (tasks + edges)
        tasks:             dict[task_id → Task]
        degradation_inventory: list[DegradationItem] for known-acceptable-degradation
        action_fn:         Optional callable(task) → None that performs the real work.
                           Receives the Task object; should set task.verdict and
                           call task.add_evidence() with captured evidence.
                           If None, dry-run mode is assumed.
        dry_run:           If True (or action_fn is None), simulate execution:
                           tasks are marked PASS with synthetic evidence.
        max_recheck_depth: Safety limit — how many recursive re-checks per task.
    """

    def __init__(
        self,
        graph: DepGraph,
        tasks: dict[str, Task],
        degradation_inventory: list[DegradationItem] | None = None,
        action_fn: Callable[[Task], None] | None = None,
        dry_run: bool = False,
        max_recheck_depth: int = 3,
    ) -> None:
        self.graph = graph
        self.tasks = tasks
        self.degradation_inventory = degradation_inventory or []
        self.action_fn = action_fn
        self.dry_run = dry_run or (action_fn is None)
        self.max_recheck_depth = max_recheck_depth

        self.event_log: list[dict] = []
        self._recheck_depth: dict[str, int] = defaultdict(int)
        self._executed_ids: set[str] = set()

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def run(self) -> list[dict]:
        """
        Execute all tasks in topological order, enforcing causal and evidential deps.

        Returns the structured event log (list of JSON-serialisable dicts).
        """
        order = self.graph.topological_sort()
        self._log(_evt(
            "EXECUTION_STARTED",
            task_id="__executor__",
            detail=f"Topological order: {order}",
            dry_run=self.dry_run,
            task_count=len(order),
        ))

        for task_id in order:
            self._run_task(task_id)

        self._emit_summary(order)
        return self.event_log

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    def _run_task(self, task_id: str, *, _recheck_caller: str | None = None) -> None:
        """
        Attempt to run a single task, enforcing all dep constraints.

        If the task already has verdict PASS from a prior execution cycle (it
        was seeded externally or ran in a previous pass), we treat it as
        "previously completed" — we register it as known but do NOT re-execute
        it during the normal topological scan.  The RECHECK path will reset
        verdict to PENDING before calling _run_task again, so the second call
        goes through the full execution path.
        """
        task = self.tasks[task_id]
        task.validate()

        # Short-circuit: task already completed in a prior cycle (externally seeded
        # PASS or already ran this session via the RECHECK path).
        if task.verdict == Verdict.PASS and task_id not in self._executed_ids and _recheck_caller is None:
            # Register as known-complete so causal-dep checks pass, but do NOT
            # re-execute.  Evidential freshness will be checked by the downstream
            # task when it calls _stale_evidential_preds().
            self._executed_ids.add(task_id)
            return

        # Already done this session and passed — nothing to do.
        if task_id in self._executed_ids and task.verdict == Verdict.PASS and _recheck_caller is None:
            return

        # 1. Causal-dep enforcement: all causal predecessors must be PASS
        blocked_by = self._causal_blockers(task_id)
        if blocked_by:
            self._log(_evt(
                "TASK_BLOCKED",
                task_id=task_id,
                detail=(
                    f"Task {task_id!r} blocked; causal predecessors not yet PASS: "
                    f"{blocked_by}"
                ),
                blocked_by=blocked_by,
                scope_boundary=task.scope_boundary,
            ))
            task.verdict = Verdict.BLOCKED
            return

        # 2. Evidential-dep enforcement: each evidential predecessor must have fresh evidence
        stale_preds = self._stale_evidential_preds(task_id)
        for pred_id in stale_preds:
            pred_task = self.tasks[pred_id]
            ev = pred_task.latest_evidence()
            age = ev.age_seconds() if ev else float("inf")

            self._log(_evt(
                "STALENESS_DETECTED",
                task_id=task_id,
                detail=(
                    f"Evidential predecessor {pred_id!r} has stale evidence "
                    f"(age={age:.1f}s, TTL={pred_task.evidence_ttl_seconds}s); "
                    f"recheck required before executing {task_id!r}"
                ),
                predecessor_id=pred_id,
                evidence_age_seconds=round(age, 2),
                evidence_ttl_seconds=pred_task.evidence_ttl_seconds,
            ))
            self._log(_evt(
                "RECHECK_TRIGGERED",
                task_id=pred_id,
                detail=f"Re-executing {pred_id!r} to refresh stale evidence (triggered by {task_id!r})",
                triggered_by=task_id,
                recheck_depth=self._recheck_depth[pred_id] + 1,
            ))

            depth = self._recheck_depth[pred_id]
            if depth >= self.max_recheck_depth:
                self._log(_evt(
                    "RECHECK_LIMIT_REACHED",
                    task_id=pred_id,
                    detail=(
                        f"Recheck depth limit ({self.max_recheck_depth}) reached "
                        f"for {pred_id!r}; marking STALE_EVIDENCE"
                    ),
                    max_recheck_depth=self.max_recheck_depth,
                ))
                pred_task.verdict = Verdict.STALE_EVIDENCE
                task.verdict = Verdict.BLOCKED
                return

            self._recheck_depth[pred_id] += 1
            # Reset predecessor so it can be re-executed
            pred_task.verdict = Verdict.PENDING
            self._executed_ids.discard(pred_id)
            self._run_task(pred_id, _recheck_caller=task_id)

            if pred_task.verdict != Verdict.PASS:
                self._log(_evt(
                    "RECHECK_FAILED",
                    task_id=pred_id,
                    detail=f"Recheck of {pred_id!r} failed with verdict {pred_task.verdict}; blocking {task_id!r}",
                    triggered_by=task_id,
                    verdict=pred_task.verdict,
                ))
                task.verdict = Verdict.BLOCKED
                return

            self._log(_evt(
                "RECHECK_PASSED",
                task_id=pred_id,
                detail=f"Recheck of {pred_id!r} passed; {task_id!r} may proceed",
                triggered_by=task_id,
            ))

        # 3. Execute the task
        self._log(_evt(
            "TASK_STARTED",
            task_id=task_id,
            detail=f"Starting task {task_id!r} | scope: {task.scope_boundary}",
            scope_boundary=task.scope_boundary,
            dry_run=self.dry_run,
        ))
        task.verdict = Verdict.RUNNING

        if self.dry_run:
            self._dry_run_execute(task)
        else:
            self._real_execute(task)

        self._executed_ids.add(task_id)

        if task.verdict == Verdict.PASS:
            self._log(_evt(
                "TASK_PASSED",
                task_id=task_id,
                detail=f"Task {task_id!r} passed",
                known_acceptable_degradation=task.known_acceptable_degradation_ids,
                evidence_sources=[e.source for e in task.evidence],
            ))
        elif task.verdict == Verdict.FAIL:
            self._check_degradation_exemption(task)
        else:
            self._log(_evt(
                "TASK_FAILED",
                task_id=task_id,
                detail=f"Task {task_id!r} ended with verdict {task.verdict}",
                verdict=task.verdict,
            ))

    def _causal_blockers(self, task_id: str) -> list[str]:
        """Return causal predecessors whose verdict is not PASS."""
        return [
            pred_id
            for pred_id in self.graph.causal_predecessors(task_id)
            if self.tasks[pred_id].verdict != Verdict.PASS
        ]

    def _stale_evidential_preds(self, task_id: str) -> list[str]:
        """Return evidential predecessors with stale or absent evidence."""
        stale = []
        for pred_id in self.graph.evidential_predecessors(task_id):
            pred = self.tasks[pred_id]
            if not pred.evidence_is_fresh():
                stale.append(pred_id)
        return stale

    def _dry_run_execute(self, task: Task) -> None:
        """Simulate task execution: inject synthetic evidence, mark PASS."""
        synthetic_output = (
            f"[DRY-RUN] task={task.id!r} scope={task.scope_boundary!r} "
            f"simulated_at={_now_iso()}"
        )
        task.add_evidence(
            raw_output=synthetic_output,
            source="dry-run-simulator",
        )
        task.verdict = Verdict.PASS

    def _real_execute(self, task: Task) -> None:
        """Invoke user-supplied action_fn; propagate exceptions as FAIL."""
        try:
            self.action_fn(task)
        except Exception as exc:  # noqa: BLE001
            task.add_evidence(
                raw_output=f"Exception: {exc}",
                source="executor-exception-handler",
            )
            task.verdict = Verdict.FAIL

    def _check_degradation_exemption(self, task: Task) -> None:
        """
        If all applicable degradation items in the inventory cover this task,
        emit TASK_DEGRADED_ACCEPTABLE instead of TASK_FAILED.
        """
        applicable = [
            d for d in self.degradation_inventory
            if task.id in d.affects_task_ids
        ]
        exempted_ids = {d.id for d in applicable}
        declared_ids = set(task.known_acceptable_degradation_ids)

        if declared_ids and declared_ids.issubset(exempted_ids):
            self._log(_evt(
                "TASK_DEGRADED_ACCEPTABLE",
                task_id=task.id,
                detail=(
                    f"Task {task.id!r} failed but degradation is known-acceptable: "
                    f"{sorted(declared_ids)}"
                ),
                degradation_ids=sorted(declared_ids),
                degradation_descriptions={d.id: d.description for d in applicable},
            ))
        else:
            self._log(_evt(
                "TASK_FAILED",
                task_id=task.id,
                detail=f"Task {task.id!r} FAILED (no applicable degradation exemption)",
                verdict=task.verdict,
                declared_degradation_ids=sorted(declared_ids),
                evidence_sources=[e.source for e in task.evidence],
            ))

    def _emit_summary(self, order: list[str]) -> None:
        passed = [t for t in order if self.tasks[t].verdict == Verdict.PASS]
        failed = [t for t in order if self.tasks[t].verdict == Verdict.FAIL]
        blocked = [t for t in order if self.tasks[t].verdict == Verdict.BLOCKED]
        stale = [t for t in order if self.tasks[t].verdict == Verdict.STALE_EVIDENCE]

        self._log(_evt(
            "DRY_RUN_SUMMARY" if self.dry_run else "EXECUTION_SUMMARY",
            task_id="__executor__",
            detail=(
                f"Execution complete. passed={len(passed)}, failed={len(failed)}, "
                f"blocked={len(blocked)}, stale_evidence={len(stale)}"
            ),
            passed=passed,
            failed=failed,
            blocked=blocked,
            stale_evidence=stale,
            total=len(order),
        ))

    def _log(self, event: dict) -> None:
        self.event_log.append(event)

    # ------------------------------------------------------------------
    # Convenience: dump log as JSON
    # ------------------------------------------------------------------

    def dump_log_json(self, indent: int = 2) -> str:
        return json.dumps(self.event_log, indent=indent, default=str)
