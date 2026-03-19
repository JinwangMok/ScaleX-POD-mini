"""
tests/task_model/test_timestamp_enforcement.py  [Sub-AC 2a]

Timestamp enforcement tests for the ScaleX-POD-mini verification harness.

Scope boundary (declared before evaluation):
  - Unit tests only — no remote calls, no VMs, no SSH.
  - Tests verify that every evidence artifact produced by TaskExecutor is tagged
    with a run-scoped run_id (UUID) and a wall-clock captured_at timestamp.
  - All test data is synthetic (in-process stubs); no external I/O.

Evidence freshness constraint: MAX_EVIDENCE_AGE_S = 600 (10 minutes).
Known-acceptable-degradation: None for this sub-AC (pure in-process tests).
"""

from __future__ import annotations

import time
import uuid
from typing import List

import pytest

from tests.task_model.model import (
    Evidence,
    EvidentialDep,
    MAX_EVIDENCE_AGE_S,
    RunContext,
    Task,
    TaskExecutor,
    TaskStatus,
    _new_run_context,
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _stub_evidence(name: str = "stub") -> Evidence:
    """Create a minimal Evidence without a run_id (simulating pre-executor stubs)."""
    return Evidence(
        captured_at=time.time(),
        raw_output=f"stub output for {name}",
        summary=f"{name} ok",
    )


def _make_task(
    name: str,
    prereqs: list[str] | None = None,
    evidence_deps: list[EvidentialDep] | None = None,
    produces_key: str | None = None,
) -> Task:
    prereqs = prereqs or []
    evidence_deps = evidence_deps or []

    def run_fn() -> Evidence:
        return _stub_evidence(name)

    return Task(
        name=name,
        scope=f"test-scope:{name}",
        prerequisites=prereqs,
        evidence_deps=evidence_deps,
        run_fn=run_fn,
        produces_evidence_key=produces_key or f"{name}:result",
    )


# ---------------------------------------------------------------------------
# TC-TS-1: RunContext dataclass  [Sub-AC 2a — structure]
# ---------------------------------------------------------------------------

class TestRunContextDataclass:
    def test_run_context_has_run_id_and_started_at(self):
        """
        SCOPE: unit test — pure in-process.
        RunContext dataclass must expose run_id (str) and started_at (float).
        """
        ctx = RunContext(run_id="test-id-123", started_at=1234567890.0)
        assert ctx.run_id == "test-id-123"
        assert ctx.started_at == 1234567890.0

    def test_new_run_context_generates_valid_uuid(self):
        """
        SCOPE: unit test.
        _new_run_context() must produce a RunContext whose run_id is a valid UUID4.
        """
        ctx = _new_run_context()
        # Must be parseable as a UUID
        parsed = uuid.UUID(ctx.run_id, version=4)
        assert str(parsed) == ctx.run_id, (
            f"run_id {ctx.run_id!r} is not a canonical UUID4 string"
        )

    def test_new_run_context_started_at_is_recent(self):
        """
        SCOPE: unit test.
        _new_run_context().started_at must be within 5 seconds of time.time().
        """
        before = time.time()
        ctx = _new_run_context()
        after = time.time()
        assert before <= ctx.started_at <= after, (
            f"started_at={ctx.started_at} is outside [{before}, {after}] window"
        )

    def test_two_run_contexts_have_different_run_ids(self):
        """
        SCOPE: unit test.
        Each call to _new_run_context() must produce a distinct run_id.
        """
        ctx1 = _new_run_context()
        ctx2 = _new_run_context()
        assert ctx1.run_id != ctx2.run_id, (
            "Two RunContext instances must have distinct run_ids; "
            f"both got {ctx1.run_id!r}"
        )


# ---------------------------------------------------------------------------
# TC-TS-2: TaskExecutor creates RunContext at init  [Sub-AC 2a — executor]
# ---------------------------------------------------------------------------

class TestExecutorRunContext:
    def test_executor_exposes_run_context(self):
        """
        SCOPE: unit test.
        TaskExecutor must have a run_context attribute after construction.
        """
        task = _make_task("A")
        executor = TaskExecutor([task], dry_run=True)
        assert hasattr(executor, "run_context"), (
            "TaskExecutor must expose run_context (RunContext instance)"
        )
        assert isinstance(executor.run_context, RunContext), (
            f"run_context must be RunContext, got {type(executor.run_context).__name__}"
        )

    def test_executor_run_context_has_valid_uuid(self):
        """
        SCOPE: unit test.
        executor.run_context.run_id must be a valid UUID4.
        """
        task = _make_task("A")
        executor = TaskExecutor([task], dry_run=True)
        parsed = uuid.UUID(executor.run_context.run_id, version=4)
        assert str(parsed) == executor.run_context.run_id

    def test_executor_run_context_started_at_is_recent(self):
        """
        SCOPE: unit test.
        executor.run_context.started_at must be within 5 seconds of construction.
        """
        before = time.time()
        task = _make_task("A")
        executor = TaskExecutor([task], dry_run=True)
        after = time.time()
        assert before <= executor.run_context.started_at <= after, (
            f"started_at={executor.run_context.started_at} outside window [{before}, {after}]"
        )

    def test_two_executors_have_different_run_ids(self):
        """
        SCOPE: unit test.
        Two independently-constructed TaskExecutor instances must have distinct
        run_ids so evidence from separate runs is never confused.
        """
        task_a = _make_task("A")
        task_b = _make_task("B")
        exec1 = TaskExecutor([task_a], dry_run=True)
        exec2 = TaskExecutor([task_b], dry_run=True)
        assert exec1.run_context.run_id != exec2.run_context.run_id, (
            "Different executor instances must have distinct run_ids"
        )


# ---------------------------------------------------------------------------
# TC-TS-3: Evidence artifacts tagged with run_id  [Sub-AC 2a — tagging]
# ---------------------------------------------------------------------------

class TestEvidenceTagging:
    def test_produced_evidence_has_run_id_set(self):
        """
        SCOPE: unit test — in-process stub only.
        After a task succeeds, its evidence.run_id must equal executor.run_context.run_id.
        """
        task = _make_task("A")
        executor = TaskExecutor([task], dry_run=False)
        results = executor.run()

        result = results["A"]
        assert result.status == TaskStatus.SUCCEEDED
        assert result.evidence is not None
        assert result.evidence.run_id is not None, (
            "Produced evidence must have run_id set by the executor"
        )
        assert result.evidence.run_id == executor.run_context.run_id, (
            f"evidence.run_id={result.evidence.run_id!r} must match "
            f"executor.run_context.run_id={executor.run_context.run_id!r}"
        )

    def test_all_task_results_share_same_run_id(self):
        """
        SCOPE: unit test.
        In a single executor run with multiple tasks, all produced evidence must
        carry the same run_id (the executor's run_context.run_id).
        """
        tasks = [
            _make_task("A", produces_key="A:out"),
            _make_task("B", prereqs=["A"], produces_key="B:out"),
            _make_task("C", prereqs=["B"], produces_key="C:out"),
        ]
        executor = TaskExecutor(tasks, dry_run=False)
        run_id = executor.run_context.run_id
        results = executor.run()

        for name in ("A", "B", "C"):
            ev = results[name].evidence
            assert ev is not None, f"Task {name} must produce evidence"
            assert ev.run_id == run_id, (
                f"Task {name} evidence.run_id={ev.run_id!r} != "
                f"run_context.run_id={run_id!r} — all evidence in one run must share run_id"
            )

    def test_recheck_evidence_has_executor_run_id(self):
        """
        SCOPE: unit test — in-process stub.
        Evidence captured via re-check (_run_recheck) must also be tagged with
        the executor's run_id, not a stale/None value.
        """
        captured_run_ids: list[str | None] = []

        def source_run_fn() -> Evidence:
            ev = Evidence(
                captured_at=time.time(),
                raw_output="re-checked SSH output",
                summary="ssh ok (recheck)",
            )
            captured_run_ids.append(ev.run_id)  # capture BEFORE executor tags it
            return ev

        source = Task(
            name="source",
            scope="test:source",
            run_fn=source_run_fn,
            produces_evidence_key="source:result",
        )
        consumer = Task(
            name="consumer",
            scope="test:consumer",
            prerequisites=["source"],
            evidence_deps=[EvidentialDep("source:result", "source", max_age_s=600)],
            run_fn=lambda: Evidence(
                captured_at=time.time(),
                raw_output="consumer ran",
                summary="consumer ok",
            ),
        )

        executor = TaskExecutor([source, consumer], dry_run=False)
        run_id = executor.run_context.run_id

        # Seed stale evidence to force a re-check
        executor.seed_evidence(
            "source:result",
            raw_output="stale output",
            summary="old",
            age_seconds=700,
        )

        executor.run()

        # After re-check, evidence in the store must carry the executor's run_id
        refreshed_ev = executor._evidence_store.get("source:result")
        assert refreshed_ev is not None, "Evidence must be in store after re-check"
        assert refreshed_ev.run_id == run_id, (
            f"Re-checked evidence.run_id={refreshed_ev.run_id!r} must match "
            f"executor run_id={run_id!r}"
        )

    def test_evidence_captured_at_is_recent(self):
        """
        SCOPE: unit test.
        Produced evidence.captured_at must be within 5 seconds of the
        time the task ran (wall-clock accuracy check).
        """
        task = _make_task("A")
        before = time.time()
        executor = TaskExecutor([task], dry_run=False)
        results = executor.run()
        after = time.time()

        ev = results["A"].evidence
        assert ev is not None
        assert before <= ev.captured_at <= after, (
            f"evidence.captured_at={ev.captured_at} is outside run window "
            f"[{before:.3f}, {after:.3f}]"
        )

    def test_stub_evidence_run_id_is_none_before_executor(self):
        """
        SCOPE: unit test.
        Evidence objects created outside an executor (e.g. in test stubs) must
        default run_id=None until the executor tags them.
        This validates that run_id is explicitly set by the executor, not by the
        run_fn itself.
        """
        ev = Evidence(
            captured_at=time.time(),
            raw_output="manual stub",
            summary="ok",
        )
        assert ev.run_id is None, (
            "Evidence created outside executor must default run_id=None; "
            "only the executor sets run_id."
        )


# ---------------------------------------------------------------------------
# TC-TS-4: Seeded evidence NOT tagged with executor run_id  [Sub-AC 2a — boundary]
# ---------------------------------------------------------------------------

class TestSeededEvidenceNotTagged:
    def test_seeded_evidence_run_id_is_none_by_default(self):
        """
        SCOPE: unit test.
        Pre-seeded test evidence (via seed_evidence()) represents state from a
        prior run or external source.  It must NOT be retroactively tagged with
        the current executor's run_id — it represents foreign-run state.
        """
        task = _make_task("A", produces_key="A:out")
        executor = TaskExecutor([task], dry_run=True)

        executor.seed_evidence(
            "A:out",
            raw_output="seeded output",
            summary="pre-seeded",
            age_seconds=0.0,
        )

        seeded = executor._evidence_store["A:out"]
        assert seeded.run_id is None, (
            "Seeded evidence must default to run_id=None (it represents prior-run state, "
            f"not the current run {executor.run_context.run_id!r})"
        )


# ---------------------------------------------------------------------------
# TC-TS-5: run_id logged at executor construction  [Sub-AC 2a — observability]
# ---------------------------------------------------------------------------

class TestRunContextLogged:
    def test_run_context_run_id_is_logged_at_construction(self, caplog):
        """
        SCOPE: unit test.
        TaskExecutor must log the run_id at INFO level on construction so that
        every evidence artifact can be correlated to a run in the log stream.
        """
        import logging
        task = _make_task("A")

        with caplog.at_level(logging.INFO, logger="task_model"):
            executor = TaskExecutor([task], dry_run=True)

        run_id = executor.run_context.run_id
        run_id_logs = [
            r.message for r in caplog.records
            if "RUN_CONTEXT" in r.message and run_id in r.message
        ]
        assert run_id_logs, (
            f"Expected RUN_CONTEXT log with run_id={run_id!r} at construction.\n"
            f"All INFO+ logs: {[r.message for r in caplog.records]}"
        )
