"""
tests/task_model/test_artifact_vocabulary.py  [Sub-AC 7c]

Verification pass: 100% of task records reference only registered artifact
identifiers from the controlled vocabulary, each with an explicit granularity level.

Scope boundary (declared before evaluation):
  - Unit tests only -- no remote calls, no VMs, no SSH.
  - Tests cover:
      1. Controlled vocabulary structure (ArtifactDescriptor, GranularityLevel)
      2. All registered keys have explicit, non-None granularity levels
      3. All task produces_evidence_key values are in ARTIFACT_REGISTRY
      4. All EvidentialDep.evidence_key values are in ARTIFACT_REGISTRY
      5. No unregistered keys exist in the task graph (100% compliance)
      6. Banned aspect names are rejected by ArtifactDescriptor.__post_init__
      7. Import-time validation: scalex_tasks._REGISTERED_KEYS matches task graph

Known-acceptable-degradation inventory:
  - None for this AC (pure unit tests, local execution only).
"""

from __future__ import annotations

import pytest

from ops.artifact_vocabulary import (
    ARTIFACT_REGISTRY,
    ArtifactDescriptor,
    GranularityLevel,
    get_all_registered_keys,
    validate_artifact_id,
)
from tests.task_model.scalex_tasks import _REGISTERED_KEYS, build_task_graph


# ---------------------------------------------------------------------------
# TC-AV-1: Controlled vocabulary structure
# ---------------------------------------------------------------------------

class TestArtifactVocabularyStructure:
    def test_artifact_registry_is_non_empty(self):
        """ARTIFACT_REGISTRY must contain at least one entry."""
        assert ARTIFACT_REGISTRY, "ARTIFACT_REGISTRY must not be empty"

    def test_all_registered_keys_have_granularity(self):
        """
        Every ArtifactDescriptor in the registry must carry an explicit
        GranularityLevel (ATOMIC, FINE, or COARSE).  No None values allowed.
        """
        for key, descriptor in ARTIFACT_REGISTRY.items():
            assert descriptor.granularity is not None, (
                f"ArtifactDescriptor {key!r} has no granularity level set"
            )
            assert isinstance(descriptor.granularity, GranularityLevel), (
                f"ArtifactDescriptor {key!r}: granularity must be a GranularityLevel "
                f"member, got {type(descriptor.granularity).__name__!r}"
            )

    def test_all_registered_keys_follow_naming_convention(self):
        """
        Every registered key must follow '<task_name>:<aspect>' format.
        Both parts must be non-empty.
        """
        for key in ARTIFACT_REGISTRY:
            assert ":" in key, (
                f"Registered key {key!r} does not follow '<task_name>:<aspect>' "
                "naming convention"
            )
            task_part, aspect_part = key.split(":", 1)
            assert task_part.strip(), (
                f"Registered key {key!r}: task-name part is empty"
            )
            assert aspect_part.strip(), (
                f"Registered key {key!r}: aspect part is empty"
            )

    def test_all_registered_descriptors_have_non_empty_descriptions(self):
        """Every ArtifactDescriptor must have a non-empty description."""
        for key, descriptor in ARTIFACT_REGISTRY.items():
            assert descriptor.description.strip(), (
                f"ArtifactDescriptor {key!r} has an empty description"
            )

    def test_all_registered_descriptors_have_produced_by(self):
        """Every ArtifactDescriptor must name the task that produces it."""
        for key, descriptor in ARTIFACT_REGISTRY.items():
            assert descriptor.produced_by.strip(), (
                f"ArtifactDescriptor {key!r} has an empty produced_by field"
            )

    def test_get_all_registered_keys_returns_sorted_list(self):
        """get_all_registered_keys() must return a sorted list of strings."""
        keys = get_all_registered_keys()
        assert keys == sorted(keys), (
            "get_all_registered_keys() must return keys in sorted order"
        )
        assert len(keys) == len(ARTIFACT_REGISTRY), (
            "get_all_registered_keys() count must match ARTIFACT_REGISTRY size"
        )


# ---------------------------------------------------------------------------
# TC-AV-2: validate_artifact_id helper
# ---------------------------------------------------------------------------

