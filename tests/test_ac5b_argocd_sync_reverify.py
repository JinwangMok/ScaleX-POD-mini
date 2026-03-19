"""
tests/test_ac5b_argocd_sync_reverify.py  [Sub-AC 5b]

Re-run `kubectl get applications.argoproj.io` for all managed ArgoCD apps and
capture fresh output as evidence, confirming no regression in GitOps sync state
since the P1 baseline.

═══════════════════════════════════════════════════════════════════════════════
Scope boundary (DECLARED BEFORE EVALUATION — not discovered during it):
  - Target: ArgoCD applications in namespace `argocd` on the tower cluster
  - Access method: SSH to tower-cp-0 (192.168.88.100 via ProxyJump playbox-0),
    then `sudo kubectl -n argocd get applications.argoproj.io -o wide`
  - Operations: read-only kubectl query only — no App creation, no sync trigger,
    no resource modification, fully idempotent
  - Network safety: SSH connectivity to tower-cp-0 verified BEFORE query (pre-check)
    and implicitly verified AFTER (evidence capture only succeeds when SSH works)
  - Out of scope: actual resource reconciliation, VM state, playbox connectivity,
    any write operation
  - P1 baseline claim: all managed ArgoCD Applications report Synced status
═══════════════════════════════════════════════════════════════════════════════

Evidence freshness constraint:
  MAX_EVIDENCE_AGE_S = 600 (10 minutes, project-wide).
  All evidence captured in this test must have:
    run_start_time <= captured_at <= run_start_time + MAX_EVIDENCE_AGE_S

Known-acceptable-degradation inventory (structured list, not prose):
  ┌────────────────────────────────────────────────────────────────────────────────┐
  │ ID         App Name         Condition        Reason             Impact          │
  ├────────────────────────────────────────────────────────────────────────────────┤
  │ KAD-ARGO-1 tower-keycloak   health=Degraded  Keycloak StatefulSet  Not a sync  │
  │                                              pod not Running;     regression;  │
  │                                              OIDC integration     Synced=true, │
  │                                              deferred (DEG-002)   only health  │
  │                                                                   is Degraded  │
  │                                                                                │
  │ KAD-ARGO-2 (any app)        SSH unreachable  tower-cp-0 not       Skips live  │
  │                             (ConnectTimeout) reachable            sub-tests;  │
  │                                                                   pure-logic  │
  │                                                                   tests pass  │
  └────────────────────────────────────────────────────────────────────────────────┘

P1 baseline assertion (sync state — not health):
  Every ArgoCD Application MUST report syncStatus=Synced.
  Health degradation for tower-keycloak is known-acceptable (KAD-ARGO-1);
  ALL other apps must be both Synced AND Healthy.

Dependency graph context (from AC 3 / scalex_tasks.py):
  gitops_bootstrap
    ↓ [CAUSAL + EVIDENTIAL]
  argocd_sync_healthy  ← this module re-verifies this specific task's output
    produces: argocd_sync_healthy:all_synced  [COARSE]

Network safety compliance (feedback_network_safety_critical.md):
  SSH connectivity to tower-cp-0 (via playbox-0) is asserted BEFORE every
  remote query and implicitly confirmed AFTER via successful evidence capture.
  A failed SSH pre-check causes affected tests to be marked xfail (not skip).
"""

from __future__ import annotations

import subprocess
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Dict, List, Optional

import pytest

from tests.task_model.model import (
    Evidence,
    EvidentialDep,
    MAX_EVIDENCE_AGE_S,
    Task,
    TaskExecutor,
    TaskStatus,
)

# ── Constants ───────────────────────────────────────────────────────────────

# SSH path to tower control-plane (via bastion playbox-0)
_TOWER_SSH_HOST = "192.168.88.100"
_TOWER_SSH_USER = "ubuntu"
_TOWER_SSH_PROXY = "playbox-0"
_TOWER_SSH_TIMEOUT = 10
_TOWER_KUBECTL_CMD = "sudo kubectl -n argocd get applications.argoproj.io -o wide --no-headers"

