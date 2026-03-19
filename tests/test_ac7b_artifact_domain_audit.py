"""
tests/test_ac7b_artifact_domain_audit.py — Sub-AC 7b verification

Scope boundary (declared before evaluation):
  - Audits tests/task_model/scalex_tasks.py (14 operational task records).
  - Validates against ops/artifact_vocabulary.ARTIFACT_REGISTRY (evidence keys)
    and ops/artifact_registry.parse_artifact_ref() (scope_artifact_ids).
  - Test-scaffold tasks (test_dep_graph_enforcement.py, test_causal_deps.py)
    are explicitly OUT OF SCOPE — they use synthetic scope strings by design.
  - Test-scope vocabulary producers (kyverno_policy_check, sdi_status_reverify)
    are explicitly EXEMPT from the producing-task-in-graph check: they produce
    evidence only during test runs, not in the main pipeline.
  - No network calls, no SSH, no VMs, no Kubernetes cluster.

Known-acceptable-degradation inventory:
  (none — all tests in this suite must pass cleanly)

Evidence freshness: tests run in < 1 second; no evidence TTL concerns.

Gap report: docs/ac7b-artifact-domain-gap-report.yaml

Historical findings (now remediated):
  GAP-001 (tests/task_model/scalex_tasks.py:194):
    sdi_init.produces_evidence_key was "sdi_init:completion" — banned aspect,
    unregistered. Fixed to "sdi_init:vm_list" (GranularityLevel.FINE).
  GAP-002 (tests/task_model/scalex_tasks.py:212):
    sdi_verify_vms EvidentialDep[0].evidence_key was "sdi_init:completion".
    Fixed to "sdi_init:vm_list".
  GAP-003 (tests/task_model/scalex_tasks.py:241):
    sdi_health_check EvidentialDep[0].evidence_key was "sdi_init:completion".
    Fixed to "sdi_init:vm_list".

Current state: all 14 task records are artifact-domain-clean (0 gaps).
  (Task count grew from 13 → 14 when Sub-AC 2b added cilium_health_verify.)
"""

from __future__ import annotations

import pytest

from ops.artifact_registry import ArtifactRefError, parse_artifact_ref
from ops.artifact_vocabulary import ARTIFACT_REGISTRY, GranularityLevel
from tests.task_model.scalex_tasks import build_task_graph


# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

_BANNED_ASPECTS = frozenset({"completion", "done", "finished", "ran", "executed", "ok"})

ALL_TASK_NAMES = [
    "check_ssh_connectivity",
    "gather_hardware_facts",
    "sdi_init",
    "sdi_verify_vms",
    "sdi_health_check",
    "kubespray_tower",
    "tower_post_install_verify",
    "kubespray_sandbox",
    "gitops_bootstrap",
    "argocd_sync_healthy",
    "cf_tunnel_healthy",
    "dash_headless_verify",
    "scalex_dash_token_provisioned",
    # Sub-AC 2b added this periodic Cilium CNI health re-verification task:
    "cilium_health_verify",
]

# Vocabulary entries produced by TEST-SCOPE tasks (not pipeline tasks).
# These are re-verification tasks executed only during test runs:
#   kyverno_policy_check  — Sub-AC 2c: re-verify Kyverno policies in test
#   sdi_status_reverify   — Sub-AC 5c: re-verify SDI health in test
# They are exempt from the "every vocabulary key must have a pipeline task"
# check because they are not intended to run in the main pipeline.
_TEST_SCOPE_PRODUCERS: frozenset[str] = frozenset({
    "kyverno_policy_check",   # Sub-AC 2c: Kyverno policy re-verification test
    "sdi_status_reverify",    # Sub-AC 5c: SDI component health re-verification test
})


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _get_task(name: str):
    tasks = {t.name: t for t in build_task_graph()}
    if name not in tasks:
        pytest.fail(f"Task '{name}' not found in build_task_graph()")
    return tasks[name]


