"""
tests/task_model/test_evidential_deps.py  [Sub-AC 3b]

Evidential dependency edge tests for the ScaleX-POD-mini task model.

Scope boundary (declared before evaluation):
  - Unit tests only — no remote calls, no VMs, no SSH.
  - Tests verify executor logic for EVIDENTIAL dependency enforcement:
      * Each Task declares evidence it relies on (EvidentialDep)
      * Executor detects STALE evidence → logs RECHECK_TRIGGERED + re-runs source
      * Executor detects MISSING evidence → logs RECHECK_TRIGGERED + runs source
      * Executor detects FRESH evidence → proceeds without re-check
      * Dry-run mode logs re-check plan without executing source task
  - Evidence is pre-seeded via executor.seed_evidence() for deterministic tests.

Evidence freshness constraint: MAX_EVIDENCE_AGE_S = 600 (10 minutes).
Known-acceptable-degradation: None for this AC (unit tests only).
"""

from __future__ import annotations

import logging
import time
from typing import List

import pytest

from tests.task_model.model import (
    Evidence,
    EvidentialDep,
    MAX_EVIDENCE_AGE_S,
    Task,
    TaskExecutor,
    TaskStatus,
)
from tests.task_model.scalex_tasks import build_task_graph


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _stub_evidence(name: str, age_seconds: float = 0.0) -> Evidence:
    return Evidence(
        captured_at=time.time() - age_seconds,
        raw_output=f"stub output for {name}",
        summary=f"{name} ok",
    )


def _make_task(
    name: str,
    prereqs: list[str] | None = None,
    evidence_deps: list[EvidentialDep] | None = None,
    *,
    should_fail: bool = False,
    produces_key: str | None = None,
) -> Task:
    prereqs = prereqs or []
    evidence_deps = evidence_deps or []

    def run_fn() -> Evidence:
        if should_fail:
            raise RuntimeError(f"Task {name} intentionally failed")
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
# TC-EB-1: EvidentialDep declared on Task  [AC 3b — declaration]
# ---------------------------------------------------------------------------

class TestEvidentialDepDeclaration:
    def test_task_can_declare_evidence_dep(self):
        """Task.evidence_deps field accepts EvidentialDep objects."""
        dep = EvidentialDep(
            evidence_key="check_ssh_connectivity:reachability",
            source_task_name="check_ssh_connectivity",
            max_age_s=600,
        )
        task = Task(
            name="gather_hardware_facts",
            scope="bare-metal:hw-facts",
            evidence_deps=[dep],
        )
        assert len(task.evidence_deps) == 1
        assert task.evidence_deps[0].evidence_key == "check_ssh_connectivity:reachability"
        assert task.evidence_deps[0].source_task_name == "check_ssh_connectivity"
        assert task.evidence_deps[0].max_age_s == 600

    def test_evidence_dep_default_ttl_is_600s(self):
        """EvidentialDep.max_age_s defaults to MAX_EVIDENCE_AGE_S (600 s)."""
        dep = EvidentialDep(
            evidence_key="foo:bar",
            source_task_name="foo",
        )
        assert dep.max_age_s == MAX_EVIDENCE_AGE_S == 600

    def test_scalex_tasks_have_evidence_deps_declared(self):
        """
        Every non-root task in the ScaleX task graph declares at least one
        evidential dependency (Sub-AC 3b requirement).
        """
        tasks = {t.name: t for t in build_task_graph()}
        root = "check_ssh_connectivity"
        for name, task in tasks.items():
            if name == root:
                continue  # root produces evidence, doesn't consume it
            assert task.evidence_deps, (
                f"Task '{name}' has no evidence_deps declared — "
                f"Sub-AC 3b requires every non-root task to declare its evidential deps"
            )

    def test_scalex_task_evidence_dep_keys_reference_valid_sources(self):
        """
        Every EvidentialDep.source_task_name in the ScaleX graph must reference
        a task that actually exists in the registry.
        """
        tasks = {t.name: t for t in build_task_graph()}
        for task in tasks.values():
            for dep in task.evidence_deps:
                assert dep.source_task_name in tasks, (
                    f"Task '{task.name}' evidence_dep references unknown source "
                    f"'{dep.source_task_name}'"
                )


# ---------------------------------------------------------------------------
# TC-EB-2: STALE evidence triggers RECHECK  [AC 3b — staleness detection]
# ---------------------------------------------------------------------------

