"""
tests/test_cause_kind.py — Sub-AC 6a verification

Scope boundary (declared before evaluation):
  - Unit tests only — no remote calls, no SSH, no VMs, no Kubernetes.
  - Tests verify the CauseKind enum taxonomy and RootCause schema defined in
    ops/task_model.py.
  - Coverage:
      * All five CauseKind values exist and have the expected string literals.
      * RootCause dataclass accepts valid CauseKind members.
      * RootCause rejects non-CauseKind values with TypeError.
      * RootCause rejects empty description with ValueError.
      * Optional fields (ticket, mitigation) default to None.
      * Taxonomy is documented: enum docstring is non-empty.

Known-acceptable-degradation inventory:
  (none — all tests in this suite must pass cleanly)

Evidence freshness: tests run in < 1 second; no evidence TTL concerns.
"""

from __future__ import annotations

import pytest

from ops.task_model import CauseKind, RootCause


# ===========================================================================
# 1. CauseKind enum taxonomy
# ===========================================================================

class TestCauseKindTaxonomy:
    """Verify the enum defines at least two distinct, correctly-valued members."""

    def test_at_least_two_distinct_values(self):
        """CauseKind MUST have at least two distinct members (Sub-AC 6a contract)."""
        members = list(CauseKind)
        assert len(members) >= 2, (
            f"CauseKind must define at least 2 members; found {len(members)}"
        )

    def test_architectural_assumption_member_exists(self):
        assert CauseKind.ARCHITECTURAL_ASSUMPTION == "architectural_assumption"

    def test_code_defect_member_exists(self):
        assert CauseKind.CODE_DEFECT == "code_defect"

    def test_external_dependency_member_exists(self):
        assert CauseKind.EXTERNAL_DEPENDENCY == "external_dependency"

    def test_configuration_error_member_exists(self):
        assert CauseKind.CONFIGURATION_ERROR == "configuration_error"

    def test_known_limitation_member_exists(self):
        assert CauseKind.KNOWN_LIMITATION == "known_limitation"

    def test_all_five_members_present(self):
        expected = {
            "architectural_assumption",
            "code_defect",
            "external_dependency",
            "configuration_error",
            "known_limitation",
        }
        actual = {c.value for c in CauseKind}
        assert actual == expected, (
            f"CauseKind values mismatch.\n  expected: {sorted(expected)}\n  got:      {sorted(actual)}"
        )

    def test_members_are_distinct(self):
        values = [c.value for c in CauseKind]
        assert len(values) == len(set(values)), "CauseKind has duplicate values"

    def test_cause_kind_is_str_enum(self):
        """CauseKind inherits from str so values can be compared to plain strings."""
        assert CauseKind.CODE_DEFECT == "code_defect"
        assert isinstance(CauseKind.CODE_DEFECT, str)

    def test_enum_docstring_documents_classifications(self):
        """The CauseKind docstring must be non-empty and mention 'classification'."""
        doc = CauseKind.__doc__
        assert doc, "CauseKind must have a non-empty docstring"
        assert "classification" in doc.lower() or "valid" in doc.lower(), (
            "CauseKind docstring must document valid classifications"
        )

    def test_lookup_by_value(self):
        """All values must be round-trippable via CauseKind(value)."""
        for member in CauseKind:
            assert CauseKind(member.value) is member


# ===========================================================================
# 2. RootCause schema
# ===========================================================================

