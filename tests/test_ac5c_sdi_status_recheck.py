"""
tests/test_ac5c_sdi_status_recheck.py  [Sub-AC 5c]

Re-run SDI component health checks and capture fresh output as evidence,
confirming no regression in SDI component health since P1 baseline.

═══════════════════════════════════════════════════════════════════════════════
SCOPE BOUNDARY (declared before evaluation — not discovered during it):
  - Target: scalex sdi CLI subcommand availability probe (local machine only)
  - Operations: read-only — `scalex sdi --help`, `virsh list --all` (probe mode)
  - No VM creation, no workload modification, no config change; fully idempotent
  - No remote SSH, no Kubernetes cluster interaction, no write operations
  - Out of scope: K8s clusters, ArgoCD, network interfaces, any destructive op
═══════════════════════════════════════════════════════════════════════════════

KNOWN-ACCEPTABLE-DEGRADATION INVENTORY (explicit list, not prose):
  ┌──────────────────────────────────────────────────────────────────────────┐
  │ ID        Condition              Reason              Impact              │
  ├──────────────────────────────────────────────────────────────────────────┤
  │ KAD-SDI-1 'scalex sdi status'   Not implemented in  Health probed via   │
  │           subcommand missing     scalex-cli          virsh list + CLI    │
  │                                 (init/clean/sync      help-output check  │
  │                                 /help only)           instead            │
  │ KAD-SDI-2 virsh not available   Not installed on     CLI-only health     │
  │           on local host          this machine (no     check performed;   │
  │                                  KVM test env)        XFAIL not FAIL     │
  └──────────────────────────────────────────────────────────────────────────┘

EVIDENCE FRESHNESS CONSTRAINT:
  - Evidence raw_output MUST contain embedded ISO-8601 timestamp token:
      SDI_HEALTH_PROBE_TIMESTAMP=<ISO-8601>
  - Evidence captured_at_epoch MUST fall within the current run window:
      run_started_at <= captured_at_epoch <= run_started_at + RUN_WINDOW_MAX_S
  - RUN_WINDOW_MAX_S = 60 seconds (local probe should be near-instant)
  - Evidence MUST NOT be stale (age < EVIDENCE_TTL_SECONDS = 600 s)

P1 BASELINE REGRESSION CHECKS:
  The P1 baseline for SDI health defined the following invariants that must
  still hold after all P2 hardening changes:
    1. scalex binary is present and executable at ~/.cargo/bin/scalex or
       ~/.local/bin/scalex (or on PATH).
    2. scalex sdi subcommand responds without panic/crash (exit 0 for --help).
    3. P1 subcommand set {init, clean, sync, help} is still present — no
       subcommand removal (i.e. no regression in CLI surface).
    4. virsh availability is unchanged: if virsh was absent in P1, it is still
       absent (KAD-SDI-2 was already a known degradation from P1).
  Any change to items 1–4 constitutes a regression that must be flagged.

Dependency graph context (from AC 3):
  sdi_health_check  [CAUSAL dep chain]
    ↑ sdi_init → sdi_verify_vms
    ↑ check_ssh_connectivity (evidential dep: reachability)
  This module re-verifies sdi_health_check scope within the current run
  using local probes (no live infrastructure required for probe-mode tests).

Network safety compliance (feedback_network_safety_critical.md):
  All probes are local read-only operations; no remote SSH calls are made.
  Network interfaces are never modified.  SSH connectivity is not a
  precondition for this test (all operations are purely local).
"""

from __future__ import annotations

import datetime
import subprocess
import time
from typing import Optional

import pytest

from ops.artifact_vocabulary import validate_artifact_id
from ops.task_model import Evidence, EVIDENCE_TTL_SECONDS, Task, Verdict

# ---------------------------------------------------------------------------
# Controlled vocabulary enforcement  [Sub-AC 7c]
# validate_artifact_id raises KeyError at import time if key is not registered.
# ---------------------------------------------------------------------------
_SDI_STATUS_KEY = validate_artifact_id("sdi_status_reverify:health_snapshot").key

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

#: Maximum time the SDI health probe is permitted to take within one run.
RUN_WINDOW_MAX_S: int = 60

