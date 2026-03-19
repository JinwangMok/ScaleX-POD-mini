"""
tests/test_causekind_backfill.py — Sub-AC 6b verification

Scope boundary (declared before evaluation):
  - Unit tests only — no remote calls, no SSH, no VMs, no Kubernetes.
  - Tests verify that every DegradationItem in the canonical inventory
    (ops/degradation_inventory.py) carries a root_cause with a valid
    CauseKind classification.
  - Also validates that config/known_degradations.yaml entries all carry
    a cause_kind field that maps to a valid CauseKind enum value.
  - Does NOT test Kubernetes health state; purely structural/schema checks.

Known-acceptable-degradation inventory:
  (none — all tests in this suite must pass cleanly)

Evidence freshness: tests run in < 1 second; no evidence TTL concerns.

Classification decisions encoded here (see ops/degradation_inventory.py
module docstring for detailed rationale):
  DEG-001  CoreDNS ContainerNotReady          → architectural_assumption
  DEG-002  ArgoCD dex-server CrashLoopBackOff → known_limitation
  DEG-003  kube-vip NodeNotReady              → architectural_assumption
"""

from __future__ import annotations

import pathlib

import pytest
import yaml

from ops.degradation_inventory import KNOWN_DEGRADATIONS, get_inventory
from ops.task_model import CauseKind, DegradationItem, RootCause


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

VALID_CAUSE_KIND_VALUES = frozenset(ck.value for ck in CauseKind)

REPO_ROOT = pathlib.Path(__file__).parent.parent
YAML_PATH = REPO_ROOT / "config" / "known_degradations.yaml"


# ===========================================================================
# 1. Canonical Python inventory — every item has root_cause with cause_kind
# ===========================================================================

class TestInventoryCauseKindBackfill:
    """
    Sub-AC 6b core requirement: every DegradationItem in KNOWN_DEGRADATIONS
    must have a root_cause with a valid CauseKind value.
    """

    def test_inventory_is_non_empty(self):
        """Inventory must contain at least one item."""
        items = get_inventory()
        assert len(items) >= 1, "KNOWN_DEGRADATIONS must not be empty"

    def test_every_item_has_root_cause(self):
        """Every DegradationItem must have root_cause set (not None)."""
        items = get_inventory()
        missing = [item.id for item in items if item.root_cause is None]
        assert not missing, (
            f"The following DegradationItems are missing root_cause: {missing}\n"
            "Sub-AC 6b requires every item to have a root_cause with cause_kind."
        )

    def test_every_root_cause_has_valid_cause_kind(self):
        """Every root_cause.cause_kind must be a valid CauseKind enum member."""
        items = get_inventory()
        for item in items:
            assert item.root_cause is not None, f"{item.id}: root_cause is None"
            assert isinstance(item.root_cause.cause_kind, CauseKind), (
                f"{item.id}: root_cause.cause_kind is not a CauseKind instance "
                f"(got {type(item.root_cause.cause_kind).__name__!r})"
            )

    def test_every_root_cause_has_non_empty_description(self):
        """Every root_cause.description must be a non-empty string."""
        items = get_inventory()
        for item in items:
            rc = item.root_cause
            assert rc is not None, f"{item.id}: root_cause is None"
            assert rc.description.strip(), (
                f"{item.id}: root_cause.description is empty"
            )

    def test_all_items_have_non_empty_id(self):
        """Every DegradationItem must have a non-empty id."""
        items = get_inventory()
        for item in items:
            assert item.id.strip(), "DegradationItem id must not be empty"

    def test_all_items_have_non_empty_description(self):
        """Every DegradationItem must have a non-empty description."""
        items = get_inventory()
        for item in items:
            assert item.description.strip(), (
                f"{item.id}: DegradationItem description is empty"
            )

    def test_get_inventory_returns_copy(self):
        """get_inventory() returns a list; mutations do not affect canonical store."""
        inv1 = get_inventory()
        inv2 = get_inventory()
        inv1.clear()
        assert len(inv2) > 0, "get_inventory() must return independent copies"