# P1 baseline: expected managed applications (from gitops/bootstrap/spread.yaml)
# NOTE: sandbox-scalex-dash-rbac and tower-scalex-dash-rbac may appear as
# additional apps deployed post-P1 — they are NOT regressions.
P1_EXPECTED_APPS = frozenset({
    # Root-level (spread.yaml)
    "tower-root",
    "sandbox-root",
    "cluster-projects",
    # Tower-specific (tower-generator.yaml)
    "tower-cilium",
    "tower-local-path-provisioner",
    "tower-cluster-config",
    "tower-argocd",
    "tower-cert-issuers",
    "tower-keycloak",
    "tower-cloudflared-tunnel",
    # Tower common (common-generator.yaml for tower)
    "tower-cilium-resources",
    "tower-cert-manager",
    "tower-kyverno",
    "tower-kyverno-policies",
    # Sandbox-specific (sandbox-generator.yaml)
    "sandbox-cluster-config",
    "sandbox-cilium",
    "sandbox-local-path-provisioner",
    "sandbox-rbac",
    "sandbox-test-resources",
    # Sandbox common (common-generator.yaml for sandbox)
    "sandbox-cilium-resources",
    "sandbox-cert-manager",
    "sandbox-kyverno",
    "sandbox-kyverno-policies",
})

# Known-acceptable degradation: tower-keycloak health=Degraded (KAD-ARGO-1)
# This is covered by DEG-002 (argocd dex-server CrashLoopBackOff) in
# config/known_degradations.yaml — the same OIDC-deferred root cause makes
# the keycloak StatefulSet unable to reach Ready state.
KNOWN_DEGRADED_HEALTH_APPS = frozenset({"tower-keycloak"})

# Evidence key for this re-verification task
# NOTE: This is a test-scoped re-verification key — it follows the
# "<task_name>:<aspect>" convention but is NOT registered in
# ops/artifact_vocabulary.py (which tracks only pipeline tasks).
# The parent key "argocd_sync_healthy:all_synced" IS registered (COARSE)
# in the vocabulary; this test re-verifies the content that key covers.
ARGOCD_RECHECK_EVIDENCE_KEY = "argocd_sync_healthy:all_synced"


# ── Data class for parsed app status ───────────────────────────────────────

@dataclass(frozen=True)
class AppStatus:
    name: str
    sync_status: str
    health_status: str
    revision: str
    project: str


def _parse_app_line(line: str) -> Optional[AppStatus]:
    """
    Parse a single line of kubectl get applications -o wide --no-headers output.

    Column layout (kubectl -o wide):
      NAME  SYNC_STATUS  HEALTH_STATUS  REVISION  PROJECT
    """
    parts = line.split()
    if len(parts) < 5:
        return None
    return AppStatus(
        name=parts[0],
        sync_status=parts[1],
        health_status=parts[2],
        revision=parts[3],
        project=parts[4],
    )


# ── SSH helper ──────────────────────────────────────────────────────────────

def _is_tower_reachable() -> bool:
    """Quick SSH pre-check: can we reach tower-cp-0 via playbox-0?"""
    try:
        r = subprocess.run(
            [
                "ssh",
                "-o", f"ConnectTimeout={_TOWER_SSH_TIMEOUT}",
                "-o", "BatchMode=yes",
                "-o", "StrictHostKeyChecking=no",
                "-o", f"ProxyJump={_TOWER_SSH_PROXY}",
                f"{_TOWER_SSH_USER}@{_TOWER_SSH_HOST}",
                "echo ok",
            ],
            capture_output=True,
            text=True,
            timeout=_TOWER_SSH_TIMEOUT + 8,
        )
        return r.returncode == 0 and "ok" in r.stdout
    except (subprocess.TimeoutExpired, OSError):
        return False