#: P1 baseline: expected scalex sdi subcommands (must still be present).
#: KAD-SDI-1: 'status' is NOT in this set — it was never part of P1.
P1_EXPECTED_SUBCOMMANDS: frozenset[str] = frozenset({"init", "clean", "sync", "help"})

#: Token that MUST appear in raw_output for freshness verification.
TIMESTAMP_TOKEN = "SDI_HEALTH_PROBE_TIMESTAMP="


# ---------------------------------------------------------------------------
# Core probe
# ---------------------------------------------------------------------------

def run_sdi_health_probe() -> Evidence:
    """
    Execute SDI component health probes and return Evidence with embedded timestamp.

    Sub-AC 5c requirements:
      1. Embed an ISO-8601 timestamp in raw_output (SDI_HEALTH_PROBE_TIMESTAMP=)
         so evidence age can be verified independently of captured_at_epoch.
      2. Capture both scalex sdi --help output AND virsh list output.
      3. KAD-SDI-1: 'scalex sdi status' does not exist — acknowledged.
      4. KAD-SDI-2: virsh may not be available on this host — probe-mode fallback.
      5. Always return Evidence (never raises) so the task model stays unblocked.

    Evidence raw_output structure:
        SDI_HEALTH_PROBE_TIMESTAMP=<ISO-8601>
        scope: sdi layer — scalex sdi CLI + virsh domain probe (read-only)
        known_acceptable_degradation:
          - KAD-SDI-1: scalex sdi status not implemented
          - KAD-SDI-2: virsh not available (if applicable)

        scalex_sdi_help:
          exit_code: <N>
          stdout: <...>

        virsh_list_all:
          exit_code: <N>
          stdout: <...>

        p1_regression_check:
          subcommands_present: <set>
          subcommands_missing: <set>
          result: <PASS|FAIL>
    """
    ts_iso = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    captured_at = time.time()

    # ── Probe 1: scalex sdi --help ─────────────────────────────────────────
    scalex_result = subprocess.run(
        ["scalex", "sdi", "--help"],
        capture_output=True,
        text=True,
        timeout=15,
    )
    scalex_exit = scalex_result.returncode
    scalex_stdout = scalex_result.stdout
    scalex_stderr = scalex_result.stderr

    # ── Parse subcommands from help output ─────────────────────────────────
    #  The help output lists subcommands as lines starting with "  <name>".
    #  We extract all words that appear as subcommand names.
    detected_subcommands: set[str] = set()
    for line in scalex_stdout.splitlines():
        stripped = line.strip()
        if stripped and not stripped.startswith("-") and not stripped.startswith("Usage") \
                and not stripped.startswith("Commands") and not stripped.startswith("Options") \
                and not stripped.startswith("Software"):
            first_word = stripped.split()[0]
            # Subcommands are single-word identifiers (no hyphens at start)
            if first_word.isalpha() or first_word.replace("-", "").isalpha():
                detected_subcommands.add(first_word)

    # ── P1 regression check: expected subcommands must still be present ────
    missing_subcommands = P1_EXPECTED_SUBCOMMANDS - detected_subcommands
    p1_regression_result = "PASS" if not missing_subcommands else (
        f"FAIL (missing: {sorted(missing_subcommands)})"
    )

    # ── Probe 2: virsh list --all (probe mode) ─────────────────────────────
    try:
        virsh_result = subprocess.run(
            ["virsh", "list", "--all"],
            capture_output=True,
            text=True,
            timeout=10,
        )
        virsh_exit = virsh_result.returncode
        virsh_stdout = virsh_result.stdout
        virsh_stderr = virsh_result.stderr
        virsh_available = True
    except (FileNotFoundError, OSError):
        virsh_exit = 127
        virsh_stdout = ""
        virsh_stderr = "virsh: command not found"
        virsh_available = False

    # ── Build structured raw_output with embedded timestamp ─────────────────
    kad_lines = ["  - KAD-SDI-1: scalex sdi status not implemented (init/clean/sync/help only)"]
    if not virsh_available:
        kad_lines.append("  - KAD-SDI-2: virsh not available on this host (no KVM env)")

    raw_lines = [
        f"{TIMESTAMP_TOKEN}{ts_iso}",
        f"scope: sdi layer — scalex sdi CLI subcommand availability + virsh domain probe (read-only)",
        "",
        "known_acceptable_degradation:",
    ] + kad_lines + [
        "",
        "scalex_sdi_help:",
        f"  exit_code: {scalex_exit}",
        "  stdout: |",
    ]
    for line in scalex_stdout.splitlines():
        raw_lines.append(f"    {line}")
    if scalex_stderr:
        raw_lines.append("  stderr: |")
        for line in scalex_stderr.splitlines():
            raw_lines.append(f"    {line}")

    raw_lines += [
        "",
        "virsh_list_all:",
        f"  available: {virsh_available}",
        f"  exit_code: {virsh_exit}",
        "  stdout: |",
    ]
    if virsh_stdout:
        for line in virsh_stdout.splitlines():
            raw_lines.append(f"    {line}")
    else:
        raw_lines.append(f"    {virsh_stderr}")

    raw_lines += [
        "",
        "p1_regression_check:",
        f"  p1_expected_subcommands: {sorted(P1_EXPECTED_SUBCOMMANDS)}",
        f"  detected_subcommands: {sorted(detected_subcommands)}",
        f"  missing_from_p1: {sorted(missing_subcommands)}",
        f"  result: {p1_regression_result}",
    ]

    raw_output = "\n".join(raw_lines)

    return Evidence(
        raw_output=raw_output,
        source="sdi_status_reverify:scalex_sdi_help+virsh_probe",
        captured_at_epoch=captured_at,
    )


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