# ===========================================================================
# 2. Classification accuracy — check expected cause_kind per known item
# ===========================================================================

class TestClassificationAccuracy:
    """
    Verify each known degradation item carries the expected cause_kind.

    These tests encode the classification decisions made in Sub-AC 6b.
    If an item's cause_kind is changed, this suite will fail as a
    regression guard.
    """

    def _by_id(self, deg_id: str) -> DegradationItem:
        for item in KNOWN_DEGRADATIONS:
            if item.id == deg_id:
                return item
        pytest.fail(f"DegradationItem {deg_id!r} not found in KNOWN_DEGRADATIONS")

    def test_deg_001_coredns_is_architectural_assumption(self):
        """
        DEG-001 (CoreDNS ContainerNotReady) must be architectural_assumption.

        Rationale: bare-metal bootstrap sequence races CNI readiness with the
        CoreDNS readiness probe; this is structural, not a defect.
        """
        item = self._by_id("DEG-001")
        assert item.root_cause is not None
        assert item.root_cause.cause_kind == CauseKind.ARCHITECTURAL_ASSUMPTION, (
            f"DEG-001 expected architectural_assumption, "
            f"got {item.root_cause.cause_kind.value!r}"
        )

    def test_deg_002_dex_server_is_known_limitation(self):
        """
        DEG-002 (ArgoCD dex-server CrashLoopBackOff) must be known_limitation.

        Rationale: OIDC integration is intentionally deferred to post-P2;
        this is a deliberate capability boundary, not an architecture choice
        or a defect — hence known_limitation, not architectural_assumption.
        """
        item = self._by_id("DEG-002")
        assert item.root_cause is not None
        assert item.root_cause.cause_kind == CauseKind.KNOWN_LIMITATION, (
            f"DEG-002 expected known_limitation, "
            f"got {item.root_cause.cause_kind.value!r}"
        )

    def test_deg_003_kubevip_is_architectural_assumption(self):
        """
        DEG-003 (kube-vip NodeNotReady) must be architectural_assumption.

        Rationale: ARP propagation delay is structural to the bare-metal VIP
        design in POD-mini; no code change or configuration will eliminate it.
        """
        item = self._by_id("DEG-003")
        assert item.root_cause is not None
        assert item.root_cause.cause_kind == CauseKind.ARCHITECTURAL_ASSUMPTION, (
            f"DEG-003 expected architectural_assumption, "
            f"got {item.root_cause.cause_kind.value!r}"
        )


# ===========================================================================
# 3. YAML config file — cause_kind values match CauseKind enum
# ===========================================================================

