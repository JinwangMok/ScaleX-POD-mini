"""
tests/test_ac6c_cause_kind_completeness.py — Sub-AC 6c

Scope boundary (declared before evaluation):
  - Programmatic inspection of config/known_degradations.yaml
  - Validates that every entry carries a cause_kind field
  - Validates cause_kind values belong to the CauseKind taxonomy
  - Spot-checks architectural_assumption vs code_defect classification accuracy
  - Out of scope: Kubernetes cluster, SSH, network, bare-metal VMs

Known-acceptable-degradation inventory for THIS test suite:
  (none — all tests must pass cleanly; this suite IS the verification)

Evidence freshness rule:
  - YAML is a static artefact; age constraint does not apply to file reads.
  - The test execution timestamp is captured as evidence.
"""

from __future__ import annotations

import pathlib
import time
from typing import Any

import pytest
import yaml

from ops.task_model import CauseKind


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

YAML_PATH = pathlib.Path(__file__).parent.parent / "config" / "known_degradations.yaml"

# Valid cause_kind string values (from the CauseKind enum)
VALID_CAUSE_KINDS: frozenset[str] = frozenset(c.value for c in CauseKind)


def _load_degradations() -> list[dict[str, Any]]:
    """Load and return the list of known_degradations entries from YAML."""
    with YAML_PATH.open() as f:
        data = yaml.safe_load(f)
    return data["known_degradations"]


def _entry_label(entry: dict[str, Any]) -> str:
    """Return a short label for an entry (for assertion messages)."""
    return f"{entry.get('namespace', '?')}/{entry.get('resource_kind', '?')}/{entry.get('name', '?')}"


# ---------------------------------------------------------------------------
# TC-1: Every entry has a cause_kind field (completeness check)
# ---------------------------------------------------------------------------

class TestCauseKindCompleteness:
    """
    Verify that no known_degradation entry is missing cause_kind.

    This is the primary AC 6c completeness gate.
    """

    def test_yaml_file_exists(self):
        """The known_degradations.yaml file must exist and be non-empty."""
        assert YAML_PATH.exists(), f"Known degradations file not found: {YAML_PATH}"
        assert YAML_PATH.stat().st_size > 0, "known_degradations.yaml is empty"

    def test_all_entries_have_cause_kind(self):
        """
        No entry may be missing the cause_kind field.

        This is the primary completeness assertion for Sub-AC 6c.
        Raw evidence: full list of entries with their cause_kind values
        is printed below.
        """
        entries = _load_degradations()
        assert len(entries) > 0, "known_degradations list must not be empty"

        missing = []
        for entry in entries:
            if "cause_kind" not in entry or not entry["cause_kind"]:
                missing.append(_entry_label(entry))

        print(f"\n[AC-6c evidence] Checked {len(entries)} degradation entries:")
        for e in entries:
            label = _entry_label(e)
            ck = e.get("cause_kind", "<MISSING>")
            print(f"  {label!r:60s}  cause_kind={ck!r}")

        assert missing == [], (
            f"Entries missing cause_kind ({len(missing)} found): {missing}\n"
            "Every known_degradation entry MUST carry a cause_kind field. "
            "Add the field and classify using the CauseKind taxonomy."
        )

    def test_all_cause_kinds_are_valid_enum_values(self):
        """
        Every cause_kind value must match a CauseKind enum member.

        Rejects free-text values that are not in the taxonomy.
        """
        entries = _load_degradations()
        invalid = []
        for entry in entries:
            ck = entry.get("cause_kind")
            if ck not in VALID_CAUSE_KINDS:
                invalid.append((_entry_label(entry), ck))

        print(f"\n[AC-6c evidence] Valid CauseKind values: {sorted(VALID_CAUSE_KINDS)}")
        for label, ck in invalid:
            print(f"  INVALID: {label!r}  cause_kind={ck!r}")

        assert invalid == [], (
            f"Entries with invalid cause_kind ({len(invalid)} found): {invalid}\n"
            f"Valid values are: {sorted(VALID_CAUSE_KINDS)}"
        )


# ---------------------------------------------------------------------------
# TC-2: Spot-check — architectural_assumption vs code_defect accuracy
# ---------------------------------------------------------------------------

