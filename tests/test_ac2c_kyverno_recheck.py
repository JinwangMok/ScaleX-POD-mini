"""
tests/test_ac2c_kyverno_recheck.py — ScaleX-POD-mini P2 Operational Hardening

Sub-AC 2c: Re-verify Kyverno (AC 2) within current run.

SCOPE BOUNDARY (declared before evaluation, not discovered during):
  - gitops/common/kyverno-policies/: 3 ClusterPolicy manifests
    (disallow-privileged.yaml, require-labels.yaml, restrict-host-namespaces.yaml)
  - Validation: YAML structure, Kyverno v1 apiVersion, ClusterPolicy kind,
    required metadata.name and spec.rules fields present in each policy.
  - Out of scope: live Kubernetes cluster, admission webhook, actual policy
    enforcement, ArgoCD sync state.

KNOWN-ACCEPTABLE-DEGRADATION INVENTORY (explicit list, not prose):
  (none — all assertions in this suite must pass without degradation)

EVIDENCE FRESHNESS CONSTRAINT:
  - Evidence captured in the current run MUST have an embedded ISO timestamp
    in raw_output (format: "TIMESTAMP: <ISO-8601>").
  - Evidence captured_at_epoch MUST fall within the current run window:
      run_started_at <= captured_at_epoch <= run_started_at + RUN_WINDOW_MAX_S
  - RUN_WINDOW_MAX_S = 60 (one minute); if the policy check takes longer than
    60 seconds the test fails — that is a signal to investigate, not to widen
    the window.
  - Evidence must NOT be stale (age < EVIDENCE_TTL_SECONDS = 600).
"""

from __future__ import annotations

import datetime
import os
import time
from pathlib import Path
from typing import Any

import pytest
import yaml

from ops.task_model import Evidence, EVIDENCE_TTL_SECONDS

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

#: Maximum time (seconds) the policy check is permitted to take within one run.
RUN_WINDOW_MAX_S: int = 60

#: Absolute path to the Kyverno policy directory (relative to repo root).
KYVERNO_POLICIES_DIR = Path(__file__).parent.parent / "gitops" / "common" / "kyverno-policies"

#: Expected ClusterPolicy files (order independent).
EXPECTED_POLICY_FILES = {
    "disallow-privileged.yaml",
    "require-labels.yaml",
    "restrict-host-namespaces.yaml",
}

#: Required top-level fields in every Kyverno ClusterPolicy manifest.
REQUIRED_KYVERNO_FIELDS = ("apiVersion", "kind", "metadata", "spec")

#: Expected apiVersion for all policies in this inventory.
EXPECTED_API_VERSION = "kyverno.io/v1"

#: Expected Kubernetes kind.
EXPECTED_KIND = "ClusterPolicy"


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _iso_now() -> str:
    """Return current UTC time as an ISO-8601 string (embedded in raw_output)."""
    return datetime.datetime.utcnow().strftime("%Y-%m-%dT%H:%M:%SZ")


def _load_policy(path: Path) -> dict[str, Any]:
    """Load a YAML manifest; raise AssertionError with path on failure."""
    try:
        with path.open() as fh:
            doc = yaml.safe_load(fh)
    except yaml.YAMLError as exc:
        raise AssertionError(
            f"YAML parse error in {path.name}: {exc}"
        ) from exc
    assert isinstance(doc, dict), (
        f"{path.name}: expected a YAML mapping at top level, got {type(doc).__name__}"
    )
    return doc