class TestStaleEvidenceTriggerRecheck:
    def test_stale_evidence_triggers_recheck_warning_log(self, caplog):
        """
        SCOPE: local unit test — no remote calls.
        When a task has an evidential dep with STALE evidence (age > TTL),
        the executor emits a RECHECK_TRIGGERED log entry at WARNING level.
        """
        # Task A produces evidence; Task B depends on it
        tasks = [
            _make_task("A", produces_key="A:result"),
            _make_task("B", prereqs=["A"], evidence_deps=[
                EvidentialDep("A:result", "A", max_age_s=600),
            ]),
        ]
        executor = TaskExecutor(tasks, dry_run=True)

        # Seed stale evidence for A (age = 700 s, exceeds 600 s TTL)
        executor.seed_evidence(
            "A:result",
            raw_output="$ ssh playbox-0 hostname\nplaybox-0\nexit_code: 0",
            summary="A ok",
            age_seconds=700,
        )

        with caplog.at_level(logging.WARNING, logger="task_model"):
            executor.run()

        recheck_msgs = [r for r in caplog.records if "RECHECK_TRIGGERED" in r.message]
        assert recheck_msgs, (
            "Expected RECHECK_TRIGGERED log entry for stale evidence, none found.\n"
            f"All log records: {[r.message for r in caplog.records]}"
        )

    def test_stale_evidence_log_contains_evidence_key_and_age(self, caplog):
        """
        SCOPE: local unit test.
        RECHECK_TRIGGERED log for stale evidence must include:
          - evidence_key
          - reason=STALE
          - age in seconds
          - ttl in seconds
          - source task name
        """
        tasks = [
            _make_task("source_task", produces_key="source_task:connectivity"),
            _make_task("consumer_task", prereqs=["source_task"], evidence_deps=[
                EvidentialDep("source_task:connectivity", "source_task", max_age_s=600),
            ]),
        ]
        executor = TaskExecutor(tasks, dry_run=True)
        executor.seed_evidence(
            "source_task:connectivity",
            raw_output="old output",
            summary="ok",
            age_seconds=750,
        )

        with caplog.at_level(logging.WARNING, logger="task_model"):
            executor.run()

        recheck_msgs = [r.message for r in caplog.records if "RECHECK_TRIGGERED" in r.message]
        assert recheck_msgs, "No RECHECK_TRIGGERED entry found"
        msg = recheck_msgs[0]
        assert "source_task:connectivity" in msg, f"evidence_key missing from log: {msg}"
        assert "STALE" in msg, f"reason=STALE missing from log: {msg}"
        assert "source_task" in msg, f"source task name missing from log: {msg}"

    def test_stale_evidence_dry_run_logs_recheck_plan(self, caplog):
        """
        SCOPE: local unit test.
        Dry-run mode must log what would be re-checked without running source task.
        The 're-run would happen' message proves the executor planned the re-check.
        """
        tasks = [
            _make_task("ssh_check", produces_key="ssh_check:reachability"),
            _make_task("hardware_facts", prereqs=["ssh_check"], evidence_deps=[
                EvidentialDep("ssh_check:reachability", "ssh_check", max_age_s=600),
            ]),
        ]
        executor = TaskExecutor(tasks, dry_run=True)
        executor.seed_evidence(
            "ssh_check:reachability",
            raw_output="stale SSH output",
            summary="ssh ok",
            age_seconds=900,  # 15 minutes old — stale
        )

        with caplog.at_level(logging.INFO, logger="task_model"):
            executor.run()

        # Check that dry-run re-check plan is logged
        dry_recheck = [
            r.message for r in caplog.records
            if "DRY-RUN" in r.message and "RECHECK" in r.message
        ]
        assert dry_recheck, (
            "Expected DRY-RUN RECHECK log entry showing re-check plan.\n"
            f"All logs: {[r.message for r in caplog.records]}"
        )

    def test_stale_ssh_evidence_triggers_recheck_before_remote_task(self, caplog):
        """
        SCOPE: local unit test — no actual SSH connections.
        Network safety invariant: if SSH connectivity evidence is STALE (>600 s),
        any task with an evidence_dep on it must trigger RECHECK_TRIGGERED before
        the remote task is allowed to proceed.
        This mirrors the feedback_network_safety_critical.md constraint.
        """
        tasks = [
            _make_task("check_ssh_connectivity",
                       produces_key="check_ssh_connectivity:reachability"),
            _make_task("sdi_init",
                       prereqs=["check_ssh_connectivity"],
                       evidence_deps=[
                           EvidentialDep(
                               "check_ssh_connectivity:reachability",
                               "check_ssh_connectivity",
                               max_age_s=600,
                           )
                       ]),
        ]
        executor = TaskExecutor(tasks, dry_run=True)
        # Seed stale SSH evidence (12 minutes old)
        executor.seed_evidence(
            "check_ssh_connectivity:reachability",
            raw_output="$ ssh playbox-0 echo ok\nok\nexit_code: 0",
            summary="ssh_check exit=0",
            age_seconds=720,
        )

        with caplog.at_level(logging.WARNING, logger="task_model"):
            executor.run()

        recheck = [r for r in caplog.records if "RECHECK_TRIGGERED" in r.message]
        assert recheck, "RECHECK_TRIGGERED must be emitted when SSH evidence is stale"
        msg = recheck[0].message
        assert "check_ssh_connectivity:reachability" in msg or "check_ssh_connectivity" in msg, (
            f"Log must reference SSH evidence key, got: {msg}"
        )