class TestCauseKindAccuracy:
    """
    Spot-check that architectural_assumption vs code_defect distinctions
    are applied correctly.

    Definition applied:
      architectural_assumption — degradation stems from a deliberate design
        choice, environment constraint, or structural property of the
        deployment.  No code change is needed; the system is behaving as
        designed within known constraints.

      code_defect — degradation is caused by a software bug (wrong logic,
        incorrect configuration generated by code, etc.) that should be
        fixed.  The exemption is temporary.

    Current inventory has 3 entries, all classified as architectural_assumption.
    This spot-check validates that classification is accurate.
    """

    def test_coredns_is_architectural_assumption_not_code_defect(self):
        """
        coredns-* ContainerNotReady is an architectural assumption.

        Rationale: CoreDNS readiness probe behaviour during bootstrap is a
        well-known Kubernetes property — not a bug in ScaleX code.  The pod
        recovers automatically; no code change would eliminate the transient.
        Classification: architectural_assumption  ✓ (not code_defect)
        """
        entries = _load_degradations()
        entry = next(
            (e for e in entries if e.get("name", "").startswith("coredns")), None
        )
        assert entry is not None, "coredns-* entry not found in known_degradations"
        assert entry["cause_kind"] == CauseKind.ARCHITECTURAL_ASSUMPTION.value, (
            f"coredns-* should be classified as "
            f"{CauseKind.ARCHITECTURAL_ASSUMPTION.value!r} "
            f"(got {entry['cause_kind']!r}). "
            "The transient ContainerNotReady is inherent to Kubernetes bootstrap "
            "propagation, not a ScaleX code bug."
        )

    def test_argocd_dex_server_is_known_limitation_not_code_defect(self):
        """
        argocd-dex-server-* CrashLoopBackOff is a known_limitation.

        Rationale: OIDC/Keycloak integration is intentionally deferred to a
        future iteration (post-P2).  dex-server is non-functional until OIDC
        secrets are provisioned.  This is a deliberate capability boundary of
        the current release scope — not an architectural assumption (which would
        imply a structural property of the deployment) and not a code defect.

        Classification: known_limitation  ✓
        NOT code_defect  — there is no bug to fix in ScaleX code; OIDC is simply
                           not yet built.
        NOT architectural_assumption — the dex-server COULD work if OIDC were
                           wired; the absence is a scope decision, not inherent
                           to the architecture.

        Spot-check accuracy: applying architectural_assumption here would be
        incorrect because the degradation would cease if OIDC were enabled —
        architectural assumptions do not change with feature delivery.
        """
        entries = _load_degradations()
        entry = next(
            (e for e in entries if "dex" in e.get("name", "")), None
        )
        assert entry is not None, "argocd-dex-server-* entry not found in known_degradations"
        assert entry["cause_kind"] == CauseKind.KNOWN_LIMITATION.value, (
            f"argocd-dex-server-* should be {CauseKind.KNOWN_LIMITATION.value!r} "
            f"(got {entry['cause_kind']!r}). "
            "Deferred OIDC integration is a deliberate capability boundary (known_limitation), "
            "not a code defect or an inherent architectural property."
        )

    def test_kube_vip_is_architectural_assumption_not_code_defect(self):
        """
        kube-vip-* NodeNotReady is an architectural assumption.

        Rationale: ARP propagation delay after bare-metal VIP assignment is
        structural to the kube-vip approach on a flat L2 network with br0.
        The delay is inherent to the bare-metal VIP architecture; no code
        change in ScaleX can eliminate it.
        Classification: architectural_assumption  ✓ (not code_defect)
        """
        entries = _load_degradations()
        entry = next(
            (e for e in entries if "kube-vip" in e.get("name", "")), None
        )
        assert entry is not None, "kube-vip-* entry not found in known_degradations"
        assert entry["cause_kind"] == CauseKind.ARCHITECTURAL_ASSUMPTION.value, (
            f"kube-vip-* should be {CauseKind.ARCHITECTURAL_ASSUMPTION.value!r} "
            f"(got {entry['cause_kind']!r}). "
            "ARP propagation delay is structural to bare-metal VIP, not a code bug."
        )

    def test_no_code_defect_entries_without_ticket(self):
        """
        Every code_defect entry MUST have a ticket reference.

        code_defect entries are temporary exemptions pending a fix.  Without
        a ticket they become invisible permanent exemptions — which is
        unacceptable.  This test enforces the tracking requirement.

        (Currently 0 code_defect entries; this acts as a regression gate.)
        """
        entries = _load_degradations()
        offenders = [
            _entry_label(e)
            for e in entries
            if e.get("cause_kind") == CauseKind.CODE_DEFECT.value
            and (not e.get("ticket") or e.get("ticket") == "N/A")
        ]
        assert offenders == [], (
            f"code_defect entries without a valid ticket ({len(offenders)}): "
            f"{offenders}. Every code_defect degradation must reference a ticket."
        )