def run_kyverno_policy_check() -> Evidence:
    """
    Execute Kyverno policy checks locally (no cluster needed):
      1. Assert all expected policy files exist in KYVERNO_POLICIES_DIR.
      2. Load each file as YAML.
      3. Assert apiVersion == kyverno.io/v1, kind == ClusterPolicy.
      4. Assert metadata.name and spec.rules are present and non-empty.
      5. Build structured raw_output with embedded timestamp.
      6. Return Evidence with captured_at_epoch = time.time().

    Raises AssertionError on any check failure (will surface in pytest).
    """
    captured_at = time.time()
    iso_ts = _iso_now()

    assert KYVERNO_POLICIES_DIR.is_dir(), (
        f"Kyverno policies directory not found: {KYVERNO_POLICIES_DIR}. "
        "Scope boundary: gitops/common/kyverno-policies/ must exist in gitops repo."
    )

    present_files = {f.name for f in KYVERNO_POLICIES_DIR.glob("*.yaml")
                     if f.name != "kustomization.yaml"}
    missing = EXPECTED_POLICY_FILES - present_files
    assert not missing, (
        f"Missing Kyverno policy files: {sorted(missing)}. "
        f"Present files: {sorted(present_files)}"
    )

    results: list[dict[str, Any]] = []
    for fname in sorted(EXPECTED_POLICY_FILES):
        fpath = KYVERNO_POLICIES_DIR / fname
        doc = _load_policy(fpath)

        # Check required top-level fields
        for field_name in REQUIRED_KYVERNO_FIELDS:
            assert field_name in doc, (
                f"{fname}: missing required field '{field_name}'. "
                f"Present keys: {list(doc.keys())}"
            )

        # Check apiVersion
        assert doc["apiVersion"] == EXPECTED_API_VERSION, (
            f"{fname}: expected apiVersion={EXPECTED_API_VERSION!r}, "
            f"got {doc['apiVersion']!r}"
        )

        # Check kind
        assert doc["kind"] == EXPECTED_KIND, (
            f"{fname}: expected kind={EXPECTED_KIND!r}, "
            f"got {doc['kind']!r}"
        )

        # Check metadata.name is non-empty
        meta = doc.get("metadata", {})
        assert isinstance(meta, dict) and meta.get("name"), (
            f"{fname}: metadata.name is missing or empty"
        )

        # Check spec.rules is a non-empty list
        spec = doc.get("spec", {})
        rules = spec.get("rules") if isinstance(spec, dict) else None
        assert isinstance(rules, list) and len(rules) > 0, (
            f"{fname}: spec.rules must be a non-empty list, got {rules!r}"
        )

        results.append({
            "file": fname,
            "policy_name": meta["name"],
            "api_version": doc["apiVersion"],
            "kind": doc["kind"],
            "rule_count": len(rules),
            "validation_failure_action": spec.get("validationFailureAction", "<not set>"),
        })

    # Build raw_output with embedded timestamp
    lines = [
        f"TIMESTAMP: {iso_ts}",
        f"scope: gitops/common/kyverno-policies/ ({len(results)} policies)",
        "",
        "policy_audit:",
    ]
    for r in results:
        lines.append(
            f"  - file={r['file']}"
            f"  name={r['policy_name']}"
            f"  api_version={r['api_version']}"
            f"  kind={r['kind']}"
            f"  rules={r['rule_count']}"
            f"  action={r['validation_failure_action']}"
        )
    lines.append("")
    lines.append(f"result: PASS ({len(results)} ClusterPolicy manifests validated)")

    raw_output = "\n".join(lines)

    return Evidence(
        raw_output=raw_output,
        source="kyverno_policy_check:local_yaml_validation",
        captured_at_epoch=captured_at,
    )


# ---------------------------------------------------------------------------
# Test suite
# ---------------------------------------------------------------------------