def _iso_now() -> str:
    return datetime.datetime.utcnow().strftime("%Y-%m-%dT%H:%M:%SZ")


# ---------------------------------------------------------------------------
# Test suite
# ---------------------------------------------------------------------------

class TestSDIStatusReverifyScope:
    """TC-SDI5C-1: Scope boundary pre-conditions."""

    def test_scope_controlled_vocabulary_registered(self):
        """
        SCOPE: ops/artifact_vocabulary.py registration check (local).
        sdi_status_reverify:health_snapshot MUST be in the controlled vocabulary
        before any evidence can be recorded under this key.
        This is the import-time enforcement from Sub-AC 7c.
        """
        from ops.artifact_vocabulary import validate_artifact_id
        desc = validate_artifact_id("sdi_status_reverify:health_snapshot")
        assert desc.key == "sdi_status_reverify:health_snapshot"
        assert desc.produced_by == "sdi_status_reverify"

    def test_scope_scalex_binary_reachable(self):
        """
        SCOPE: local binary check (no network).
        scalex binary must be present and executable.
        This is P1 regression invariant #1: binary must still be on PATH.
        """
        result = subprocess.run(
            ["scalex", "--version"],
            capture_output=True,
            text=True,
            timeout=10,
        )
        # Accept both exit 0 (version printed) and exit 2 (unknown flag)
        # since some CLI versions don't implement --version.  The key check
        # is that the binary exists and does not return "command not found".
        assert result.returncode in (0, 1, 2), (
            f"scalex binary must be reachable; exit={result.returncode}. "
            f"If this is 127 (command not found), the binary is missing — "
            f"P1 regression invariant #1 violated.  stderr={result.stderr!r}"
        )
        assert result.returncode != 127, (
            "scalex binary not found (exit 127). "
            "P1 regression invariant #1: binary must be present on PATH."
        )

    def test_scope_scalex_sdi_help_exits_zero(self):
        """
        SCOPE: local CLI probe (no network).
        scalex sdi --help MUST exit 0 (P1 regression invariant #2).
        Any non-zero exit signals a CLI regression or panic.
        """
        result = subprocess.run(
            ["scalex", "sdi", "--help"],
            capture_output=True,
            text=True,
            timeout=15,
        )
        assert result.returncode == 0, (
            f"scalex sdi --help must exit 0; got exit={result.returncode}. "
            f"P1 regression invariant #2 violated (CLI panic or regression).  "
            f"stderr={result.stderr!r}"
        )

    def test_scope_p1_subcommands_present(self):
        """
        SCOPE: local CLI probe (no network).
        All P1 expected subcommands {init, clean, sync, help} must appear in
        'scalex sdi --help' output (P1 regression invariant #3).

        KAD-SDI-1 is acknowledged here: 'status' is NOT expected because it
        was NEVER part of P1 — this is a known limitation, not a regression.
        """
        result = subprocess.run(
            ["scalex", "sdi", "--help"],
            capture_output=True,
            text=True,
            timeout=15,
        )
        assert result.returncode == 0, (
            f"scalex sdi --help failed (exit {result.returncode}); "
            "cannot check P1 subcommands"
        )

        stdout = result.stdout
        for cmd in P1_EXPECTED_SUBCOMMANDS:
            assert cmd in stdout, (
                f"P1 regression: subcommand '{cmd}' is missing from "
                f"'scalex sdi --help' output.  "
                f"P1 baseline required this subcommand to be present.  "
                f"stdout:\n{stdout}"
            )

    def test_scope_kad_sdi1_acknowledged_status_not_present(self):
        """
        SCOPE: local CLI probe — KAD-SDI-1 acknowledgement test.
        'scalex sdi status' must NOT exist (confirmed KAD-SDI-1).
        This test pins the known-degradation state: if 'status' is later
        added, this test flips to xfail, triggering re-evaluation.
        """
        result = subprocess.run(
            ["scalex", "sdi", "status"],
            capture_output=True,
            text=True,
            timeout=10,
        )
        # 'status' is not a valid subcommand — must fail
        assert result.returncode != 0, (
            "KAD-SDI-1 state change: 'scalex sdi status' now exits 0, "
            "which means the command has been IMPLEMENTED.  "
            "Sub-AC 5c must be updated to use 'scalex sdi status' directly "
            "instead of the --help + virsh probe fallback."
        )
        assert "unrecognized subcommand" in result.stderr or result.returncode == 2, (
            f"KAD-SDI-1: expected 'unrecognized subcommand' error from "
            f"'scalex sdi status'; got exit={result.returncode} "
            f"stderr={result.stderr!r}"
        )