def _audit_task(task) -> list[dict]:
    """
    Audit a single task record.  Returns a list of gap dicts (empty if clean).

    Checks:
      1. produces_evidence_key: must be in ARTIFACT_REGISTRY (vocabulary).
      2. EvidentialDep.evidence_key: each dep key must be in ARTIFACT_REGISTRY.
      3. scope_artifact_ids: each ref must parse via parse_artifact_ref().
      4. Granularity: produces_key descriptor must have a GranularityLevel.
    """
    gaps = []

    # 1. produces_evidence_key
    key = task.produces_evidence_key
    if not key:
        gaps.append({
            "gap_type": "MISSING_PRODUCES_KEY",
            "field": "produces_evidence_key",
            "value": repr(key),
        })
    elif key not in ARTIFACT_REGISTRY:
        parts = key.split(":", 1)
        aspect = parts[1] if len(parts) > 1 else ""
        detail = (
            f"banned aspect {aspect!r}"
            if aspect.lower() in _BANNED_ASPECTS
            else "not in artifact vocabulary"
        )
        gaps.append({
            "gap_type": "UNREGISTERED_PRODUCES_KEY",
            "field": "produces_evidence_key",
            "value": key,
            "detail": detail,
        })
    else:
        descriptor = ARTIFACT_REGISTRY[key]
        if not isinstance(descriptor.granularity, GranularityLevel):
            gaps.append({
                "gap_type": "MISSING_GRANULARITY",
                "field": "produces_evidence_key",
                "value": key,
            })

    # 2. evidential dep keys
    for dep in getattr(task, "evidence_deps", []):
        ekey = dep.evidence_key
        if ekey not in ARTIFACT_REGISTRY:
            parts = ekey.split(":", 1)
            aspect = parts[1] if len(parts) > 1 else ""
            detail = (
                f"banned aspect {aspect!r}"
                if aspect.lower() in _BANNED_ASPECTS
                else "not in artifact vocabulary"
            )
            gaps.append({
                "gap_type": "UNREGISTERED_EVIDENTIAL_DEP_KEY",
                "field": f"evidence_deps[evidence_key={ekey!r}]",
                "value": ekey,
                "detail": detail,
            })

    # 3. scope_artifact_ids
    for ref in getattr(task, "scope_artifact_ids", []):
        try:
            parse_artifact_ref(ref)
        except ArtifactRefError as e:
            gaps.append({
                "gap_type": "UNREGISTERED_SCOPE_ARTIFACT_ID",
                "field": f"scope_artifact_ids[{ref!r}]",
                "value": ref,
                "detail": str(e),
            })

    return gaps


# ---------------------------------------------------------------------------
# 1. All 13 task records must be gap-free (current state)
# ---------------------------------------------------------------------------

class TestAllTasksClean:
    """
    Regression guard: every task record in the pipeline must have
    fully registered, granularity-bearing artifact domain references.

    Historical gaps (GAP-001/002/003) have been remediated; this suite
    ensures they do not regress and catches any new gaps introduced.
    """

    @pytest.mark.parametrize("task_name", ALL_TASK_NAMES)
    def test_task_has_no_artifact_domain_gaps(self, task_name: str):
        """Each task record must produce zero artifact domain audit findings."""
        task = _get_task(task_name)
        gaps = _audit_task(task)
        assert gaps == [], (
            f"Task '{task_name}' has {len(gaps)} artifact domain gap(s):\n"
            + "\n".join(
                f"  [{g['gap_type']}] {g['field']}: {g['value']!r}"
                + (f" — {g['detail']}" if "detail" in g else "")
                for g in gaps
            )
            + "\nSee docs/ac7b-artifact-domain-gap-report.yaml for remediation guidance."
        )

    @pytest.mark.parametrize("task_name", ALL_TASK_NAMES)
    def test_task_produces_key_is_registered(self, task_name: str):
        """Every task must have a produces_evidence_key registered in the vocabulary."""
        task = _get_task(task_name)
        key = task.produces_evidence_key
        assert key, f"Task '{task_name}' has no produces_evidence_key"
        assert key in ARTIFACT_REGISTRY, (
            f"Task '{task_name}' produces_evidence_key {key!r} not in "
            "ops/artifact_vocabulary.ARTIFACT_REGISTRY"
        )

    @pytest.mark.parametrize("task_name", ALL_TASK_NAMES)
    def test_task_produces_key_has_explicit_granularity(self, task_name: str):
        """Every registered produces_evidence_key must carry an explicit GranularityLevel."""
        task = _get_task(task_name)
        key = task.produces_evidence_key
        if key and key in ARTIFACT_REGISTRY:
            descriptor = ARTIFACT_REGISTRY[key]
            assert isinstance(descriptor.granularity, GranularityLevel), (
                f"Task '{task_name}' key {key!r}: granularity is not a GranularityLevel "
                f"(got {type(descriptor.granularity).__name__!r})"
            )

    @pytest.mark.parametrize("task_name", ALL_TASK_NAMES)
    def test_task_produces_key_aspect_not_banned(self, task_name: str):
        """produces_evidence_key aspect must not be a banned lifecycle verb."""
        task = _get_task(task_name)
        key = task.produces_evidence_key
        if key:
            parts = key.split(":", 1)
            aspect = parts[1] if len(parts) > 1 else ""
            assert aspect.lower() not in _BANNED_ASPECTS, (
                f"Task '{task_name}' produces_evidence_key {key!r}: aspect {aspect!r} "
                f"is a banned lifecycle verb.  Banned set: {sorted(_BANNED_ASPECTS)}"
            )

    @pytest.mark.parametrize("task_name", ALL_TASK_NAMES)
    def test_task_scope_artifact_ids_all_registered(self, task_name: str):
        """All scope_artifact_ids entries must parse without error."""
        task = _get_task(task_name)
        for ref in getattr(task, "scope_artifact_ids", []):
            try:
                parse_artifact_ref(ref)
            except ArtifactRefError as e:
                pytest.fail(
                    f"Task '{task_name}' scope_artifact_ids contains unregistered "
                    f"ref {ref!r}: {e}"
                )

    @pytest.mark.parametrize("task_name", ALL_TASK_NAMES)
    def test_task_evidential_dep_keys_all_registered(self, task_name: str):
        """All EvidentialDep.evidence_key values must be in the vocabulary."""
        task = _get_task(task_name)
        for dep in getattr(task, "evidence_deps", []):
            assert dep.evidence_key in ARTIFACT_REGISTRY, (
                f"Task '{task_name}' EvidentialDep references unregistered "
                f"evidence_key {dep.evidence_key!r}"
            )