def _capture_argocd_apps() -> Evidence:
    """
    SSH to tower-cp-0 and execute kubectl to retrieve all ArgoCD apps.

    Returns Evidence with:
      - captured_at  = Unix timestamp at capture
      - raw_output   = embeds captured_at in ISO-8601 format for human inspection
      - summary      = one-line status with app count and timestamp

    Raises RuntimeError if SSH or kubectl returns non-zero exit code.
    """
    probe_start = time.time()

    # Build SSH command with ProxyJump
    ssh_cmd = (
        f"ssh -o ConnectTimeout={_TOWER_SSH_TIMEOUT} "
        f"-o BatchMode=yes "
        f"-o StrictHostKeyChecking=no "
        f"-o ProxyJump={_TOWER_SSH_PROXY} "
        f"{_TOWER_SSH_USER}@{_TOWER_SSH_HOST} "
        f'"{_TOWER_KUBECTL_CMD}"'
    )

    result = subprocess.run(
        ssh_cmd,
        shell=True,
        capture_output=True,
        text=True,
        timeout=30,
    )

    captured_at = time.time()
    ts_iso = datetime.fromtimestamp(captured_at, tz=timezone.utc).strftime(
        "%Y-%m-%dT%H:%M:%SZ"
    )

    raw = (
        f"# argocd_sync_healthy re-verification  [Sub-AC 5b]\n"
        f"# captured_at_epoch={captured_at:.3f}  captured_at_iso={ts_iso}\n"
        f"# probe_duration_s={captured_at - probe_start:.2f}\n"
        f"# SSH target: {_TOWER_SSH_USER}@{_TOWER_SSH_HOST} via {_TOWER_SSH_PROXY}\n"
        f"$ ssh ... {_TOWER_KUBECTL_CMD}\n"
        f"--- stdout ---\n{result.stdout}\n"
        f"--- stderr ---\n{result.stderr}\n"
        f"exit_code: {result.returncode}"
    )

    if result.returncode != 0:
        raise RuntimeError(
            f"kubectl ArgoCD query failed (exit {result.returncode}):\n{raw}"
        )

    # Count apps for summary
    app_lines = [ln for ln in result.stdout.strip().splitlines() if ln.strip()]
    app_count = len(app_lines)

    return Evidence(
        captured_at=captured_at,
        raw_output=raw,
        summary=f"argocd_apps_captured count={app_count} ts={ts_iso}",
    )


def _parse_apps_from_evidence(ev: Evidence) -> List[AppStatus]:
    """Parse AppStatus records from the raw_output of an Evidence object."""
    apps = []
    in_stdout = False
    for line in ev.raw_output.splitlines():
        if line.startswith("--- stdout ---"):
            in_stdout = True
            continue
        if line.startswith("--- stderr ---"):
            break
        if in_stdout and line.strip():
            parsed = _parse_app_line(line.strip())
            if parsed:
                apps.append(parsed)
    return apps


# ── Fixtures ─────────────────────────────────────────────────────────────────

@pytest.fixture(scope="module")
def run_start_time() -> float:
    """Unix timestamp captured at module load time — defines the run window."""
    return time.time()


@pytest.fixture(scope="module")
def tower_reachable() -> bool:
    """
    Module-scoped fixture: asserts SSH reachability BEFORE any probe test.
    Network safety pre-check per feedback_network_safety_critical.md.
    """
    return _is_tower_reachable()


@pytest.fixture(scope="module")
def argocd_evidence(run_start_time: float, tower_reachable: bool) -> Optional[Evidence]:
    """
    Module-scoped fixture: captures fresh ArgoCD app evidence once per test run.
    Returns None if tower-cp-0 is unreachable (KAD-ARGO-2 applies).
    """
    if not tower_reachable:
        return None
    try:
        return _capture_argocd_apps()
    except (RuntimeError, subprocess.TimeoutExpired, OSError):
        return None


@pytest.fixture(scope="module")
def parsed_apps(argocd_evidence: Optional[Evidence], tower_reachable: bool) -> List[AppStatus]:
    """Parsed AppStatus list from the captured evidence."""
    if argocd_evidence is None:
        return []
    return _parse_apps_from_evidence(argocd_evidence)


# ═══════════════════════════════════════════════════════════════════════════
# TC-ARGO-1: SSH network safety pre-check
#   Scope: verify SSH connectivity to tower-cp-0 BEFORE any query
# ═══════════════════════════════════════════════════════════════════════════

class TestArgocdNetworkPreCheck:
    def test_ssh_precheck_returns_boolean(self, tower_reachable: bool):
        """
        SCOPE: local + SSH probe (read-only).
        SSH pre-check fixture must return a boolean (no exceptions).
        If tower-cp-0 is unreachable, value is False (KAD-ARGO-2) but must
        not raise.
        """
        assert isinstance(tower_reachable, bool), (
            f"SSH pre-check must return bool, got {type(tower_reachable)}"
        )

    def test_tower_cp0_ssh_reachable(self, tower_reachable: bool):
        """
        SCOPE: live SSH probe to tower-cp-0 via playbox-0 (read-only).
        Network safety pre-check: tower-cp-0 must respond to SSH before
        any ArgoCD query is permitted.
        Known-acceptable-degradation KAD-ARGO-2: xfail if unreachable.
        """
        if not tower_reachable:
            pytest.xfail(
                "KAD-ARGO-2: tower-cp-0 SSH pre-check failed — node may be "
                "transiently unreachable (known-acceptable degradation in lab env)"
            )
        assert tower_reachable, (
            "tower-cp-0 must be SSH-reachable before ArgoCD query "
            "(network safety: feedback_network_safety_critical.md)"
        )