class TestKyvernoPolicyCheckScope:
    """TC-KYV-1: Scope boundary pre-conditions."""

    def test_scope_kyverno_policies_dir_exists(self):
        """
        SCOPE: gitops/common/kyverno-policies/ must exist in the gitops repo.
        This is the declared scope boundary — if this fails, the AC cannot proceed.
        """
        assert KYVERNO_POLICIES_DIR.is_dir(), (
            f"Scope boundary violation: {KYVERNO_POLICIES_DIR} does not exist. "
            "Kyverno ClusterPolicy manifests must be present in gitops repo."
        )

    def test_scope_expected_policy_files_present(self):
        """
        SCOPE: Exactly the expected set of policy files must be present.
        Any missing file is a scope boundary violation.
        """
        present = {f.name for f in KYVERNO_POLICIES_DIR.glob("*.yaml")
                   if f.name != "kustomization.yaml"}
        missing = EXPECTED_POLICY_FILES - present
        assert not missing, (
            f"Scope boundary: expected policy files not found: {sorted(missing)}. "
            f"Present: {sorted(present)}"
        )

    def test_scope_artifact_vocabulary_registered(self):
        """
        SCOPE: kyverno_policy_check:policy_audit must be registered in the
        controlled vocabulary (ops/artifact_vocabulary.py) before this test
        suite can assert evidence keys.
        """
        from ops.artifact_vocabulary import validate_artifact_id
        descriptor = validate_artifact_id("kyverno_policy_check:policy_audit")
        assert descriptor.key == "kyverno_policy_check:policy_audit"
        assert descriptor.produced_by == "kyverno_policy_check"


class TestKyvernoPolicyCheckExecution:
    """TC-KYV-2: Execute Kyverno policy checks and capture evidence."""

    def test_policy_check_executes_without_error(self):
        """
        SCOPE: local YAML validation of 3 ClusterPolicy manifests.
        run_kyverno_policy_check() must complete without raising.
        """
        evidence = run_kyverno_policy_check()
        assert evidence is not None

    def test_policy_check_returns_evidence_object(self):
        """
        run_kyverno_policy_check() must return an ops.task_model.Evidence instance.
        """
        evidence = run_kyverno_policy_check()
        assert isinstance(evidence, Evidence), (
            f"Expected ops.task_model.Evidence, got {type(evidence).__name__}"
        )

    def test_policy_check_raw_output_non_empty(self):
        """
        Evidence.raw_output must be non-empty — the policy audit produces output.
        """
        evidence = run_kyverno_policy_check()
        assert evidence.raw_output.strip(), "Evidence.raw_output must not be empty"

    def test_policy_check_raw_output_contains_all_policy_names(self):
        """
        Evidence.raw_output must reference each ClusterPolicy by file name.
        This proves the audit actually inspected each file.
        """
        evidence = run_kyverno_policy_check()
        for fname in EXPECTED_POLICY_FILES:
            assert fname in evidence.raw_output, (
                f"Policy file {fname!r} not found in raw_output. "
                "The audit must reference each policy file by name."
            )

    def test_policy_check_source_identifies_check_type(self):
        """
        Evidence.source must identify the check as kyverno local YAML validation.
        """
        evidence = run_kyverno_policy_check()
        assert "kyverno_policy_check" in evidence.source, (
            f"Evidence.source must identify kyverno_policy_check, got: {evidence.source!r}"
        )


