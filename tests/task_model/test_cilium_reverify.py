"""
tests/task_model/test_cilium_reverify.py  [Sub-AC 2b]

Scope boundary (declared before evaluation):
  - Scope: service:cilium — kube-system namespace on tower cluster.
  - Tests verify that the cilium_health_verify task:
      1. Executes Cilium CNI health checks within the current run window.
      2. Captures stdout/stderr with an embedded ISO-8601 timestamp token
         (CILIUM_HEALTH_PROBE_TIMESTAMP=<ISO>).
      3. Evidence.captured_at falls within [run_start_epoch, run_start_epoch +
         EVIDENCE_TTL_SECONDS] — the "current run window" constraint.
      4. Periodic re-verification: stale evidence triggers RECHECK_TRIGGERED
         before any downstream task consuming cilium_health_verify:cni_status
         is executed.
  - No live Cilium cluster required: the task falls back to probe mode if
    kubectl is unavailable or the cluster is unreachable.
  - All assertions use local in-process Python — no SSH, no VMs.

Evidence freshness constraint: EVIDENCE_TTL_SECONDS = 600 (10 minutes).
Known-acceptable-degradation: None for this AC (unit tests only).

References:
  - ops/artifact_vocabulary.py: cilium_health_verify:cni_status (COARSE)
  - ops/artifact_registry.py:   service:cilium
  - tests/task_model/scalex_tasks.py: cilium_health_verify task with
    run_fn=_cilium_health_run_fn
"""

from __future__ import annotations

import logging
import time

import pytest

from tests.task_model.model import (
    Evidence,
    EvidentialDep,
    MAX_EVIDENCE_AGE_S,
    Task,
    TaskExecutor,
    TaskStatus,
)
from tests.task_model.scalex_tasks import _cilium_health_run_fn, build_task_graph


# ---------------------------------------------------------------------------
# TC-CR-1: Cilium task exists and is properly declared
# ---------------------------------------------------------------------------

class TestCiliumTaskDeclaration:
    """Structural checks — task graph contains a well-formed cilium_health_verify task."""

    def test_cilium_task_exists_in_graph(self):
        """
        SCOPE: local unit test.
        build_task_graph() must include a 'cilium_health_verify' task.
        """
        tasks = {t.name: t for t in build_task_graph()}
        assert "cilium_health_verify" in tasks, (
            "[Sub-AC 2b] 'cilium_health_verify' task not found in build_task_graph(). "
            f"Available tasks: {sorted(tasks.keys())}"
        )

    def test_cilium_task_scope_boundary_declared(self):
        """
        SCOPE: local unit test.
        cilium_health_verify.scope must be non-empty (declared before evaluation).
        """
        tasks = {t.name: t for t in build_task_graph()}
        task = tasks["cilium_health_verify"]
        assert task.scope.strip(), (
            "cilium_health_verify.scope must be a non-empty declared boundary"
        )
        assert "cilium" in task.scope.lower(), (
            "cilium_health_verify.scope must name 'cilium' as the artifact in scope"
        )

    def test_cilium_task_produces_registered_evidence_key(self):
        """
        SCOPE: local unit test.
        cilium_health_verify must produce 'cilium_health_verify:cni_status',
        which is registered in the controlled vocabulary [Sub-AC 7c].
        """
        from ops.artifact_vocabulary import ARTIFACT_REGISTRY
        tasks = {t.name: t for t in build_task_graph()}
        task = tasks["cilium_health_verify"]
        assert task.produces_evidence_key == "cilium_health_verify:cni_status", (
            f"Expected produces_evidence_key='cilium_health_verify:cni_status', "
            f"got {task.produces_evidence_key!r}"
        )
        assert task.produces_evidence_key in ARTIFACT_REGISTRY, (
            f"cilium_health_verify:cni_status must be in the controlled vocabulary. "
            f"Registered keys: {sorted(ARTIFACT_REGISTRY.keys())}"
        )

    def test_cilium_task_has_tower_api_evidential_dep(self):
        """
        SCOPE: local unit test.
        cilium_health_verify must declare an evidential dep on
        tower_post_install_verify:api_reachable (cluster must be up before CNI check).
        """
        tasks = {t.name: t for t in build_task_graph()}
        task = tasks["cilium_health_verify"]
        dep_keys = {d.evidence_key for d in task.evidence_deps}
        assert "tower_post_install_verify:api_reachable" in dep_keys, (
            "[Sub-AC 2b] cilium_health_verify must depend on "
            "tower_post_install_verify:api_reachable (cluster API must be reachable "
            "before checking CNI health). "
            f"Current evidence_deps: {dep_keys}"
        )

    def test_cilium_task_has_ssh_evidential_dep(self):
        """
        SCOPE: local unit test.
        Network safety invariant: cilium_health_verify must depend on
        check_ssh_connectivity:reachability because it makes a remote kubectl call.
        Per feedback_network_safety_critical.md: verify SSH before AND after
        every remote operation.
        """
        tasks = {t.name: t for t in build_task_graph()}
        task = tasks["cilium_health_verify"]
        dep_keys = {d.evidence_key for d in task.evidence_deps}
        assert "check_ssh_connectivity:reachability" in dep_keys, (
            "[Sub-AC 2b][network-safety] cilium_health_verify must depend on "
            "check_ssh_connectivity:reachability. "
            f"Current evidence_deps: {dep_keys}"
        )

    def test_cilium_task_has_run_fn(self):
        """
        SCOPE: local unit test.
        cilium_health_verify must have a run_fn (not None) so it can be
        executed live for re-verification.  [Sub-AC 2b requires execution]
        """
        tasks = {t.name: t for t in build_task_graph()}
        task = tasks["cilium_health_verify"]
        assert task.run_fn is not None, (
            "[Sub-AC 2b] cilium_health_verify.run_fn must not be None — "
            "Sub-AC 2b requires live execution with captured evidence."
        )