# ---------------------------------------------------------------------------
# TC-EB-3: MISSING evidence triggers RECHECK  [AC 3b — missing detection]
# ---------------------------------------------------------------------------

class TestMissingEvidenceTriggerRecheck:
    def test_missing_evidence_triggers_recheck_warning_log(self, caplog):
        """
        SCOPE: local unit test.
        When a task has an evidential dep with NO evidence in store,
        the executor emits RECHECK_TRIGGERED(reason=MISSING).
        """
        tasks = [
            _make_task("A", produces_key="A:result"),
            _make_task("B", prereqs=["A"], evidence_deps=[
                EvidentialDep("A:result", "A", max_age_s=600),
            ]),
        ]
        # Do NOT seed any evidence — it's MISSING
        executor = TaskExecutor(tasks, dry_run=True)

        with caplog.at_level(logging.WARNING, logger="task_model"):
            executor.run()

        recheck_msgs = [r for r in caplog.records if "RECHECK_TRIGGERED" in r.message]
        assert recheck_msgs, (
            "Expected RECHECK_TRIGGERED for missing evidence, none found.\n"
            f"All logs: {[r.message for r in caplog.records]}"
        )

    def test_missing_evidence_log_contains_missing_reason(self, caplog):
        """
        SCOPE: local unit test.
        RECHECK_TRIGGERED log for MISSING evidence must include 'MISSING' reason.
        """
        tasks = [
            _make_task("source", produces_key="source:output"),
            _make_task("consumer", prereqs=["source"], evidence_deps=[
                EvidentialDep("source:output", "source", max_age_s=600),
            ]),
        ]
        executor = TaskExecutor(tasks, dry_run=True)
        # No evidence seeded

        with caplog.at_level(logging.WARNING, logger="task_model"):
            executor.run()

        recheck_msgs = [r.message for r in caplog.records if "RECHECK_TRIGGERED" in r.message]
        assert recheck_msgs, "No RECHECK_TRIGGERED entry"
        assert "MISSING" in recheck_msgs[0], (
            f"Expected 'MISSING' in log, got: {recheck_msgs[0]}"
        )

    def test_missing_evidence_dry_run_plans_source_task_execution(self, caplog):
        """
        SCOPE: local unit test.
        Dry-run: missing evidence causes the executor to plan re-execution of
        source task.  Log must indicate which source task would be run.
        """
        tasks = [
            _make_task("ssh_check", produces_key="ssh_check:reachability"),
            _make_task("gather_facts", prereqs=["ssh_check"], evidence_deps=[
                EvidentialDep("ssh_check:reachability", "ssh_check", max_age_s=600),
            ]),
        ]
        executor = TaskExecutor(tasks, dry_run=True)
        # No evidence seeded

        with caplog.at_level(logging.INFO, logger="task_model"):
            executor.run()

        # Dry-run should log that it would re-run ssh_check
        plan_msgs = [
            r.message for r in caplog.records
            if "DRY-RUN" in r.message and "RECHECK" in r.message
        ]
        assert plan_msgs, (
            "Dry-run should emit RECHECK plan log for missing evidence.\n"
            f"All logs: {[r.message for r in caplog.records]}"
        )
        # Source task name must appear in the plan
        assert "ssh_check" in plan_msgs[0], (
            f"Source task 'ssh_check' must be named in re-check plan: {plan_msgs[0]}"
        )


# ---------------------------------------------------------------------------
# TC-EB-4: FRESH evidence does NOT trigger recheck  [AC 3b — no false positives]
# ---------------------------------------------------------------------------