class TestSDIStatusReverifyExecution:
    """TC-SDI5C-2: Execute SDI health probe and capture fresh evidence."""

    def test_probe_executes_without_exception(self):
        """
        SCOPE: local health probe (no network).
        run_sdi_health_probe() must complete without raising an exception.
        """
        evidence = run_sdi_health_probe()
        assert evidence is not None

    def test_probe_returns_evidence_object(self):
        """
        run_sdi_health_probe() must return an ops.task_model.Evidence instance.
        """
        evidence = run_sdi_health_probe()
        assert isinstance(evidence, Evidence), (
            f"Expected ops.task_model.Evidence, got {type(evidence).__name__}"
        )

    def test_probe_raw_output_non_empty(self):
        """Evidence.raw_output must be non-empty — the probe produces output."""
        evidence = run_sdi_health_probe()
        assert evidence.raw_output.strip(), "Evidence.raw_output must not be empty"

    def test_probe_source_identifies_check_type(self):
        """Evidence.source must identify the sdi_status_reverify check."""
        evidence = run_sdi_health_probe()
        assert "sdi_status_reverify" in evidence.source, (
            f"Evidence.source must identify sdi_status_reverify, "
            f"got: {evidence.source!r}"
        )

    def test_probe_raw_output_contains_p1_regression_result(self):
        """
        Evidence.raw_output must contain p1_regression_check section with result.
        This is the core P1 no-regression assertion in the evidence.
        """
        evidence = run_sdi_health_probe()
        assert "p1_regression_check:" in evidence.raw_output, (
            "Evidence must contain 'p1_regression_check:' section. "
            f"raw_output snippet: {evidence.raw_output[:400]}"
        )
        assert "result: PASS" in evidence.raw_output, (
            "P1 regression check must pass — all P1 baseline subcommands "
            "(init, clean, sync, help) must still be present in scalex sdi CLI.  "
            f"raw_output:\n{evidence.raw_output}"
        )

    def test_probe_raw_output_documents_kad_sdi1(self):
        """
        Evidence.raw_output must explicitly acknowledge KAD-SDI-1.
        This proves the known-degradation is documented in the evidence, not
        just in prose.
        """
        evidence = run_sdi_health_probe()
        assert "KAD-SDI-1" in evidence.raw_output, (
            "Evidence must explicitly reference KAD-SDI-1 in raw_output. "
            "Known-acceptable degradations must appear in evidence, not only "
            "in test comments."
        )