# ---------------------------------------------------------------------------
# TC-CR-2: Evidence freshness — timestamp within current run window
# ---------------------------------------------------------------------------

class TestCiliumEvidenceTimestamp:
    """
    Core Sub-AC 2b requirement: evidence must be captured within the current
    run window, not reused from a previous execution.
    """

    def test_run_fn_captures_evidence_within_current_run_window(self):
        """
        SCOPE: local unit test — executes _cilium_health_run_fn() live.

        [Sub-AC 2b] Primary assertion:
          Before calling run_fn, record run_start = time.time().
          After calling run_fn, assert:
            run_start <= evidence.captured_at <= run_start + MAX_EVIDENCE_AGE_S

        This proves the evidence is freshly captured in the current run
        window, not reused stale evidence from a previous run.
        """
        run_start = time.time()

        evidence = _cilium_health_run_fn()

        run_end = time.time()

        assert evidence.captured_at >= run_start, (
            f"[Sub-AC 2b] evidence.captured_at ({evidence.captured_at:.3f}) "
            f"must be >= run_start ({run_start:.3f}). "
            "Evidence predates the current run — stale reuse detected."
        )
        assert evidence.captured_at <= run_end + 1.0, (
            f"[Sub-AC 2b] evidence.captured_at ({evidence.captured_at:.3f}) "
            f"must be <= run_end ({run_end:.3f}) + 1s tolerance. "
            "Captured_at is suspiciously in the future."
        )
        assert evidence.captured_at <= run_start + MAX_EVIDENCE_AGE_S, (
            f"[Sub-AC 2b] evidence.captured_at ({evidence.captured_at:.3f}) "
            f"exceeds run_start + TTL ({run_start + MAX_EVIDENCE_AGE_S:.3f}). "
            "Evidence would already be stale at capture time — impossible."
        )

    def test_run_fn_embeds_iso_timestamp_in_raw_output(self):
        """
        SCOPE: local unit test — executes _cilium_health_run_fn() live.

        [Sub-AC 2b] The raw_output must contain the token
          CILIUM_HEALTH_PROBE_TIMESTAMP=<ISO-8601>
        so that evidence age can be verified from the output text alone,
        independently of the captured_at epoch field.
        """
        evidence = _cilium_health_run_fn()

        assert "CILIUM_HEALTH_PROBE_TIMESTAMP=" in evidence.raw_output, (
            "[Sub-AC 2b] raw_output must embed 'CILIUM_HEALTH_PROBE_TIMESTAMP=' "
            "token for independent timestamp verification from output text. "
            f"raw_output excerpt: {evidence.raw_output[:300]!r}"
        )

    def test_run_fn_timestamp_in_raw_output_matches_iso_format(self):
        """
        SCOPE: local unit test — executes _cilium_health_run_fn() live.

        The embedded CILIUM_HEALTH_PROBE_TIMESTAMP value must follow
        ISO-8601 format (YYYY-MM-DDTHH:MM:SSZ) so it can be parsed.
        """
        import re
        evidence = _cilium_health_run_fn()

        # Extract the timestamp value from the token
        match = re.search(
            r"CILIUM_HEALTH_PROBE_TIMESTAMP=(\S+)",
            evidence.raw_output,
        )
        assert match, (
            "[Sub-AC 2b] CILIUM_HEALTH_PROBE_TIMESTAMP token not found in "
            f"raw_output. raw_output excerpt: {evidence.raw_output[:300]!r}"
        )

        ts_value = match.group(1)
        # Must match YYYY-MM-DDTHH:MM:SSZ
        iso_pattern = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
        assert iso_pattern.match(ts_value), (
            f"[Sub-AC 2b] Embedded timestamp {ts_value!r} does not match "
            "ISO-8601 format YYYY-MM-DDTHH:MM:SSZ. "
            "raw_output must embed a parseable timestamp."
        )

    def test_run_fn_evidence_is_not_stale_immediately_after_capture(self):
        """
        SCOPE: local unit test.
        Evidence captured by _cilium_health_run_fn() must be fresh (not stale)
        immediately after capture.  is_fresh() must return True.
        """
        evidence = _cilium_health_run_fn()

        age_s = time.time() - evidence.captured_at
        assert evidence.is_fresh(max_age_s=MAX_EVIDENCE_AGE_S), (
            f"[Sub-AC 2b] Evidence captured just now must be fresh (age < {MAX_EVIDENCE_AGE_S}s). "
            f"captured_at={evidence.captured_at:.3f}, "
            f"now={time.time():.3f}, "
            f"age={age_s:.3f}s"
        )

    def test_run_fn_summary_contains_timestamp(self):
        """
        SCOPE: local unit test.
        The evidence summary must include 'ts=<ISO>' so it's visible
        in plan/result output without parsing raw_output.
        """
        evidence = _cilium_health_run_fn()

        assert "ts=" in evidence.summary, (
            f"[Sub-AC 2b] evidence.summary must include 'ts=<ISO>' for quick "
            f"visibility in execution plans. Got: {evidence.summary!r}"
        )