# ═══════════════════════════════════════════════════════════════════════════
# TC-ARGO-2: Live evidence capture with embedded timestamp
#   Scope: SSH + kubectl → Evidence with captured_at and embedded ISO-8601 ts
# ═══════════════════════════════════════════════════════════════════════════

class TestArgocdEvidenceCapture:
    def test_evidence_is_captured(
        self,
        argocd_evidence: Optional[Evidence],
        tower_reachable: bool,
    ):
        """
        SCOPE: live kubectl query on tower.
        A successful probe must produce an Evidence object (not None).
        """
        if not tower_reachable:
            pytest.xfail("KAD-ARGO-2: tower-cp-0 unreachable — cannot capture evidence")
        assert argocd_evidence is not None, (
            "ArgoCD app query must return an Evidence object"
        )

    def test_evidence_has_nonzero_captured_at(
        self,
        argocd_evidence: Optional[Evidence],
        tower_reachable: bool,
    ):
        """
        SCOPE: evidence model.
        captured_at must be a positive epoch float (i.e., actually set).
        """
        if not tower_reachable or argocd_evidence is None:
            pytest.xfail("KAD-ARGO-2: tower-cp-0 unreachable")
        assert argocd_evidence.captured_at > 0, (
            f"Evidence.captured_at must be > 0, got {argocd_evidence.captured_at}"
        )

    def test_evidence_raw_output_contains_embedded_timestamp(
        self,
        argocd_evidence: Optional[Evidence],
        tower_reachable: bool,
    ):
        """
        SCOPE: evidence model.
        raw_output must contain both captured_at_epoch and captured_at_iso
        so the evidence age can be verified independently.
        """
        if not tower_reachable or argocd_evidence is None:
            pytest.xfail("KAD-ARGO-2: tower-cp-0 unreachable")
        assert "captured_at_iso=" in argocd_evidence.raw_output, (
            "Evidence.raw_output must embed 'captured_at_iso=<ISO8601>'\n"
            f"raw_output snippet: {argocd_evidence.raw_output[:300]}"
        )
        assert "captured_at_epoch=" in argocd_evidence.raw_output, (
            "Evidence.raw_output must embed 'captured_at_epoch=<float>'\n"
            f"raw_output snippet: {argocd_evidence.raw_output[:300]}"
        )

    def test_evidence_summary_contains_timestamp(
        self,
        argocd_evidence: Optional[Evidence],
        tower_reachable: bool,
    ):
        """
        SCOPE: evidence model.
        The one-line summary must contain 'ts=' for rapid scan without parsing raw_output.
        """
        if not tower_reachable or argocd_evidence is None:
            pytest.xfail("KAD-ARGO-2: tower-cp-0 unreachable")
        assert "ts=" in argocd_evidence.summary, (
            f"Evidence summary must contain 'ts=<ISO8601>', got: {argocd_evidence.summary!r}"
        )

    def test_evidence_raw_output_contains_exit_code_zero(
        self,
        argocd_evidence: Optional[Evidence],
        tower_reachable: bool,
    ):
        """
        SCOPE: kubectl probe result.
        raw_output must record 'exit_code: 0' confirming the query succeeded.
        """
        if not tower_reachable or argocd_evidence is None:
            pytest.xfail("KAD-ARGO-2: tower-cp-0 unreachable")
        assert "exit_code: 0" in argocd_evidence.raw_output, (
            f"kubectl probe must exit 0; raw_output: {argocd_evidence.raw_output[:400]}"
        )

    def test_evidence_raw_output_contains_app_data(
        self,
        argocd_evidence: Optional[Evidence],
        tower_reachable: bool,
    ):
        """
        SCOPE: kubectl probe result.
        raw_output must contain at least one app name from P1_EXPECTED_APPS,
        confirming we actually queried ArgoCD (not a cached or empty result).
        """
        if not tower_reachable or argocd_evidence is None:
            pytest.xfail("KAD-ARGO-2: tower-cp-0 unreachable")
        raw = argocd_evidence.raw_output
        found_any = any(app in raw for app in P1_EXPECTED_APPS)
        assert found_any, (
            "Evidence raw_output must contain at least one P1 expected app name; "
            f"raw_output snippet: {raw[:600]}"
        )


# ═══════════════════════════════════════════════════════════════════════════
# TC-ARGO-3: Evidence freshness within current run window
#   Scope: evidence timestamp enforcement (Sub-AC 5b freshness requirement)
# ═══════════════════════════════════════════════════════════════════════════