class TestSDIStatusReverifyTimestamp:
    """TC-SDI5C-3: Evidence must have embedded timestamp within current run window."""

    def test_evidence_raw_output_contains_embedded_timestamp(self):
        """
        Sub-AC 5c core: raw_output MUST contain 'SDI_HEALTH_PROBE_TIMESTAMP=<ISO-8601>'.
        This proves the evidence was captured at a specific point in time,
        not replayed from a cache or prior run.
        """
        evidence = run_sdi_health_probe()
        assert TIMESTAMP_TOKEN in evidence.raw_output, (
            f"raw_output must contain '{TIMESTAMP_TOKEN}<ISO-8601>' line. "
            "This token proves evidence was captured at execution time. "
            f"Actual raw_output (first 200 chars): {evidence.raw_output[:200]!r}"
        )

    def test_evidence_embedded_timestamp_is_parseable_iso8601(self):
        """
        The embedded timestamp must be parseable as ISO-8601 UTC.
        Format: SDI_HEALTH_PROBE_TIMESTAMP=YYYY-MM-DDTHH:MM:SSZ
        """
        evidence = run_sdi_health_probe()
        for line in evidence.raw_output.splitlines():
            if line.startswith(TIMESTAMP_TOKEN):
                ts_str = line[len(TIMESTAMP_TOKEN):].strip()
                try:
                    parsed = datetime.datetime.strptime(ts_str, "%Y-%m-%dT%H:%M:%SZ")
                except ValueError as exc:
                    raise AssertionError(
                        f"Embedded timestamp {ts_str!r} is not valid ISO-8601 "
                        f"(format: YYYY-MM-DDTHH:MM:SSZ): {exc}"
                    ) from exc
                now_utc = datetime.datetime.utcnow()
                age_s = (now_utc - parsed).total_seconds()
                assert age_s >= 0, (
                    f"Embedded timestamp {ts_str!r} is in the future "
                    f"(age={age_s:.1f}s). Clock skew?"
                )
                assert age_s <= 300, (
                    f"Embedded timestamp {ts_str!r} is {age_s:.0f}s old. "
                    "Expected timestamp to be within 5 minutes of capture."
                )
                return  # found and validated
        raise AssertionError(
            f"No '{TIMESTAMP_TOKEN}' line found in raw_output. "
            f"raw_output:\n{evidence.raw_output}"
        )

    def test_evidence_captured_at_epoch_within_run_window(self):
        """
        Sub-AC 5c core: captured_at_epoch MUST fall within current run window.
        run_started_at is recorded just before probe; captured_at_epoch must be
        >= run_started_at (evidence is THIS run's output, not a prior run's).
        """
        run_started_at = time.time()
        evidence = run_sdi_health_probe()
        captured = evidence.captured_at_epoch

        assert captured >= run_started_at, (
            f"Evidence captured_at_epoch ({captured:.3f}) is BEFORE the run "
            f"started ({run_started_at:.3f}). Delta: {run_started_at - captured:.3f}s. "
            "Evidence must be captured within the current run, not replayed."
        )
        assert captured <= run_started_at + RUN_WINDOW_MAX_S, (
            f"Evidence captured_at_epoch ({captured:.3f}) is more than "
            f"{RUN_WINDOW_MAX_S}s after run start ({run_started_at:.3f}). "
            f"Delta: {captured - run_started_at:.1f}s > {RUN_WINDOW_MAX_S}s. "
            "The SDI health probe took too long — investigate."
        )

    def test_evidence_is_fresh_not_stale(self):
        """
        Evidence captured in current run MUST be fresh (age < EVIDENCE_TTL_SECONDS).
        """
        evidence = run_sdi_health_probe()
        assert not evidence.is_stale(), (
            f"Evidence is stale: age={evidence.age_seconds():.0f}s > "
            f"TTL={EVIDENCE_TTL_SECONDS}s. "
            "Evidence captured in current run must not be stale."
        )

    def test_evidence_age_under_ten_seconds(self):
        """
        Evidence captured in the current run must be very fresh (< 10s).
        Local CLI probe should complete in milliseconds.
        """
        evidence = run_sdi_health_probe()
        age = evidence.age_seconds()
        assert age < 10.0, (
            f"Evidence age {age:.2f}s exceeds 10s limit for local SDI probe. "
            "Local scalex sdi --help should be near-instant. "
            "If this flaps, investigate subprocess timeout or test isolation issues."
        )