class TestYamlCauseKindConsistency:
    """
    Verify that config/known_degradations.yaml entries all carry a cause_kind
    field whose value is a valid CauseKind enum value.

    This ensures the YAML (human-readable mirror) stays in sync with the enum.
    """

    def _load_yaml(self) -> list[dict]:
        assert YAML_PATH.exists(), f"YAML not found: {YAML_PATH}"
        with open(YAML_PATH) as fh:
            data = yaml.safe_load(fh)
        return data.get("known_degradations", [])

    def test_yaml_entries_non_empty(self):
        entries = self._load_yaml()
        assert len(entries) >= 1, "known_degradations.yaml must have at least one entry"

    def test_yaml_every_entry_has_cause_kind(self):
        """Every YAML entry must have a cause_kind field."""
        entries = self._load_yaml()
        missing = [
            e.get("id", e.get("name", "<unknown>"))
            for e in entries
            if "cause_kind" not in e
        ]
        assert not missing, (
            f"YAML entries missing cause_kind field: {missing}\n"
            "Sub-AC 6b requires every entry to have cause_kind."
        )

    def test_yaml_cause_kind_values_are_valid_enum_members(self):
        """Every YAML cause_kind value must match a CauseKind enum string value."""
        entries = self._load_yaml()
        invalid = []
        for entry in entries:
            ck = entry.get("cause_kind", "")
            if ck not in VALID_CAUSE_KIND_VALUES:
                entry_id = entry.get("id", entry.get("name", "<unknown>"))
                invalid.append(f"{entry_id}: {ck!r}")
        assert not invalid, (
            f"YAML entries with invalid cause_kind values:\n"
            + "\n".join(f"  {i}" for i in invalid)
            + f"\nValid values: {sorted(VALID_CAUSE_KIND_VALUES)}"
        )

    def test_yaml_cause_kind_no_hyphens(self):
        """
        YAML cause_kind values must use underscores, not hyphens.

        CauseKind enum values use underscore notation (e.g. 'architectural_assumption').
        Hyphens ('architectural-assumption') are NOT valid.
        """
        entries = self._load_yaml()
        hyphenated = []
        for entry in entries:
            ck = entry.get("cause_kind", "")
            if "-" in ck:
                entry_id = entry.get("id", entry.get("name", "<unknown>"))
                hyphenated.append(f"{entry_id}: {ck!r}")
        assert not hyphenated, (
            f"YAML entries with hyphenated cause_kind (must use underscores):\n"
            + "\n".join(f"  {h}" for h in hyphenated)
        )

    def test_yaml_entry_ids_match_python_inventory(self):
        """
        YAML entries with explicit id fields should have matching items in
        the Python canonical inventory.
        """
        yaml_ids = {e["id"] for e in self._load_yaml() if "id" in e}
        python_ids = {item.id for item in KNOWN_DEGRADATIONS}
        # All YAML ids must appear in the Python inventory
        missing_from_python = yaml_ids - python_ids
        assert not missing_from_python, (
            f"YAML ids not found in Python inventory: {sorted(missing_from_python)}"
        )

    def test_yaml_deg002_cause_kind_is_known_limitation(self):
        """DEG-002 in YAML must be known_limitation (regression guard)."""
        entries = self._load_yaml()
        deg002 = next((e for e in entries if e.get("id") == "DEG-002"), None)
        assert deg002 is not None, "DEG-002 not found in YAML"
        assert deg002.get("cause_kind") == "known_limitation", (
            f"DEG-002 YAML cause_kind should be 'known_limitation', "
            f"got {deg002.get('cause_kind')!r}"
        )


# ===========================================================================
# 4. DegradationItem model — root_cause field accepted correctly
# ===========================================================================

class TestDegradationItemRootCauseField:
    """Verify DegradationItem now accepts root_cause and stores it correctly."""

    def test_degradation_item_accepts_root_cause(self):
        rc = RootCause(
            cause_kind=CauseKind.KNOWN_LIMITATION,
            description="Metrics server out of scope for POD-mini",
        )
        item = DegradationItem(
            id="DEG-TEST-001",
            description="Metrics server not deployed",
            affects_task_ids=["AC-5"],
            ticket="N/A",
            root_cause=rc,
        )
        assert item.root_cause is rc
        assert item.root_cause.cause_kind == CauseKind.KNOWN_LIMITATION

    def test_degradation_item_root_cause_defaults_to_none(self):
        """Existing code that creates DegradationItem without root_cause still works."""
        item = DegradationItem(
            id="DEG-LEGACY-001",
            description="Legacy item without root_cause",
            affects_task_ids=["AC-1"],
        )
        assert item.root_cause is None

    def test_degradation_item_all_cause_kinds_accepted(self):
        """Every CauseKind value can be stored in a DegradationItem.root_cause."""
        for ck in CauseKind:
            rc = RootCause(cause_kind=ck, description=f"Test for {ck.value}")
            item = DegradationItem(
                id=f"DEG-KIND-{ck.value}",
                description=f"test item for {ck.value}",
                affects_task_ids=["AC-test"],
                root_cause=rc,
            )
            assert item.root_cause.cause_kind is ck