# ---------------------------------------------------------------------------
# 2. Regression guards for the historical gaps (GAP-001/002/003)
# ---------------------------------------------------------------------------

class TestHistoricalGapRegressionGuards:
    """
    Ensure the three historical gaps (remediated in Sub-AC 7c) do not
    re-appear.  Each test checks the positive form: the correct registered
    key is in place.

    If any of these fail, the corresponding historical gap has regressed.
    """

    def test_gap_001_sdi_init_uses_registered_key(self):
        """
        GAP-001 regression guard:
        sdi_init.produces_evidence_key must be "sdi_init:vm_list" (FINE),
        NOT "sdi_init:completion" (unregistered, banned aspect).

        File: tests/task_model/scalex_tasks.py (~line 214)
        """
        task = _get_task("sdi_init")
        assert task.produces_evidence_key == "sdi_init:vm_list", (
            f"GAP-001 regressed! sdi_init.produces_evidence_key is "
            f"{task.produces_evidence_key!r}, expected 'sdi_init:vm_list'."
        )
        assert "sdi_init:vm_list" in ARTIFACT_REGISTRY
        assert ARTIFACT_REGISTRY["sdi_init:vm_list"].granularity == GranularityLevel.FINE

    def test_gap_002_sdi_verify_vms_uses_registered_dep_key(self):
        """
        GAP-002 regression guard:
        sdi_verify_vms must reference "sdi_init:vm_list" as its evidential dep,
        NOT "sdi_init:completion".

        File: tests/task_model/scalex_tasks.py (~line 230)
        """
        task = _get_task("sdi_verify_vms")
        dep_keys = [dep.evidence_key for dep in task.evidence_deps]
        assert "sdi_init:vm_list" in dep_keys, (
            f"GAP-002 regressed! sdi_verify_vms EvidentialDep keys are {dep_keys}, "
            "expected 'sdi_init:vm_list' to be present."
        )
        assert "sdi_init:completion" not in dep_keys, (
            "GAP-002 regressed! 'sdi_init:completion' (banned aspect) re-appeared in "
            "sdi_verify_vms.evidence_deps"
        )

    def test_gap_003_sdi_health_check_uses_registered_dep_key(self):
        """
        GAP-003 regression guard:
        sdi_health_check must reference "sdi_init:vm_list" as its evidential dep,
        NOT "sdi_init:completion".

        File: tests/task_model/scalex_tasks.py (~line 256)
        """
        task = _get_task("sdi_health_check")
        dep_keys = [dep.evidence_key for dep in task.evidence_deps]
        assert "sdi_init:vm_list" in dep_keys, (
            f"GAP-003 regressed! sdi_health_check EvidentialDep keys are {dep_keys}, "
            "expected 'sdi_init:vm_list' to be present."
        )
        assert "sdi_init:completion" not in dep_keys, (
            "GAP-003 regressed! 'sdi_init:completion' (banned aspect) re-appeared in "
            "sdi_health_check.evidence_deps"
        )

    def test_sdi_init_vm_list_registered_with_fine_granularity(self):
        """
        The authoritative registered key 'sdi_init:vm_list' must remain in
        the vocabulary with GranularityLevel.FINE.
        """
        assert "sdi_init:vm_list" in ARTIFACT_REGISTRY, (
            "'sdi_init:vm_list' must be registered in ops/artifact_vocabulary"
        )
        descriptor = ARTIFACT_REGISTRY["sdi_init:vm_list"]
        assert descriptor.granularity == GranularityLevel.FINE
        assert descriptor.produced_by == "sdi_init"

    def test_sdi_init_completion_remains_unregistered(self):
        """
        'sdi_init:completion' must NOT be added to the vocabulary — it is a
        banned lifecycle-verb aspect that was correctly excluded.
        """
        assert "sdi_init:completion" not in ARTIFACT_REGISTRY, (
            "'sdi_init:completion' must NOT be registered (it uses a banned "
            "lifecycle-verb aspect).  Use 'sdi_init:vm_list' instead."
        )

    def test_docstring_diagram_references_correct_key(self):
        """
        The module in scalex_tasks.py must reference 'sdi_init:vm_list'
        (not 'sdi_init:completion') — ensuring documentation stays aligned.
        """
        import inspect
        from tests.task_model import scalex_tasks
        source = inspect.getsource(scalex_tasks)
        assert "sdi_init:vm_list" in source, (
            "scalex_tasks.py source must contain 'sdi_init:vm_list' "
            "(should appear in both the docstring diagram and the code)"
        )