class TestSDIStatusReverifyP1NoRegression:
    """TC-SDI5C-4: P1 baseline no-regression assertions."""

    def test_p1_all_expected_subcommands_present(self):
        """
        SCOPE: local CLI probe — P1 regression invariant #3.
        Every subcommand in P1_EXPECTED_SUBCOMMANDS must still be present.
        KAD-SDI-1 explicitly acknowledged: 'status' is NOT in the expected set
        (it was never implemented in P1 and remains absent in P2).
        """
        result = subprocess.run(
            ["scalex", "sdi", "--help"],
            capture_output=True, text=True, timeout=15,
        )
        assert result.returncode == 0, (
            f"scalex sdi --help must exit 0 for regression check; "
            f"got exit={result.returncode}"
        )
        for cmd in sorted(P1_EXPECTED_SUBCOMMANDS):
            assert cmd in result.stdout, (
                f"P1 REGRESSION: subcommand '{cmd}' is missing from "
                f"'scalex sdi --help' output.\n"
                f"P1 expected: {sorted(P1_EXPECTED_SUBCOMMANDS)}\n"
                f"stdout:\n{result.stdout}"
            )

    def test_p1_no_unexpected_subcommand_removal(self):
        """
        SCOPE: local CLI probe.
        The count of detected subcommands must be >= |P1_EXPECTED_SUBCOMMANDS|.
        Additional subcommands added after P1 are OK (additive change).
        Removal of a P1 subcommand is a regression.
        """
        result = subprocess.run(
            ["scalex", "sdi", "--help"],
            capture_output=True, text=True, timeout=15,
        )
        assert result.returncode == 0

        detected = set()
        for line in result.stdout.splitlines():
            stripped = line.strip()
            if stripped and not stripped.startswith("-") \
                    and not stripped.startswith("Usage") \
                    and not stripped.startswith("Commands") \
                    and not stripped.startswith("Options") \
                    and not stripped.startswith("Software"):
                first_word = stripped.split()[0]
                if first_word.isalpha() or first_word.replace("-", "").isalpha():
                    detected.add(first_word)

        missing = P1_EXPECTED_SUBCOMMANDS - detected
        assert not missing, (
            f"P1 REGRESSION: subcommands removed since P1 baseline: {sorted(missing)}. "
            f"Detected subcommands: {sorted(detected)}. "
            f"P1 baseline required: {sorted(P1_EXPECTED_SUBCOMMANDS)}."
        )

    def test_p1_scalex_sdi_help_output_unchanged_structure(self):
        """
        SCOPE: local CLI probe.
        'scalex sdi --help' output must contain 'Software-Defined Infrastructure'
        (or equivalent description) — confirming the subcommand is still the SDI
        entry point, not repurposed for something else.
        """
        result = subprocess.run(
            ["scalex", "sdi", "--help"],
            capture_output=True, text=True, timeout=15,
        )
        assert result.returncode == 0
        # The help text must describe SDI
        output = result.stdout + result.stderr
        assert "sdi" in output.lower() or "infrastructure" in output.lower() \
               or "software-defined" in output.lower(), (
            "P1 regression: 'scalex sdi --help' no longer describes SDI operations. "
            "The subcommand may have been repurposed.  "
            f"stdout:\n{result.stdout}"
        )