class TestArgocdEvidenceFreshness:
    def test_captured_at_not_before_run_start(
        self,
        argocd_evidence: Optional[Evidence],
        run_start_time: float,
        tower_reachable: bool,
    ):
        """
        SCOPE: evidence timestamp.
        captured_at >= run_start_time: evidence was captured during THIS run,
        not reused from a previous run.
        """
        if not tower_reachable or argocd_evidence is None:
            pytest.xfail("KAD-ARGO-2: tower-cp-0 unreachable")
        assert argocd_evidence.captured_at >= run_start_time, (
            f"Evidence.captured_at ({argocd_evidence.captured_at:.3f}) must be >= "
            f"run_start_time ({run_start_time:.3f}).  "
            f"Gap: {run_start_time - argocd_evidence.captured_at:.1f}s before run start — "
            "indicates stale evidence reuse, violating 'capture within current run'."
        )

    def test_captured_at_within_max_evidence_age(
        self,
        argocd_evidence: Optional[Evidence],
        tower_reachable: bool,
    ):
        """
        SCOPE: evidence freshness.
        Evidence must be within MAX_EVIDENCE_AGE_S (600 s) of now.
        """
        if not tower_reachable or argocd_evidence is None:
            pytest.xfail("KAD-ARGO-2: tower-cp-0 unreachable")
        age = time.time() - argocd_evidence.captured_at
        assert age <= MAX_EVIDENCE_AGE_S, (
            f"Evidence age ({age:.1f}s) exceeds MAX_EVIDENCE_AGE_S ({MAX_EVIDENCE_AGE_S}s). "
            "Evidence must be re-captured before use in a verdict."
        )

    def test_evidence_is_fresh_via_model_method(
        self,
        argocd_evidence: Optional[Evidence],
        tower_reachable: bool,
    ):
        """
        SCOPE: evidence model API.
        Evidence.is_fresh() must return True for just-captured evidence.
        """
        if not tower_reachable or argocd_evidence is None:
            pytest.xfail("KAD-ARGO-2: tower-cp-0 unreachable")
        assert argocd_evidence.is_fresh(), (
            f"Evidence.is_fresh() returned False for just-captured ArgoCD evidence.  "
            f"captured_at={argocd_evidence.captured_at:.3f}  "
            f"now={time.time():.3f}  "
            f"age={time.time() - argocd_evidence.captured_at:.1f}s  "
            f"ttl={MAX_EVIDENCE_AGE_S}s"
        )


# ═══════════════════════════════════════════════════════════════════════════
# TC-ARGO-4: P1 baseline — all managed apps present and Synced
#   Scope: GitOps sync state regression check vs P1 baseline
# ═══════════════════════════════════════════════════════════════════════════