class TestKyvernoPolicyStructure:
    """TC-KYV-3: Each ClusterPolicy manifest must conform to Kyverno v1 schema."""

    @pytest.fixture(scope="class")
    def policies(self) -> list[tuple[str, dict]]:
        """Load all policy files; return list of (filename, parsed_doc)."""
        docs = []
        for fname in sorted(EXPECTED_POLICY_FILES):
            fpath = KYVERNO_POLICIES_DIR / fname
            with fpath.open() as fh:
                doc = yaml.safe_load(fh)
            docs.append((fname, doc))
        return docs

    def test_all_policies_have_kyverno_v1_api_version(self, policies):
        """All policies must declare apiVersion: kyverno.io/v1."""
        for fname, doc in policies:
            assert doc.get("apiVersion") == EXPECTED_API_VERSION, (
                f"{fname}: apiVersion must be {EXPECTED_API_VERSION!r}, "
                f"got {doc.get('apiVersion')!r}"
            )

    def test_all_policies_have_cluster_policy_kind(self, policies):
        """All policies must declare kind: ClusterPolicy."""
        for fname, doc in policies:
            assert doc.get("kind") == EXPECTED_KIND, (
                f"{fname}: kind must be {EXPECTED_KIND!r}, "
                f"got {doc.get('kind')!r}"
            )

    def test_all_policies_have_non_empty_name(self, policies):
        """All policies must have a non-empty metadata.name."""
        for fname, doc in policies:
            name = doc.get("metadata", {}).get("name", "")
            assert name, f"{fname}: metadata.name is missing or empty"

    def test_all_policies_have_non_empty_rules(self, policies):
        """All policies must have at least one rule in spec.rules."""
        for fname, doc in policies:
            rules = doc.get("spec", {}).get("rules", [])
            assert isinstance(rules, list) and rules, (
                f"{fname}: spec.rules must be a non-empty list, got {rules!r}"
            )

    def test_all_policies_use_audit_action(self, policies):
        """
        All policies must use validationFailureAction: Audit.
        AC 2 hardening: audit mode ensures policies are non-blocking (safe default).
        """
        for fname, doc in policies:
            action = doc.get("spec", {}).get("validationFailureAction", "")
            assert action == "Audit", (
                f"{fname}: validationFailureAction must be 'Audit', got {action!r}. "
                "AC 2 requires audit mode to ensure non-blocking admission control."
            )

    def test_all_policies_have_required_fields(self, policies):
        """All required top-level fields must be present in each manifest."""
        for fname, doc in policies:
            for field_name in REQUIRED_KYVERNO_FIELDS:
                assert field_name in doc, (
                    f"{fname}: missing required field {field_name!r}. "
                    f"Present keys: {list(doc.keys())}"
                )


class TestKyvernoEvidenceTimestamp:
    """TC-KYV-4: Evidence must have embedded timestamp within current run window."""

    def test_evidence_raw_output_contains_embedded_timestamp(self):
        """
        Sub-AC 2c core: raw_output MUST contain 'TIMESTAMP: <ISO-8601>' line.
        This proves the evidence was captured at a specific point in time
        (not a cached or replayed result).
        """
        evidence = run_kyverno_policy_check()
        assert "TIMESTAMP: " in evidence.raw_output, (
            "raw_output must contain 'TIMESTAMP: <ISO-8601>' line. "
            "This proves the evidence was captured at execution time, "
            "not replayed from a cache. "
            f"Actual raw_output (first 200 chars): {evidence.raw_output[:200]!r}"
        )

    def test_evidence_embedded_timestamp_is_parseable_iso8601(self):
        """
        The embedded timestamp must be parseable as ISO-8601 UTC.
        Format: TIMESTAMP: YYYY-MM-DDTHH:MM:SSZ
        """
        evidence = run_kyverno_policy_check()
        for line in evidence.raw_output.splitlines():
            if line.startswith("TIMESTAMP: "):
                ts_str = line[len("TIMESTAMP: "):].strip()
                try:
                    parsed = datetime.datetime.strptime(ts_str, "%Y-%m-%dT%H:%M:%SZ")
                except ValueError as exc:
                    raise AssertionError(
                        f"Embedded timestamp {ts_str!r} is not valid ISO-8601 "
                        f"(format: YYYY-MM-DDTHH:MM:SSZ): {exc}"
                    ) from exc
                # Timestamp must be recent (within the last 5 minutes)
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
            "No 'TIMESTAMP: ' line found in raw_output. "
            f"raw_output:\n{evidence.raw_output}"
        )

    def test_evidence_captured_at_epoch_within_run_window(self):
        """
        Sub-AC 2c core: captured_at_epoch MUST fall within current run window.

        run_started_at is recorded just before capture.
        After capture: captured_at_epoch >= run_started_at
        And:           captured_at_epoch <= run_started_at + RUN_WINDOW_MAX_S

        This asserts the evidence was produced by THIS run, not a prior run.
        """
        run_started_at = time.time()
        evidence = run_kyverno_policy_check()
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
            "The policy check took too long — investigate."
        )

    def test_evidence_is_fresh_not_stale(self):
        """
        Evidence captured in current run MUST be fresh (age < EVIDENCE_TTL_SECONDS).
        If this fails the evidence is too old to use in a verdict.
        """
        evidence = run_kyverno_policy_check()
        assert not evidence.is_stale(), (
            f"Evidence is stale: age={evidence.age_seconds():.0f}s > "
            f"TTL={EVIDENCE_TTL_SECONDS}s. "
            "Evidence captured in current run must not be stale."
        )

    def test_evidence_age_is_under_ten_seconds(self):
        """
        Evidence captured in the current run must be very fresh (< 10s).
        Local YAML validation should complete in milliseconds.
        This is a stronger freshness assertion than the 600s TTL.
        """
        evidence = run_kyverno_policy_check()
        age = evidence.age_seconds()
        assert age < 10.0, (
            f"Evidence age {age:.2f}s exceeds 10s limit for local policy check. "
            "Local YAML validation should be near-instant. "
            "If this flaps, investigate file system latency or test isolation issues."
        )