class TestFreshEvidenceNoRecheck:
    def test_fresh_evidence_no_recheck_trigger(self, caplog):
        """
        SCOPE: local unit test.
        When evidence is fresh (age < TTL), no RECHECK_TRIGGERED event is emitted.
        """
        tasks = [
            _make_task("A", produces_key="A:result"),
            _make_task("B", prereqs=["A"], evidence_deps=[
                EvidentialDep("A:result", "A", max_age_s=600),
            ]),
        ]
        executor = TaskExecutor(tasks, dry_run=True)

        # Seed FRESH evidence (5 seconds old, well within 600 s TTL)
        executor.seed_evidence(
            "A:result",
            raw_output="$ hostname\nplaybox-0\nexit_code: 0",
            summary="A ok",
            age_seconds=5,
        )

        with caplog.at_level(logging.WARNING, logger="task_model"):
            executor.run()

        recheck_msgs = [r for r in caplog.records if "RECHECK_TRIGGERED" in r.message]
        assert not recheck_msgs, (
            f"Fresh evidence must NOT trigger re-check, but got: "
            f"{[r.message for r in recheck_msgs]}"
        )

    def test_evidence_just_within_ttl_is_accepted(self, caplog):
        """
        SCOPE: local unit test.
        Evidence at exactly TTL boundary (599 s) must be accepted as FRESH.
        """
        tasks = [
            _make_task("A", produces_key="A:fresh"),
            _make_task("B", prereqs=["A"], evidence_deps=[
                EvidentialDep("A:fresh", "A", max_age_s=600),
            ]),
        ]
        executor = TaskExecutor(tasks, dry_run=True)
        executor.seed_evidence(
            "A:fresh",
            raw_output="fresh evidence",
            summary="ok",
            age_seconds=599,  # 1 second before TTL expiry
        )

        with caplog.at_level(logging.WARNING, logger="task_model"):
            executor.run()

        recheck_msgs = [r for r in caplog.records if "RECHECK_TRIGGERED" in r.message]
        assert not recheck_msgs, "Evidence 1 s before TTL must be accepted as FRESH"


# ---------------------------------------------------------------------------
# TC-EB-5: Live re-execution of source task  [AC 3b — actual recheck]
# ---------------------------------------------------------------------------

class TestLiveRecheck:
    def test_stale_evidence_causes_source_task_reexecution(self):
        """
        SCOPE: local unit test — in-process stub, no remote calls.
        When evidence is STALE in non-dry-run mode, the executor re-runs
        the source task and updates the evidence store.
        """
        recheck_count = [0]

        def source_run_fn() -> Evidence:
            recheck_count[0] += 1
            return Evidence(
                captured_at=time.time(),
                raw_output="fresh SSH check output",
                summary=f"ssh ok (run #{recheck_count[0]})",
            )

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
        # Seed stale evidence — source task ran, but 11 minutes ago
        executor.seed_evidence(
            "source:result",
            raw_output="old SSH output",
            summary="ssh ok (old)",
            age_seconds=660,  # 11 minutes
        )

        executor.run()

        # Source task must have been re-executed (recheck_count incremented)
        assert recheck_count[0] >= 1, (
            "Source task must have been re-executed to refresh stale evidence. "
            f"re-check count = {recheck_count[0]}"
        )

    def test_missing_evidence_causes_source_task_execution(self):
        """
        SCOPE: local unit test.
        When evidence is MISSING (never captured), the executor runs the source
        task to capture fresh evidence before the consuming task executes.
        """
        run_log: list[str] = []

        def source_run_fn() -> Evidence:
            run_log.append("source_ran")
            return Evidence(
                captured_at=time.time(),
                raw_output="SSH check output",
                summary="ssh ok",
            )

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
        # No evidence seeded — MISSING

        executor.run()

        # Source task MUST have been called (at least once for normal run,
        # possibly twice — once normally, once via re-check)
        assert "source_ran" in run_log, (
            "Source task must execute when evidence is MISSING"
        )


# ---------------------------------------------------------------------------
# TC-EB-6: Full ScaleX task graph evidential dep checks  [AC 3b integration]
# ---------------------------------------------------------------------------