class TestRootCauseSchema:
    """Verify the RootCause dataclass accepts and validates correctly."""

    def test_minimal_root_cause_construction(self):
        rc = RootCause(
            cause_kind=CauseKind.ARCHITECTURAL_ASSUMPTION,
            description="Single-node etcd by design in POD-mini",
        )
        assert rc.cause_kind == CauseKind.ARCHITECTURAL_ASSUMPTION
        assert "etcd" in rc.description
        assert rc.ticket is None
        assert rc.mitigation is None

    def test_full_root_cause_construction(self):
        rc = RootCause(
            cause_kind=CauseKind.CODE_DEFECT,
            description="scalex sdi init exits 0 on partial failure",
            ticket="OPS-17",
            mitigation="Manual verification step added to sdi_verify_vms task",
        )
        assert rc.cause_kind == CauseKind.CODE_DEFECT
        assert rc.ticket == "OPS-17"
        assert rc.mitigation is not None

    def test_all_cause_kinds_accepted(self):
        for ck in CauseKind:
            rc = RootCause(cause_kind=ck, description=f"Test for {ck.value}")
            assert rc.cause_kind is ck

    def test_non_enum_cause_kind_raises_type_error(self):
        """Passing a plain string as cause_kind must raise TypeError."""
        with pytest.raises(TypeError, match="CauseKind"):
            RootCause(cause_kind="code_defect", description="some description")  # type: ignore[arg-type]

    def test_integer_cause_kind_raises_type_error(self):
        with pytest.raises(TypeError, match="CauseKind"):
            RootCause(cause_kind=42, description="some description")  # type: ignore[arg-type]

    def test_none_cause_kind_raises_type_error(self):
        with pytest.raises(TypeError, match="CauseKind"):
            RootCause(cause_kind=None, description="some description")  # type: ignore[arg-type]

    def test_empty_description_raises_value_error(self):
        with pytest.raises(ValueError, match="description"):
            RootCause(cause_kind=CauseKind.KNOWN_LIMITATION, description="")

    def test_whitespace_only_description_raises_value_error(self):
        with pytest.raises(ValueError, match="description"):
            RootCause(cause_kind=CauseKind.KNOWN_LIMITATION, description="   ")

    def test_ticket_optional_defaults_none(self):
        rc = RootCause(
            cause_kind=CauseKind.EXTERNAL_DEPENDENCY,
            description="Cloudflare API quota exceeded",
        )
        assert rc.ticket is None

    def test_mitigation_optional_defaults_none(self):
        rc = RootCause(
            cause_kind=CauseKind.CONFIGURATION_ERROR,
            description="KUBECONFIG not set",
        )
        assert rc.mitigation is None

    def test_cause_kind_value_accessible_as_string(self):
        rc = RootCause(
            cause_kind=CauseKind.ARCHITECTURAL_ASSUMPTION,
            description="No HA for CF Tunnel in POD-mini",
        )
        # CauseKind is a str-enum; .value is the canonical string
        assert rc.cause_kind.value == "architectural_assumption"


# ===========================================================================
# 3. Classification completeness (regression guard)
# ===========================================================================

class TestClassificationCompleteness:
    """
    Guard against silent removal of required classifications.

    These tests encode the contract: the five classifications defined in
    Sub-AC 6a must always exist.  If a classification is ever removed or
    renamed, this suite will fail before the change is merged.
    """

    REQUIRED_VALUES = frozenset({
        "architectural_assumption",
        "code_defect",
        "external_dependency",
        "configuration_error",
        "known_limitation",
    })

    def test_no_required_classification_removed(self):
        actual = {c.value for c in CauseKind}
        missing = self.REQUIRED_VALUES - actual
        assert not missing, (
            f"Required CauseKind classifications were removed: {sorted(missing)}"
        )

    def test_architectural_assumption_and_code_defect_distinct(self):
        """The two minimum values required by Sub-AC 6a must be distinct."""
        assert CauseKind.ARCHITECTURAL_ASSUMPTION != CauseKind.CODE_DEFECT

    def test_root_cause_architectural_assumption_usable_in_degradation_context(self):
        """
        Demonstrate end-to-end usage: a DegradationItem-like context carrying
        a RootCause with ARCHITECTURAL_ASSUMPTION.
        """
        from ops.task_model import DegradationItem

        rc = RootCause(
            cause_kind=CauseKind.ARCHITECTURAL_ASSUMPTION,
            description="No HA for CF Tunnel in POD-mini — accepted during scoping",
            ticket=None,
            mitigation="Monitor cloudflared pod restarts via alerting rule",
        )
        deg = DegradationItem(
            id="DEG-ARCH-001",
            description=rc.description,
            affects_task_ids=["cf_tunnel_healthy"],
            ticket=rc.ticket,
        )
        assert deg.id == "DEG-ARCH-001"
        assert rc.cause_kind == CauseKind.ARCHITECTURAL_ASSUMPTION