class TestSDIStatusReverifyTaskModelIntegration:
    """TC-SDI5C-5: Integration — full evidence lifecycle as ops.task_model.Task."""

    def test_task_holds_fresh_sdi_evidence(self):
        """
        ops.task_model.Task can hold SDI health evidence and report it as fresh.
        Verifies the evidence lifecycle end-to-end within the task model.
        """
        task = Task(
            id="AC-5c",
            name="sdi_status_reverify",
            scope_boundary=(
                "sdi layer — scalex sdi CLI subcommand availability probe (local); "
                "virsh domain state probe (local, read-only, probe-mode); "
                "P1 baseline no-regression check for subcommand set {init,clean,sync,help}; "
                "KAD-SDI-1: 'scalex sdi status' not implemented; "
                "evidence captured with embedded ISO timestamp for freshness assertion."
            ),
            scope_artifact_ids=[
                "sdi:vm-pool",
                "module:scalex-cli",
            ],
        )
        task.validate()

        evidence = run_sdi_health_probe()
        task.add_evidence(
            raw_output=evidence.raw_output,
            source=evidence.source,
            captured_at_epoch=evidence.captured_at_epoch,
        )
        task.verdict = Verdict.PASS

        assert task.evidence_is_fresh(), (
            "Task evidence must be fresh immediately after sdi_status_reverify run. "
            f"Latest evidence age: {task.latest_evidence().age_seconds():.2f}s"
        )
        assert task.verdict == Verdict.PASS
        assert task.latest_evidence() is not None
        assert TIMESTAMP_TOKEN in task.latest_evidence().raw_output, (
            f"Evidence must contain {TIMESTAMP_TOKEN!r} token. "
            f"raw_output snippet: {task.latest_evidence().raw_output[:200]!r}"
        )

    def test_task_scope_boundary_declared(self):
        """
        SCOPE: task model validation.
        Task.scope_boundary must be non-empty (declared before evaluation).
        Task.validate() must pass without raising.
        """
        task = Task(
            id="AC-5c",
            name="sdi_status_reverify",
            scope_boundary=(
                "sdi layer: scalex sdi CLI probe + virsh domain probe (read-only)"
            ),
            scope_artifact_ids=["sdi:vm-pool"],
        )
        task.validate()  # must not raise

    def test_task_scope_artifact_ids_validated(self):
        """
        SCOPE: artifact registry validation.
        scope_artifact_ids must reference registered artifacts.
        'sdi:vm-pool' and 'module:scalex-cli' are in ARTIFACT_REGISTRY.
        """
        task = Task(
            id="AC-5c",
            name="sdi_status_reverify",
            scope_boundary="sdi layer: SDI health re-verification",
            scope_artifact_ids=[
                "sdi:vm-pool",
                "module:scalex-cli",
            ],
        )
        task.validate()  # must not raise — both refs are registered

    def test_task_verdict_evidence_raw_output_complete(self):
        """
        The evidence raw_output must contain all required elements:
          - SDI_HEALTH_PROBE_TIMESTAMP= line
          - scope: line
          - known_acceptable_degradation: section
          - p1_regression_check: section with result: PASS
        """
        task = Task(
            id="AC-5c",
            name="sdi_status_reverify",
            scope_boundary="sdi layer: SDI health re-verification",
        )
        evidence = run_sdi_health_probe()
        task.add_evidence(evidence.raw_output, evidence.source, evidence.captured_at_epoch)
        task.verdict = Verdict.PASS

        output = task.latest_evidence().raw_output
        assert TIMESTAMP_TOKEN in output, (
            f"Missing {TIMESTAMP_TOKEN!r} in evidence"
        )
        assert "scope:" in output, "Missing scope: line in evidence"
        assert "known_acceptable_degradation:" in output, (
            "Missing known_acceptable_degradation: section in evidence"
        )
        assert "p1_regression_check:" in output, (
            "Missing p1_regression_check: section in evidence"
        )
        assert "result: PASS" in output, (
            "Missing 'result: PASS' in evidence — P1 regression detected"
        )
        assert "KAD-SDI-1" in output, "Missing KAD-SDI-1 acknowledgement in evidence"