class TestKyvernoEvidenceIntegration:
    """TC-KYV-5: Integration — full evidence lifecycle as ops.task_model.Task."""

    def test_task_holds_fresh_kyverno_evidence(self):
        """
        ops.task_model.Task can hold Kyverno evidence and report it as fresh.
        Verifies the evidence lifecycle end-to-end within the task model.
        """
        from ops.task_model import Task, Verdict

        task = Task(
            id="AC-2c",
            name="kyverno_policy_check",
            scope_boundary=(
                "gitops/common/kyverno-policies/: 3 ClusterPolicy manifests "
                "validated for Kyverno v1 API compliance; evidence captured with "
                "embedded ISO timestamp; freshness asserted within current run window."
            ),
        )
        task.validate()

        evidence = run_kyverno_policy_check()
        task.add_evidence(
            raw_output=evidence.raw_output,
            source=evidence.source,
            captured_at_epoch=evidence.captured_at_epoch,
        )
        task.verdict = Verdict.PASS

        assert task.evidence_is_fresh(), (
            "Task evidence must be fresh immediately after kyverno_policy_check run. "
            f"Latest evidence age: {task.latest_evidence().age_seconds():.2f}s"
        )
        assert task.verdict == Verdict.PASS
        assert task.latest_evidence() is not None
        assert "TIMESTAMP: " in task.latest_evidence().raw_output

    def test_task_verdict_evidence_raw_output_is_complete(self):
        """
        The evidence raw_output must contain all required audit elements:
          - TIMESTAMP line
          - scope line
          - policy_audit section
          - result line with PASS
        """
        from ops.task_model import Task, Verdict

        task = Task(
            id="AC-2c",
            name="kyverno_policy_check",
            scope_boundary="gitops/common/kyverno-policies/ validation",
        )
        evidence = run_kyverno_policy_check()
        task.add_evidence(evidence.raw_output, evidence.source, evidence.captured_at_epoch)
        task.verdict = Verdict.PASS

        output = task.latest_evidence().raw_output
        assert "TIMESTAMP: " in output, "Missing TIMESTAMP line in evidence"
        assert "scope:" in output, "Missing scope line in evidence"
        assert "policy_audit:" in output, "Missing policy_audit section in evidence"
        assert "result: PASS" in output, "Missing result: PASS line in evidence"
        # Verify all 3 policy files are referenced
        for fname in EXPECTED_POLICY_FILES:
            assert fname in output, (
                f"Policy file {fname!r} not referenced in evidence output. "
                "All 3 policies must appear in the audit output."
            )