class TestValidateArtifactId:
    def test_registered_key_returns_descriptor(self):
        """validate_artifact_id() returns the descriptor for a registered key."""
        descriptor = validate_artifact_id("check_ssh_connectivity:reachability")
        assert descriptor.key == "check_ssh_connectivity:reachability"
        assert descriptor.granularity == GranularityLevel.ATOMIC

    def test_unregistered_key_raises_key_error(self):
        """validate_artifact_id() raises KeyError for an unregistered key."""
        with pytest.raises(KeyError, match="not in the controlled vocabulary"):
            validate_artifact_id("nonexistent:artifact")

    def test_banned_aspect_rejected_at_construction(self):
        """
        ArtifactDescriptor rejects 'completion' and other banned lifecycle verbs
        as aspect names -- they convey no artifact identity.
        """
        with pytest.raises(ValueError, match="vague lifecycle verb"):
            ArtifactDescriptor(
                key="sdi_init:completion",
                granularity=GranularityLevel.FINE,
                description="banned aspect test",
                produced_by="sdi_init",
            )

    def test_banned_aspects_include_done_and_finished(self):
        """Other banned lifecycle verbs are also rejected."""
        for banned in ("done", "finished", "ran", "executed", "ok"):
            with pytest.raises(ValueError, match="vague lifecycle verb"):
                ArtifactDescriptor(
                    key=f"some_task:{banned}",
                    granularity=GranularityLevel.ATOMIC,
                    description="test",
                    produced_by="some_task",
                )

    def test_key_without_colon_rejected(self):
        """ArtifactDescriptor rejects keys without the colon separator."""
        with pytest.raises(ValueError, match="<task_name>:<aspect>"):
            ArtifactDescriptor(
                key="no_colon_here",
                granularity=GranularityLevel.ATOMIC,
                description="test",
                produced_by="test_task",
            )


# ---------------------------------------------------------------------------
# TC-AV-3: 100% compliance -- all task records use registered identifiers
# ---------------------------------------------------------------------------

class TestTaskGraphCompliance:
    """
    The central Sub-AC 7c check: every artifact identifier used in the
    ScaleX task graph must be in the controlled vocabulary.

    Pass criterion: 0 violations detected.
    """

    def test_all_produces_evidence_keys_are_registered(self):
        """
        SCOPE: local unit test.
        Every Task.produces_evidence_key in build_task_graph() must be
        registered in ARTIFACT_REGISTRY.

        This verifies 100% compliance for the 'producer' side of the vocabulary.
        """
        tasks = build_task_graph()
        violations = []
        for task in tasks:
            key = task.produces_evidence_key
            if key is None:
                continue  # task does not produce evidence (acceptable)
            if key not in ARTIFACT_REGISTRY:
                violations.append(
                    f"  Task '{task.name}': produces_evidence_key={key!r} "
                    "NOT in ARTIFACT_REGISTRY"
                )
        assert not violations, (
            f"[Sub-AC 7c] produces_evidence_key compliance FAILURES "
            f"({len(violations)} violation(s)):\n" + "\n".join(violations)
        )

    def test_all_evidential_dep_keys_are_registered(self):
        """
        SCOPE: local unit test.
        Every EvidentialDep.evidence_key in build_task_graph() must be
        registered in ARTIFACT_REGISTRY.

        This verifies 100% compliance for the 'consumer' side of the vocabulary.
        """
        tasks = build_task_graph()
        violations = []
        for task in tasks:
            for dep in task.evidence_deps:
                if dep.evidence_key not in ARTIFACT_REGISTRY:
                    violations.append(
                        f"  Task '{task.name}': EvidentialDep.evidence_key="
                        f"{dep.evidence_key!r} NOT in ARTIFACT_REGISTRY"
                    )
        assert not violations, (
            f"[Sub-AC 7c] EvidentialDep.evidence_key compliance FAILURES "
            f"({len(violations)} violation(s)):\n" + "\n".join(violations)
        )

    def test_all_registered_keys_have_explicit_granularity(self):
        """
        SCOPE: local unit test.
        Sub-AC 7c requires 'explicit granularity levels' — every identifier
        referenced by task records must carry a GranularityLevel.

        This test checks that all keys used in the task graph (both producer
        and consumer sides) have explicit granularity in the registry.
        """
        tasks = build_task_graph()
        used_keys = set()

        for task in tasks:
            if task.produces_evidence_key:
                used_keys.add(task.produces_evidence_key)
            for dep in task.evidence_deps:
                used_keys.add(dep.evidence_key)

        violations = []
        for key in used_keys:
            if key not in ARTIFACT_REGISTRY:
                violations.append(f"  {key!r}: not in registry (no granularity)")
            elif ARTIFACT_REGISTRY[key].granularity is None:
                violations.append(f"  {key!r}: granularity is None")

        assert not violations, (
            "[Sub-AC 7c] Explicit granularity FAILURES:\n" + "\n".join(violations)
        )

    def test_no_unregistered_keys_in_full_graph(self):
        """
        SCOPE: local unit test.
        Full compliance check: collect ALL keys from both sides (producer +
        consumer) and verify NONE are outside the controlled vocabulary.

        Pass criterion: 0 unregistered keys found.
        """
        tasks = build_task_graph()
        all_used_keys = set()

        for task in tasks:
            if task.produces_evidence_key:
                all_used_keys.add(task.produces_evidence_key)
            for dep in task.evidence_deps:
                all_used_keys.add(dep.evidence_key)

        unregistered = sorted(
            k for k in all_used_keys if k not in ARTIFACT_REGISTRY
        )
        assert not unregistered, (
            f"[Sub-AC 7c] UNREGISTERED artifact identifiers found "
            f"({len(unregistered)} key(s)):\n"
            + "\n".join(f"  {k!r}" for k in unregistered)
            + f"\n\nRegistered keys:\n"
            + "\n".join(f"  {k!r}" for k in get_all_registered_keys())
        )

    def test_compliance_summary_100_percent(self):
        """
        SCOPE: local unit test.
        Meta-test: compute compliance % and assert it equals 100.
        This test serves as the summary evidence that Sub-AC 7c is satisfied.
        """
        tasks = build_task_graph()
        total_keys = 0
        compliant_keys = 0

        for task in tasks:
            if task.produces_evidence_key:
                total_keys += 1
                if task.produces_evidence_key in ARTIFACT_REGISTRY:
                    compliant_keys += 1
            for dep in task.evidence_deps:
                total_keys += 1
                if dep.evidence_key in ARTIFACT_REGISTRY:
                    compliant_keys += 1

        compliance_pct = (compliant_keys / total_keys * 100) if total_keys else 0.0
        assert compliance_pct == 100.0, (
            f"[Sub-AC 7c] Compliance: {compliant_keys}/{total_keys} "
            f"({compliance_pct:.1f}%) -- expected 100.0%"
        )