# ---------------------------------------------------------------------------
# TC-CR-3: Periodic re-verification via evidential dep enforcement
# ---------------------------------------------------------------------------

class TestCiliumPeriodicReverification:
    """
    Periodic health re-verification: stale cilium evidence must trigger
    RECHECK_TRIGGERED before any downstream consumer executes.
    """

    def _make_downstream_task(self) -> Task:
        """Return a stub downstream task that depends on cilium_health_verify:cni_status."""
        return Task(
            name="cilium_consumer_task",
            scope="test-scope:cilium-consumer",
            prerequisites=["cilium_health_verify"],
            evidence_deps=[
                EvidentialDep(
                    evidence_key="cilium_health_verify:cni_status",
                    source_task_name="cilium_health_verify",
                    max_age_s=600,
                ),
            ],
            run_fn=lambda: Evidence(
                captured_at=time.time(),
                raw_output="consumer ran",
                summary="consumer ok",
            ),
        )

    def test_stale_cilium_evidence_triggers_recheck(self, caplog):
        """
        SCOPE: local unit test.
        When cilium_health_verify:cni_status is STALE (age > TTL), a downstream
        task must trigger RECHECK_TRIGGERED before it executes.
        """
        cilium_task = Task(
            name="cilium_health_verify",
            scope="service:cilium — test scope",
            produces_evidence_key="cilium_health_verify:cni_status",
            run_fn=_cilium_health_run_fn,
        )
        consumer_task = self._make_downstream_task()

        executor = TaskExecutor([cilium_task, consumer_task], dry_run=True)
        # Seed STALE cilium evidence (15 minutes old)
        executor.seed_evidence(
            "cilium_health_verify:cni_status",
            raw_output="CILIUM_HEALTH_PROBE_TIMESTAMP=2026-03-19T00:00:00Z\nstale data",
            summary="cilium stale (old run)",
            age_seconds=900,  # 15 minutes — STALE
        )

        with caplog.at_level(logging.WARNING, logger="task_model"):
            executor.run()

        recheck_msgs = [
            r.message for r in caplog.records if "RECHECK_TRIGGERED" in r.message
        ]
        assert recheck_msgs, (
            "[Sub-AC 2b] RECHECK_TRIGGERED must be emitted when "
            "cilium_health_verify:cni_status is STALE. "
            f"All WARNING logs: {[r.message for r in caplog.records if r.levelno >= logging.WARNING]}"
        )
        # Evidence key must appear in the re-check message
        assert any("cilium_health_verify" in m for m in recheck_msgs), (
            f"RECHECK_TRIGGERED must reference cilium_health_verify. "
            f"Got: {recheck_msgs}"
        )

    def test_missing_cilium_evidence_triggers_recheck(self, caplog):
        """
        SCOPE: local unit test.
        When cilium_health_verify:cni_status is MISSING from the store,
        a downstream task must trigger RECHECK_TRIGGERED(reason=MISSING).
        """
        cilium_task = Task(
            name="cilium_health_verify",
            scope="service:cilium — test scope",
            produces_evidence_key="cilium_health_verify:cni_status",
            run_fn=_cilium_health_run_fn,
        )
        consumer_task = self._make_downstream_task()

        executor = TaskExecutor([cilium_task, consumer_task], dry_run=True)
        # No evidence seeded — MISSING

        with caplog.at_level(logging.WARNING, logger="task_model"):
            executor.run()

        recheck_missing = [
            r.message for r in caplog.records
            if "RECHECK_TRIGGERED" in r.message and "MISSING" in r.message
        ]
        assert recheck_missing, (
            "[Sub-AC 2b] RECHECK_TRIGGERED(MISSING) must be emitted when "
            "cilium_health_verify:cni_status is absent from the evidence store. "
            f"All logs: {[r.message for r in caplog.records]}"
        )

    def test_fresh_cilium_evidence_no_recheck(self, caplog):
        """
        SCOPE: local unit test.
        When cilium evidence is FRESH (age < TTL), no RECHECK_TRIGGERED is emitted.
        """
        cilium_task = Task(
            name="cilium_health_verify",
            scope="service:cilium — test scope",
            produces_evidence_key="cilium_health_verify:cni_status",
            run_fn=_cilium_health_run_fn,
        )
        consumer_task = self._make_downstream_task()

        executor = TaskExecutor([cilium_task, consumer_task], dry_run=True)
        executor.seed_evidence(
            "cilium_health_verify:cni_status",
            raw_output="CILIUM_HEALTH_PROBE_TIMESTAMP=2026-03-19T10:00:00Z\nfresh data",
            summary="cilium healthy (fresh)",
            age_seconds=30,  # FRESH
        )

        with caplog.at_level(logging.WARNING, logger="task_model"):
            executor.run()

        recheck_msgs = [
            r.message for r in caplog.records if "RECHECK_TRIGGERED" in r.message
        ]
        assert not recheck_msgs, (
            "[Sub-AC 2b] Fresh cilium evidence must NOT trigger RECHECK. "
            f"Got unexpected RECHECK_TRIGGERED: {recheck_msgs}"
        )

    def test_stale_cilium_evidence_triggers_live_recheck_of_run_fn(self):
        """
        SCOPE: local unit test — live run (non-dry-run).
        In non-dry-run mode, stale cilium evidence causes the executor to
        actually re-run _cilium_health_run_fn to capture fresh evidence.

        Verifies the periodic re-verification loop executes (not just logs).
        """
        recheck_count = [0]
        run_start = time.time()

        def counting_cilium_fn() -> Evidence:
            recheck_count[0] += 1
            ts_iso = "2026-03-19T10:00:00Z"
            return Evidence(
                captured_at=time.time(),
                raw_output=(
                    f"CILIUM_HEALTH_PROBE_TIMESTAMP={ts_iso}\n"
                    f"cilium pod: Running (recheck #{recheck_count[0]})"
                ),
                summary=f"cilium_health ts={ts_iso} exit=0 (recheck #{recheck_count[0]})",
            )

        cilium_task = Task(
            name="cilium_health_verify",
            scope="service:cilium — test scope",
            produces_evidence_key="cilium_health_verify:cni_status",
            run_fn=counting_cilium_fn,
        )
        consumer_task = Task(
            name="cilium_consumer",
            scope="test:consumer",
            prerequisites=["cilium_health_verify"],
            evidence_deps=[
                EvidentialDep(
                    evidence_key="cilium_health_verify:cni_status",
                    source_task_name="cilium_health_verify",
                    max_age_s=600,
                ),
            ],
            run_fn=lambda: Evidence(
                captured_at=time.time(),
                raw_output="consumer ran",
                summary="consumer ok",
            ),
        )

        executor = TaskExecutor([cilium_task, consumer_task], dry_run=False)
        # Seed STALE cilium evidence
        executor.seed_evidence(
            "cilium_health_verify:cni_status",
            raw_output="CILIUM_HEALTH_PROBE_TIMESTAMP=2026-01-01T00:00:00Z\nold data",
            summary="cilium stale",
            age_seconds=900,  # 15 minutes — STALE
        )

        executor.run()

        assert recheck_count[0] >= 1, (
            "[Sub-AC 2b] counting_cilium_fn must have been called at least once "
            "(re-verification triggered by stale evidence). "
            f"recheck_count={recheck_count[0]}"
        )

        # Verify the refreshed evidence has a timestamp within the current run window
        fresh_ev = executor._evidence_store.get("cilium_health_verify:cni_status")
        assert fresh_ev is not None, "Evidence store must contain fresh cilium evidence after recheck"
        assert fresh_ev.captured_at >= run_start, (
            f"[Sub-AC 2b] Re-captured cilium evidence.captured_at ({fresh_ev.captured_at:.3f}) "
            f"must be >= run_start ({run_start:.3f})"
        )
        assert "CILIUM_HEALTH_PROBE_TIMESTAMP=" in fresh_ev.raw_output, (
            "Re-captured cilium evidence must embed CILIUM_HEALTH_PROBE_TIMESTAMP= token"
        )