# ---------------------------------------------------------------------------
# 3. Full audit integration summary
# ---------------------------------------------------------------------------

class TestAuditSummary:
    """
    Integration check: runs the complete audit and verifies the gap counts
    match the current clean state documented in the gap report.
    """

    def test_audit_finds_zero_gaps(self):
        """
        Full audit of all 14 task records must find zero gaps.
        This locks in the post-remediation baseline from Sub-AC 7b/7c.
        (Count grew 13→14 when Sub-AC 2b added cilium_health_verify.)
        """
        tasks = build_task_graph()
        all_gaps = []
        for task in tasks:
            task_gaps = _audit_task(task)
            for g in task_gaps:
                g["task"] = task.name
            all_gaps.extend(task_gaps)

        assert len(all_gaps) == 0, (
            f"Expected zero artifact domain gaps but found {len(all_gaps)}:\n"
            + "\n".join(
                f"  [{g['gap_type']}] task={g['task']!r} "
                f"value={g.get('value')!r} detail={g.get('detail', '')!r}"
                for g in all_gaps
            )
            + "\nUpdate docs/ac7b-artifact-domain-gap-report.yaml with new findings."
        )

    def test_audit_total_task_count(self):
        """build_task_graph() must return exactly 14 tasks (full pipeline).
        Count grew 13→14 when Sub-AC 2b added cilium_health_verify.
        """
        tasks = build_task_graph()
        assert len(tasks) == 14, (
            f"Expected 14 tasks in build_task_graph(), found {len(tasks)}.  "
            "Update ALL_TASK_NAMES and docs/ac7b-artifact-domain-gap-report.yaml "
            "if tasks were added or removed."
        )

    def test_audit_all_task_names_present(self):
        """The ALL_TASK_NAMES constant must match build_task_graph() task names."""
        task_names = {t.name for t in build_task_graph()}
        assert task_names == set(ALL_TASK_NAMES), (
            f"Mismatch between ALL_TASK_NAMES and actual task names.\n"
            f"Extra in ALL_TASK_NAMES: {set(ALL_TASK_NAMES) - task_names}\n"
            f"Missing from ALL_TASK_NAMES: {task_names - set(ALL_TASK_NAMES)}"
        )

    def test_every_registered_vocabulary_key_has_producing_task(self):
        """
        Every key in the artifact vocabulary must be produced by a task that
        exists in build_task_graph() OR is explicitly listed in
        _TEST_SCOPE_PRODUCERS.

        Test-scope producers (kyverno_policy_check, sdi_status_reverify) produce
        evidence only during test runs and are not part of the main pipeline.
        They are exempt from this check to avoid false orphan reports.

        Truly orphaned entries (produced_by references a non-existent task that
        is also NOT in _TEST_SCOPE_PRODUCERS) indicate stale registrations and
        must be remediated.
        """
        from ops.artifact_vocabulary import ARTIFACT_REGISTRY as vocab
        task_names = {t.name for t in build_task_graph()}
        orphaned = [
            key for key, desc in vocab.items()
            if desc.produced_by not in task_names
            and desc.produced_by not in _TEST_SCOPE_PRODUCERS
        ]
        assert orphaned == [], (
            f"Orphaned vocabulary entries (producing task not in graph and not "
            f"in _TEST_SCOPE_PRODUCERS):\n"
            + "\n".join(f"  {k!r} (produced_by={vocab[k].produced_by!r})"
                        for k in orphaned)
        )