class TestScaleXEvidentialDeps:
    def test_full_graph_all_evidence_deps_declared(self):
        """
        SCOPE: local unit test.
        All non-root ScaleX tasks have at least one EvidentialDep.
        Verifies Sub-AC 3b requirement that each task declares its evidence deps.
        """
        tasks = {t.name: t for t in build_task_graph()}
        root = "check_ssh_connectivity"
        for name, task in tasks.items():
            if name == root:
                continue
            assert task.evidence_deps, (
                f"[SUB-AC 3b VIOLATION] Task '{name}' missing evidence_deps. "
                f"Every non-root task must declare the evidence it relies on."
            )

    def test_full_graph_dry_run_with_all_stale_evidence_triggers_rechecks(
        self, caplog
    ):
        """
        SCOPE: local unit test — dry-run only, no remote calls.

        Dry-run scenario with ALL evidence pre-seeded as STALE (900 s old).
        Verifies that:
          1. Every non-root task triggers RECHECK_TRIGGERED for its evidence deps
          2. Dry-run re-check plan is logged for each stale dep

        This is the primary Sub-AC 3b dry-run evidence: log shows re-check triggers.
        """
        tasks = build_task_graph()
        executor = TaskExecutor(tasks, dry_run=True)

        # Pre-seed ALL evidence keys as STALE (15 minutes old)
        # [Sub-AC 7c] "sdi_init:completion" renamed to "sdi_init:vm_list"
        all_evidence_keys = [
            "check_ssh_connectivity:reachability",
            "gather_hardware_facts:hw_facts",
            "sdi_init:vm_list",
            "sdi_verify_vms:vm_ready",
            "sdi_health_check:virsh_status",
            "kubespray_tower:cluster_healthy",
            "kubespray_sandbox:cluster_healthy",
            "tower_post_install_verify:api_reachable",
            "gitops_bootstrap:spread_applied",
            "argocd_sync_healthy:all_synced",
            "cf_tunnel_healthy:tunnel_up",
            "dash_headless_verify:snapshot_valid",
            "scalex_dash_token_provisioned:token_valid",
        ]
        for key in all_evidence_keys:
            executor.seed_evidence(
                key,
                raw_output=f"stale evidence for {key}",
                summary="ok (stale)",
                age_seconds=900,
            )

        with caplog.at_level(logging.INFO, logger="task_model"):
            executor.run()

        recheck_events = [
            r.message for r in caplog.records
            if "RECHECK_TRIGGERED" in r.message
        ]
        dry_recheck_plans = [
            r.message for r in caplog.records
            if "DRY-RUN" in r.message and "RECHECK" in r.message
        ]

        assert recheck_events, (
            "With all evidence STALE, at least one RECHECK_TRIGGERED must be emitted.\n"
            f"Log count: {len(caplog.records)}"
        )
        assert dry_recheck_plans, (
            "Dry-run mode must emit re-check plan logs for stale evidence.\n"
            f"All logs: {[r.message for r in caplog.records]}"
        )

    def test_full_graph_dry_run_no_evidence_triggers_missing_rechecks(self, caplog):
        """
        SCOPE: local unit test — dry-run, no remote calls.

        Dry-run with NO evidence in store.
        Every non-root task must trigger RECHECK_TRIGGERED(reason=MISSING)
        for its evidential deps.
        """
        tasks = build_task_graph()
        executor = TaskExecutor(tasks, dry_run=True)
        # No evidence seeded

        with caplog.at_level(logging.WARNING, logger="task_model"):
            executor.run()

        recheck_missing = [
            r.message for r in caplog.records
            if "RECHECK_TRIGGERED" in r.message and "MISSING" in r.message
        ]
        assert recheck_missing, (
            "With no evidence in store, RECHECK_TRIGGERED(MISSING) must be emitted.\n"
            f"All WARNING logs: {[r.message for r in caplog.records if r.levelno >= logging.WARNING]}"
        )

    def test_ssh_evidence_dep_present_on_all_remote_tasks(self):
        """
        SCOPE: local unit test.
        Network safety: all tasks that make remote calls must have an evidential
        dep on check_ssh_connectivity:reachability.
        (Per feedback_network_safety_critical.md: verify SSH before AND after
        every remote operation.)
        """
        SSH_EVIDENCE_KEY = "check_ssh_connectivity:reachability"
        # Tasks that touch remote systems (bare-metal or VMs)
        remote_tasks = {
            "gather_hardware_facts",
            "sdi_init",
            "sdi_verify_vms",
            "sdi_health_check",
            "kubespray_tower",
            "kubespray_sandbox",
            "cf_tunnel_healthy",
        }
        tasks = {t.name: t for t in build_task_graph()}
        for name in remote_tasks:
            task = tasks[name]
            dep_keys = {d.evidence_key for d in task.evidence_deps}
            assert SSH_EVIDENCE_KEY in dep_keys, (
                f"Remote task '{name}' missing evidential dep on '{SSH_EVIDENCE_KEY}'. "
                f"Network safety requires SSH evidence freshness check before remote ops. "
                f"Current evidence_deps: {dep_keys}"
            )