# ---------------------------------------------------------------------------
# TC-AV-4: Import-time validation (scalex_tasks._REGISTERED_KEYS)
# ---------------------------------------------------------------------------

class TestImportTimeValidation:
    def test_registered_keys_list_is_populated(self):
        """
        scalex_tasks._REGISTERED_KEYS must be non-empty, proving that
        validate_artifact_id() ran at import time.
        """
        assert _REGISTERED_KEYS, (
            "scalex_tasks._REGISTERED_KEYS is empty -- "
            "import-time vocabulary validation did not run"
        )

    def test_registered_keys_count_matches_unique_keys_in_graph(self):
        """
        The number of keys validated at import time must equal the number of
        unique artifact identifiers in the task graph.
        """
        tasks = build_task_graph()
        unique_graph_keys = set()
        for task in tasks:
            if task.produces_evidence_key:
                unique_graph_keys.add(task.produces_evidence_key)
            for dep in task.evidence_deps:
                unique_graph_keys.add(dep.evidence_key)

        assert len(_REGISTERED_KEYS) == len(unique_graph_keys), (
            f"_REGISTERED_KEYS has {len(_REGISTERED_KEYS)} entries but task graph "
            f"uses {len(unique_graph_keys)} unique keys.  "
            f"_REGISTERED_KEYS: {sorted(_REGISTERED_KEYS)}\n"
            f"Graph keys: {sorted(unique_graph_keys)}"
        )

    def test_all_graph_keys_appear_in_registered_keys(self):
        """
        Every key used in the task graph must appear in _REGISTERED_KEYS.
        This confirms import-time validation covers the full set of graph keys.
        """
        tasks = build_task_graph()
        graph_keys = set()
        for task in tasks:
            if task.produces_evidence_key:
                graph_keys.add(task.produces_evidence_key)
            for dep in task.evidence_deps:
                graph_keys.add(dep.evidence_key)

        registered_set = set(_REGISTERED_KEYS)
        missing_from_registered = graph_keys - registered_set
        assert not missing_from_registered, (
            f"Graph keys NOT covered by _REGISTERED_KEYS: "
            f"{sorted(missing_from_registered)}"
        )

    def test_sdi_init_produces_vm_list_not_completion(self):
        """
        Regression guard: sdi_init must produce 'sdi_init:vm_list', NOT the
        banned 'sdi_init:completion'.  This is the specific Sub-AC 7c fix.
        """
        tasks = {t.name: t for t in build_task_graph()}
        sdi_init = tasks["sdi_init"]
        assert sdi_init.produces_evidence_key == "sdi_init:vm_list", (
            f"sdi_init.produces_evidence_key must be 'sdi_init:vm_list' "
            f"(was 'sdi_init:completion' -- banned lifecycle verb). "
            f"Got: {sdi_init.produces_evidence_key!r}"
        )

    def test_sdi_verify_vms_and_health_check_reference_vm_list(self):
        """
        Regression guard: tasks that depend on sdi_init output must reference
        'sdi_init:vm_list', not the old 'sdi_init:completion'.
        """
        tasks = {t.name: t for t in build_task_graph()}
        for task_name in ("sdi_verify_vms", "sdi_health_check"):
            task = tasks[task_name]
            dep_keys = {dep.evidence_key for dep in task.evidence_deps}
            assert "sdi_init:vm_list" in dep_keys, (
                f"Task '{task_name}' must reference 'sdi_init:vm_list' "
                f"(not the banned 'sdi_init:completion'). "
                f"Current evidence_dep keys: {dep_keys}"
            )
            assert "sdi_init:completion" not in dep_keys, (
                f"Task '{task_name}' still references banned 'sdi_init:completion'. "
                f"Current evidence_dep keys: {dep_keys}"
            )