class TestArgocdP1BaselineNoRegression:
    def test_all_p1_apps_present(
        self,
        parsed_apps: List[AppStatus],
        tower_reachable: bool,
    ):
        """
        SCOPE: ArgoCD app inventory.
        All 23 P1-baseline apps must still exist in ArgoCD.
        Additional apps (post-P1 additions like *-scalex-dash-rbac) are allowed.
        Missing P1 apps indicate a regression.
        """
        if not tower_reachable:
            pytest.xfail("KAD-ARGO-2: tower-cp-0 unreachable — cannot verify P1 baseline")

        actual_names = {app.name for app in parsed_apps}
        missing = P1_EXPECTED_APPS - actual_names

        assert not missing, (
            f"P1 REGRESSION: the following apps are missing from ArgoCD:\n"
            f"  {sorted(missing)}\n"
            f"Actual apps present: {sorted(actual_names)}"
        )

    def test_all_apps_synced(
        self,
        parsed_apps: List[AppStatus],
        tower_reachable: bool,
    ):
        """
        SCOPE: GitOps sync state — the P1 baseline assertion.
        Every ArgoCD Application (P1 and post-P1) MUST report syncStatus=Synced.
        An OutOfSync or Unknown status indicates a GitOps regression.

        Known-acceptable-degradation KAD-ARGO-1 does NOT affect sync state:
        tower-keycloak is Synced (Degraded health is a separate dimension).
        """
        if not tower_reachable:
            pytest.xfail("KAD-ARGO-2: tower-cp-0 unreachable — cannot verify sync state")

        not_synced = [
            f"{app.name}(sync={app.sync_status})"
            for app in parsed_apps
            if app.sync_status != "Synced"
        ]

        assert not not_synced, (
            f"SYNC REGRESSION: the following apps are NOT Synced:\n"
            f"  {not_synced}\n"
            f"P1 baseline requires all apps to be Synced.  "
            f"Full app list:\n"
            + "\n".join(
                f"  {a.name}: sync={a.sync_status} health={a.health_status}"
                for a in sorted(parsed_apps, key=lambda x: x.name)
            )
        )

    def test_p1_apps_synced_individually(
        self,
        parsed_apps: List[AppStatus],
        tower_reachable: bool,
    ):
        """
        SCOPE: Per-app sync state for all 23 P1-baseline apps.
        Parametrized-style individual assertion: each P1 app must be Synced.
        Provides granular failure messages per app.
        """
        if not tower_reachable:
            pytest.xfail("KAD-ARGO-2: tower-cp-0 unreachable")

        app_by_name: Dict[str, AppStatus] = {a.name: a for a in parsed_apps}
        failures = []

        for app_name in sorted(P1_EXPECTED_APPS):
            if app_name not in app_by_name:
                failures.append(f"  {app_name}: MISSING")
                continue
            app = app_by_name[app_name]
            if app.sync_status != "Synced":
                failures.append(
                    f"  {app_name}: sync={app.sync_status} (expected Synced)"
                )

        assert not failures, (
            f"P1 REGRESSION — {len(failures)} app(s) not Synced:\n"
            + "\n".join(failures)
        )

    def test_non_keycloak_apps_healthy(
        self,
        parsed_apps: List[AppStatus],
        tower_reachable: bool,
    ):
        """
        SCOPE: Health state for all apps EXCEPT known-degraded ones.
        All apps not in KNOWN_DEGRADED_HEALTH_APPS must be Healthy.
        tower-keycloak Degraded is acceptable (KAD-ARGO-1 / DEG-002).
        """
        if not tower_reachable:
            pytest.xfail("KAD-ARGO-2: tower-cp-0 unreachable")

        unexpected_degraded = [
            f"{app.name}(health={app.health_status})"
            for app in parsed_apps
            if app.health_status != "Healthy"
            and app.name not in KNOWN_DEGRADED_HEALTH_APPS
        ]

        assert not unexpected_degraded, (
            f"HEALTH REGRESSION: apps with unexpected Degraded/Unknown health:\n"
            f"  {unexpected_degraded}\n"
            f"Known-acceptable degraded apps: {sorted(KNOWN_DEGRADED_HEALTH_APPS)}"
        )

    def test_keycloak_degraded_is_known_acceptable(
        self,
        parsed_apps: List[AppStatus],
        tower_reachable: bool,
    ):
        """
        SCOPE: Known-acceptable degradation verification.
        If tower-keycloak is Degraded, it must be in KNOWN_DEGRADED_HEALTH_APPS —
        confirming we acknowledge it explicitly rather than silently ignoring it.
        Also verifies tower-keycloak remains Synced (health != sync).
        """
        if not tower_reachable:
            pytest.xfail("KAD-ARGO-2: tower-cp-0 unreachable")

        app_by_name: Dict[str, AppStatus] = {a.name: a for a in parsed_apps}
        keycloak = app_by_name.get("tower-keycloak")

        if keycloak is None:
            pytest.skip("tower-keycloak not present in current app list")

        # Health: Degraded is the expected state (KAD-ARGO-1)
        assert keycloak.health_status in ("Degraded", "Healthy"), (
            f"tower-keycloak health_status={keycloak.health_status!r} is unexpected; "
            f"known-acceptable is 'Degraded' or 'Healthy' (KAD-ARGO-1)"
        )

        # Sync: Must always be Synced regardless of health
        assert keycloak.sync_status == "Synced", (
            f"tower-keycloak sync_status={keycloak.sync_status!r} — "
            f"REGRESSION: must remain Synced even when health is Degraded (KAD-ARGO-1)"
        )

        # Confirm it's in the acknowledged list
        assert "tower-keycloak" in KNOWN_DEGRADED_HEALTH_APPS, (
            "tower-keycloak must be in KNOWN_DEGRADED_HEALTH_APPS to be suppressed; "
            "this test guards against silently missing the acknowledgement"
        )

    def test_total_app_count_at_least_p1_baseline(
        self,
        parsed_apps: List[AppStatus],
        tower_reachable: bool,
    ):
        """
        SCOPE: App inventory completeness.
        Total app count must be >= len(P1_EXPECTED_APPS) (23).
        A lower count indicates missing apps (regression); a higher count is allowed
        (post-P1 additions like *-scalex-dash-rbac are expected and benign).
        """
        if not tower_reachable:
            pytest.xfail("KAD-ARGO-2: tower-cp-0 unreachable")

        assert len(parsed_apps) >= len(P1_EXPECTED_APPS), (
            f"App count {len(parsed_apps)} < P1 baseline {len(P1_EXPECTED_APPS)}.  "
            f"Missing apps:\n"
            f"  {sorted(P1_EXPECTED_APPS - {a.name for a in parsed_apps})}"
        )