# ---------------------------------------------------------------------------
# TC-CR-4: Integration — cilium_health_verify in full ScaleX task graph
# ---------------------------------------------------------------------------

class TestCiliumGraphIntegration:
    """Integration checks — cilium_health_verify behaves correctly in the full pipeline."""

    def test_cilium_task_is_not_blocked_in_dry_run_with_seeded_evidence(
        self, caplog
    ):
        """
        SCOPE: local unit test — dry-run.
        With tower_post_install_verify:api_reachable and
        check_ssh_connectivity:reachability seeded as FRESH, cilium_health_verify
        should not be BLOCKED (all prerequisite evidence is available).

        This verifies the task can proceed once its deps are satisfied.
        """
        tasks = build_task_graph()
        executor = TaskExecutor(tasks, dry_run=True)

        # Seed all evidence as FRESH (5 seconds old)
        all_keys = [
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
            "cilium_health_verify:cni_status",
        ]
        for key in all_keys:
            executor.seed_evidence(
                key,
                raw_output=f"fresh evidence for {key}",
                summary="ok (fresh)",
                age_seconds=5,
            )

        results = executor.run()

        cilium_result = results.get("cilium_health_verify")
        assert cilium_result is not None, (
            "cilium_health_verify must appear in execution results"
        )
        # In dry-run mode with satisfied prerequisites, task should be SKIPPED (dry-run)
        # not BLOCKED
        assert cilium_result.status == TaskStatus.SKIPPED, (
            f"[Sub-AC 2b] With all prerequisites met, cilium_health_verify must be "
            f"SKIPPED (dry-run) not BLOCKED. Got: {cilium_result.status.name}. "
            f"block_reason: {cilium_result.block_reason}"
        )

    def test_cilium_evidence_key_in_controlled_vocabulary(self):
        """
        SCOPE: local unit test.
        The artifact 'cilium_health_verify:cni_status' must be in the controlled
        vocabulary with explicit granularity (Sub-AC 7c compliance).
        """
        from ops.artifact_vocabulary import ARTIFACT_REGISTRY, GranularityLevel
        key = "cilium_health_verify:cni_status"
        assert key in ARTIFACT_REGISTRY, (
            f"[Sub-AC 2b] {key!r} must be registered in ARTIFACT_REGISTRY. "
            f"Registered: {sorted(ARTIFACT_REGISTRY.keys())}"
        )
        descriptor = ARTIFACT_REGISTRY[key]
        assert descriptor.granularity is not None, (
            f"[Sub-AC 2b] {key!r} must have an explicit GranularityLevel"
        )
        assert isinstance(descriptor.granularity, GranularityLevel), (
            f"[Sub-AC 2b] {key!r}.granularity must be a GranularityLevel enum member"
        )
        # Cilium is a composite check → COARSE granularity
        assert descriptor.granularity == GranularityLevel.COARSE, (
            f"[Sub-AC 2b] cilium_health_verify:cni_status must be COARSE granularity "
            f"(it aggregates pod status + agent health + connectivity). "
            f"Got: {descriptor.granularity}"
        )

    def test_cilium_scope_artifact_in_artifact_registry(self):
        """
        SCOPE: local unit test.
        The artifact reference 'service:cilium' must be valid in ops/artifact_registry.py.
        This verifies cilium is a registered SERVICE-granularity artifact.
        """
        from ops.artifact_registry import parse_artifact_ref, ArtifactGranularity
        ref = parse_artifact_ref("service:cilium")
        assert ref.granularity == ArtifactGranularity.SERVICE, (
            f"'service:cilium' must have SERVICE granularity, got {ref.granularity}"
        )
        assert ref.name == "cilium"