# ---------------------------------------------------------------------------
# TC-3: RootCause / DegradationItem model consistency
# ---------------------------------------------------------------------------

class TestModelConsistency:
    """
    Verify the ops/task_model.py DegradationItem and RootCause integration.

    These tests exercise the Python model to confirm that a DegradationItem
    constructed from YAML data carries a valid cause_kind at the model level.
    """

    def test_degradation_item_accepts_valid_root_cause(self):
        """DegradationItem can be constructed with a RootCause carrying a CauseKind."""
        from ops.task_model import DegradationItem, RootCause, CauseKind

        rc = RootCause(
            cause_kind=CauseKind.ARCHITECTURAL_ASSUMPTION,
            description="CoreDNS bootstrap transient — structural to Kubernetes startup",
        )
        d = DegradationItem(
            id="DEG-COREDNS",
            description="coredns-* ContainerNotReady during bootstrap",
            root_cause=rc,
            affects_task_ids=["AC-health-check"],
            ticket="N/A",
        )
        assert d.root_cause.cause_kind == CauseKind.ARCHITECTURAL_ASSUMPTION
        assert d.root_cause.description

    def test_root_cause_rejects_invalid_cause_kind_type(self):
        """RootCause must reject a non-CauseKind value for cause_kind."""
        from ops.task_model import RootCause

        with pytest.raises(TypeError, match="cause_kind"):
            RootCause(
                cause_kind="architectural_assumption",  # str, not CauseKind enum
                description="test",
            )

    def test_root_cause_rejects_empty_description(self):
        """RootCause must reject an empty description string."""
        from ops.task_model import RootCause, CauseKind

        with pytest.raises(ValueError, match="description"):
            RootCause(
                cause_kind=CauseKind.CODE_DEFECT,
                description="   ",  # whitespace only
            )

    def test_yaml_entries_map_to_valid_cause_kind_enum(self):
        """
        Every cause_kind string in the YAML can be resolved to a CauseKind enum member.

        This is the bridge between the static YAML inventory and the Python model:
        confirms that loading the YAML and calling CauseKind(value) would succeed
        for every entry.
        """
        entries = _load_degradations()
        for entry in entries:
            ck_str = entry.get("cause_kind")
            assert ck_str is not None, (
                f"{_entry_label(entry)}: cause_kind missing"
            )
            # Must not raise ValueError
            resolved = CauseKind(ck_str)
            assert resolved is not None, (
                f"{_entry_label(entry)}: CauseKind({ck_str!r}) could not be resolved"
            )


# ---------------------------------------------------------------------------
# Evidence capture: print full YAML inventory with cause_kind at test time
# ---------------------------------------------------------------------------

def test_print_full_inventory_evidence(capsys):
    """
    Print the complete known_degradations inventory as structured evidence.

    This output constitutes the raw evidence for the AC 6c verdict:
    'no root_cause is missing a cause_kind'.
    """
    capture_time = time.time()
    entries = _load_degradations()

    print(f"\n{'='*70}")
    print(f"  AC-6c EVIDENCE: known_degradations inventory cause_kind audit")
    print(f"  captured_at={capture_time:.0f}  entries={len(entries)}")
    print(f"{'='*70}")

    all_pass = True
    for i, entry in enumerate(entries, start=1):
        label = _entry_label(entry)
        ck = entry.get("cause_kind", "<MISSING>")
        is_valid = ck in VALID_CAUSE_KINDS
        status = "OK  " if is_valid else "FAIL"
        if not is_valid:
            all_pass = False
        print(f"  [{i}] {status}  {label}")
        print(f"       cause_kind      = {ck!r}  (valid={is_valid})")
        print(f"       condition       = {entry.get('condition', '?')!r}")
        print(f"       acknowledged_by = {entry.get('acknowledged_by', '?')!r}")
        print(f"       ticket          = {entry.get('ticket', 'N/A')!r}")
        print()

    print(f"  VERDICT: {'ALL PASS — no root_cause missing cause_kind' if all_pass else 'FAIL — see above'}")
    print(f"{'='*70}\n")

    # Assertion after evidence printed
    assert all_pass, (
        "One or more entries have invalid or missing cause_kind — see printed inventory above"
    )