# ═══════════════════════════════════════════════════════════════════════════
# TC-ARGO-5: Task model integration — argocd_sync_healthy task execution
#   Scope: full loop: Task → run_fn → Evidence stored → freshness verified
# ═══════════════════════════════════════════════════════════════════════════

class TestArgocdTaskModelIntegration:
    def test_argocd_task_executes_and_produces_fresh_evidence(
        self, run_start_time: float, tower_reachable: bool
    ):
        """
        SCOPE: task model integration, live kubectl query on tower.

        Full re-verification loop:
          1. Build an argocd_sync_healthy Task with live run_fn
          2. Execute via TaskExecutor (non-dry-run)
          3. Assert status=SUCCEEDED
          4. Assert evidence.captured_at >= run_start_time
          5. Assert evidence.is_fresh()
          6. Assert evidence stored under ARGOCD_RECHECK_EVIDENCE_KEY

        This is the canonical Sub-AC 5b evidence: argocd sync state
        re-verification executed through the task model with timestamp enforcement.
        """
        if not tower_reachable:
            pytest.xfail(
                "KAD-ARGO-2: tower-cp-0 unreachable — skipping integration test"
            )

        task = Task(
            name="argocd_sync_healthy",
            scope=(
                "gitops: all ArgoCD Applications in argocd namespace on tower cluster "
                "— re-verified via SSH+kubectl within current run [Sub-AC 5b]"
            ),
            prerequisites=[],
            evidence_deps=[],
            run_fn=_capture_argocd_apps,
            produces_evidence_key=ARGOCD_RECHECK_EVIDENCE_KEY,
            description=(
                "Re-verify all ArgoCD Applications are Synced+Healthy via SSH to "
                "tower-cp-0 (Sub-AC 5b).  Captures fresh kubectl output with "
                "embedded timestamp.  Evidence dep: gitops_bootstrap:spread_applied."
            ),
        )

        executor = TaskExecutor([task], dry_run=False)
        results = executor.run()

        result = results["argocd_sync_healthy"]
        assert result.status == TaskStatus.SUCCEEDED, (
            f"argocd_sync_healthy must SUCCEED, got {result.status.name}. "
            f"error={result.error}"
        )

        ev = result.evidence
        assert ev is not None, "TaskResult must include Evidence on SUCCEEDED"

        # Timestamp within current run window
        assert ev.captured_at >= run_start_time, (
            f"Evidence.captured_at ({ev.captured_at:.3f}) must be >= "
            f"run_start_time ({run_start_time:.3f})"
        )
        assert ev.is_fresh(), (
            f"Evidence must be fresh (age <= {MAX_EVIDENCE_AGE_S}s); "
            f"age={time.time() - ev.captured_at:.1f}s"
        )

        # Evidence stored under canonical key
        stored = executor._evidence_store.get(ARGOCD_RECHECK_EVIDENCE_KEY)
        assert stored is not None, (
            f"Evidence must be stored under key {ARGOCD_RECHECK_EVIDENCE_KEY!r}"
        )
        assert stored.captured_at >= run_start_time, (
            "Stored evidence must also have captured_at within current run window"
        )

    def test_argocd_task_dry_run_skipped_not_failed(
        self, tower_reachable: bool
    ):
        """
        SCOPE: task model dry-run mode.
        In dry-run mode, argocd_sync_healthy must be SKIPPED (not FAILED),
        and no SSH call must be made (run_fn not executed).
        """
        call_count = [0]

        def probe_counting() -> Evidence:
            call_count[0] += 1
            return _capture_argocd_apps()

        task = Task(
            name="argocd_sync_healthy",
            scope="gitops: dry-run scope declaration",
            prerequisites=[],
            evidence_deps=[],
            run_fn=probe_counting,
            produces_evidence_key=ARGOCD_RECHECK_EVIDENCE_KEY,
        )

        executor = TaskExecutor([task], dry_run=True)
        results = executor.run()

        assert results["argocd_sync_healthy"].status == TaskStatus.SKIPPED, (
            "Dry-run must SKIP argocd_sync_healthy, not FAIL or RUN it"
        )
        assert call_count[0] == 0, (
            f"Dry-run must NOT call run_fn; call_count={call_count[0]}"
        )

    def test_stale_argocd_evidence_triggers_recheck(
        self, run_start_time: float, tower_reachable: bool
    ):
        """
        SCOPE: evidential dep enforcement for argocd_sync_healthy.
        When argocd_sync_healthy evidence is STALE (>600s), a downstream task
        that declares an EvidentialDep on it must trigger RECHECK_TRIGGERED.

        This tests the periodic re-verification loop: stale evidence → re-run
        source task → fresh evidence captured (Sub-AC 5b requirement).
        """
        if not tower_reachable:
            pytest.xfail(
                "KAD-ARGO-2: tower-cp-0 unreachable — cannot test live recheck"
            )

        recheck_count = [0]

        def fresh_argocd_probe() -> Evidence:
            recheck_count[0] += 1
            return _capture_argocd_apps()

        source_task = Task(
            name="argocd_sync_healthy",
            scope="gitops: source task for evidential recheck test",
            prerequisites=[],
            evidence_deps=[],
            run_fn=fresh_argocd_probe,
            produces_evidence_key=ARGOCD_RECHECK_EVIDENCE_KEY,
        )
        consumer = Task(
            name="cf_tunnel_healthy",
            scope="cf-tunnel: consumer of argocd sync evidence",
            prerequisites=["argocd_sync_healthy"],
            evidence_deps=[
                EvidentialDep(
                    evidence_key=ARGOCD_RECHECK_EVIDENCE_KEY,
                    source_task_name="argocd_sync_healthy",
                    max_age_s=MAX_EVIDENCE_AGE_S,
                ),
            ],
            run_fn=lambda: Evidence(
                captured_at=time.time(),
                raw_output="consumer ran after recheck",
                summary="consumer ok",
            ),
        )

        executor = TaskExecutor([source_task, consumer], dry_run=False)
        # Seed STALE evidence (20 minutes old) to trigger RECHECK
        executor.seed_evidence(
            ARGOCD_RECHECK_EVIDENCE_KEY,
            raw_output="# stale argocd evidence",
            summary="argocd_apps_captured count=23 ts=STALE",
            age_seconds=1200,  # 20 minutes — exceeds MAX_EVIDENCE_AGE_S
        )

        executor.run()

        # Source task must have been re-executed to refresh stale evidence
        assert recheck_count[0] >= 1, (
            f"Stale evidence must trigger source task re-execution; "
            f"recheck_count={recheck_count[0]}.  "
            "Sub-AC 5b requires periodic re-verification (not just point-in-time)."
        )

        # After recheck, evidence must be fresh
        refreshed = executor._evidence_store.get(ARGOCD_RECHECK_EVIDENCE_KEY)
        assert refreshed is not None, "Evidence store must have refreshed evidence"
        assert refreshed.captured_at >= run_start_time, (
            f"Refreshed evidence.captured_at ({refreshed.captured_at:.3f}) must be "
            f">= run_start_time ({run_start_time:.3f})"
        )
        assert refreshed.is_fresh(), "Refreshed evidence must be fresh after recheck"


# ═══════════════════════════════════════════════════════════════════════════
# TC-ARGO-6: Post-operation network safety confirmation
#   Scope: verify SSH still works AFTER all ArgoCD queries
# ═══════════════════════════════════════════════════════════════════════════

class TestArgocdPostOperationNetworkSafety:
    def test_ssh_still_reachable_after_argocd_query(self, tower_reachable: bool):
        """
        SCOPE: live SSH probe to tower-cp-0 (read-only post-operation check).
        Network safety: verify SSH connectivity AFTER all ArgoCD query tests.
        Per feedback_network_safety_critical.md: every remote operation must
        be bracketed by connectivity verification.
        Known-acceptable-degradation KAD-ARGO-2 applies if node is unreachable.
        """
        if not tower_reachable:
            pytest.xfail(
                "KAD-ARGO-2: tower-cp-0 was already unreachable before tests — "
                "post-check skipped (pre-check already failed)"
            )

        post_check = _is_tower_reachable()
        assert post_check, (
            "tower-cp-0 SSH became UNREACHABLE after ArgoCD query operations.  "
            "This is a network safety violation — operations must not break "
            "connectivity (feedback_network_safety_critical.md)."
        )
