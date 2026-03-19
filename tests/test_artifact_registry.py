"""
tests/test_artifact_registry.py — ScaleX-POD-mini P2 Operational Hardening

Sub-AC 7a verification: controlled vocabulary of artifact identifiers.

Scope boundary (declared before evaluation):
  - Unit tests for ops/artifact_registry.py
  - Integration tests: all tasks in tests/task_model/scalex_tasks.py must
    declare at least one valid scope_artifact_id
  - Integration tests: all tasks in ops/task_model.py (via direct Task construction)
    must reject invalid artifact IDs at validate() time
  - Out of scope: Kubernetes cluster, SSH, network operations

Known-acceptable-degradation inventory (explicit list):
  (none — all tests must pass cleanly)

Evidence: captured by pytest output below (raw command captured at run time).
"""

from __future__ import annotations

import pytest

from ops.artifact_registry import (
    ARTIFACT_REGISTRY,
    ArtifactGranularity,
    ArtifactId,
    ArtifactRefError,
    get_all_valid_refs,
    get_registry,
    parse_artifact_ref,
    validate_artifact_refs,
)
from ops.task_model import Task, Verdict


# ===========================================================================
# 1. ArtifactGranularity enum
# ===========================================================================

class TestArtifactGranularity:
    def test_all_required_levels_present(self):
        """All seven granularity levels must be defined."""
        required = {"file", "module", "service", "cluster", "node", "sdi", "network"}
        actual = {g.value for g in ArtifactGranularity}
        assert required == actual, (
            f"Missing granularity levels: {required - actual}"
        )

    def test_each_level_has_registry_entries(self):
        """Every granularity level must have at least one registered artifact."""
        for gran in ArtifactGranularity:
            registered = ARTIFACT_REGISTRY.get(gran, frozenset())
            assert len(registered) >= 1, (
                f"Granularity {gran.value!r} has no registered artifacts in "
                "ARTIFACT_REGISTRY"
            )


# ===========================================================================
# 2. ARTIFACT_REGISTRY contents
# ===========================================================================

class TestArtifactRegistryContents:
    """Spot-check that critical project artifacts are registered."""

    def test_clusters_registered(self):
        cluster_names = ARTIFACT_REGISTRY[ArtifactGranularity.CLUSTER]
        assert "tower" in cluster_names
        assert "sandbox" in cluster_names

    def test_key_services_registered(self):
        service_names = ARTIFACT_REGISTRY[ArtifactGranularity.SERVICE]
        for svc in ("argocd", "cloudflared", "coredns", "kube-vip",
                    "cilium", "cert-manager", "kyverno", "scalex-dash"):
            assert svc in service_names, f"Service {svc!r} not in registry"

    def test_all_playbox_nodes_registered(self):
        node_names = ARTIFACT_REGISTRY[ArtifactGranularity.NODE]
        for n in ("playbox-0", "playbox-1", "playbox-2", "playbox-3"):
            assert n in node_names, f"Node {n!r} not in registry"

    def test_sdi_artifacts_registered(self):
        sdi_names = ARTIFACT_REGISTRY[ArtifactGranularity.SDI]
        assert "vm-pool" in sdi_names
        assert "libvirt-domain" in sdi_names

    def test_network_artifacts_registered(self):
        net_names = ARTIFACT_REGISTRY[ArtifactGranularity.NETWORK]
        for artifact in ("ssh", "br0", "cf-tunnel", "tailscale"):
            assert artifact in net_names, f"Network artifact {artifact!r} not in registry"

    def test_critical_files_registered(self):
        file_names = ARTIFACT_REGISTRY[ArtifactGranularity.FILE]
        for f in ("config/sdi-specs.yaml", "config/k8s-clusters.yaml",
                  "gitops/bootstrap/spread.yaml"):
            assert f in file_names, f"File {f!r} not in registry"

    def test_key_modules_registered(self):
        mod_names = ARTIFACT_REGISTRY[ArtifactGranularity.MODULE]
        for m in ("scalex-cli", "ops", "gitops", "kubespray", "ansible"):
            assert m in mod_names, f"Module {m!r} not in registry"


# ===========================================================================
# 3. parse_artifact_ref
# ===========================================================================

class TestParseArtifactRef:
    def test_parse_granularity_name_only(self):
        aid = parse_artifact_ref("cluster:tower")
        assert aid.granularity == ArtifactGranularity.CLUSTER
        assert aid.name == "tower"
        assert aid.aspect is None

    def test_parse_with_aspect(self):
        aid = parse_artifact_ref("service:argocd:sync-state")
        assert aid.granularity == ArtifactGranularity.SERVICE
        assert aid.name == "argocd"
        assert aid.aspect == "sync-state"

    def test_parse_file_granularity(self):
        aid = parse_artifact_ref("file:config/sdi-specs.yaml")
        assert aid.granularity == ArtifactGranularity.FILE
        assert aid.name == "config/sdi-specs.yaml"

    def test_parse_network_with_aspect(self):
        aid = parse_artifact_ref("network:ssh:reachability")
        assert aid.granularity == ArtifactGranularity.NETWORK
        assert aid.name == "ssh"
        assert aid.aspect == "reachability"

    def test_str_roundtrip_without_aspect(self):
        ref = "cluster:tower"
        assert str(parse_artifact_ref(ref)) == ref

    def test_str_roundtrip_with_aspect(self):
        ref = "service:argocd:sync-state"
        assert str(parse_artifact_ref(ref)) == ref

    def test_case_insensitive_granularity(self):
        aid = parse_artifact_ref("CLUSTER:tower")
        assert aid.granularity == ArtifactGranularity.CLUSTER

    def test_invalid_format_too_few_parts(self):
        with pytest.raises(ArtifactRefError, match="expected"):
            parse_artifact_ref("cluster")  # no colon → only 1 part

    def test_unknown_granularity_raises(self):
        with pytest.raises(ArtifactRefError, match="Unknown granularity"):
            parse_artifact_ref("datacenter:rack-1")

    def test_unregistered_name_raises(self):
        with pytest.raises(ArtifactRefError, match="not registered"):
            parse_artifact_ref("cluster:production")  # "production" not in registry

    def test_unregistered_service_raises(self):
        with pytest.raises(ArtifactRefError, match="not registered"):
            parse_artifact_ref("service:prometheus")  # not registered

    def test_node_not_in_registry_raises(self):
        with pytest.raises(ArtifactRefError, match="not registered"):
            parse_artifact_ref("node:worker-99")


# ===========================================================================
# 4. validate_artifact_refs
# ===========================================================================

class TestValidateArtifactRefs:
    def test_empty_list_accepted(self):
        result = validate_artifact_refs([])
        assert result == []

    def test_all_valid_refs_pass(self):
        refs = [
            "cluster:tower",
            "cluster:sandbox",
            "service:argocd:sync-state",
            "network:ssh:reachability",
            "sdi:vm-pool:creation",
        ]
        result = validate_artifact_refs(refs)
        assert len(result) == 5
        assert all(isinstance(a, ArtifactId) for a in result)

    def test_first_invalid_ref_raises(self):
        refs = ["cluster:tower", "cluster:production"]  # "production" not in registry
        with pytest.raises(ArtifactRefError):
            validate_artifact_refs(refs)


# ===========================================================================
# 5. get_all_valid_refs and get_registry
# ===========================================================================

class TestRegistryHelpers:
    def test_get_all_valid_refs_returns_strings(self):
        refs = get_all_valid_refs()
        assert all(isinstance(r, str) for r in refs)
        assert len(refs) >= 10, "Expected at least 10 registered artifacts"

    def test_get_all_valid_refs_sorted(self):
        refs = get_all_valid_refs()
        assert refs == sorted(refs), "get_all_valid_refs() must return sorted list"

    def test_get_registry_json_serialisable(self):
        import json
        reg = get_registry()
        dumped = json.dumps(reg)
        parsed = json.loads(dumped)
        assert isinstance(parsed, dict)

    def test_get_registry_covers_all_granularities(self):
        reg = get_registry()
        for gran in ArtifactGranularity:
            assert gran.value in reg, f"Granularity {gran.value!r} missing from get_registry() output"

    def test_get_registry_entries_sorted(self):
        reg = get_registry()
        for gran_str, names in reg.items():
            assert names == sorted(names), (
                f"Names under granularity {gran_str!r} are not sorted"
            )


# ===========================================================================
# 6. Task.scope_artifact_ids integration with ops/task_model.py
# ===========================================================================

class TestTaskScopeArtifactIds:
    def _valid_task(self, artifact_ids=None) -> Task:
        return Task(
            id="TEST-1",
            name="test task",
            scope_boundary="unit-test scope only",
            scope_artifact_ids=artifact_ids or [],
        )

    def test_task_with_valid_artifact_ids_validates(self):
        task = self._valid_task(["cluster:tower", "service:argocd:sync-state"])
        task.validate()  # must not raise

    def test_task_with_empty_artifact_ids_validates(self):
        task = self._valid_task([])
        task.validate()  # empty list is accepted (backward compat)

    def test_task_with_invalid_artifact_id_raises(self):
        task = self._valid_task(["cluster:nonexistent"])
        with pytest.raises(ValueError, match="scope_artifact_ids"):
            task.validate()

    def test_task_with_bad_granularity_raises(self):
        task = self._valid_task(["datacenter:rack-1"])
        with pytest.raises(ValueError, match="scope_artifact_ids"):
            task.validate()

    def test_scope_artifact_ids_stored_correctly(self):
        ids = ["cluster:tower", "network:ssh:reachability"]
        task = self._valid_task(ids)
        assert task.scope_artifact_ids == ids

    def test_multiple_granularity_levels_accepted(self):
        task = self._valid_task([
            "cluster:tower",
            "service:argocd",
            "network:cf-tunnel",
            "module:gitops",
            "file:gitops/bootstrap/spread.yaml",
        ])
        task.validate()  # must not raise


# ===========================================================================
# 7. All scalex pipeline tasks must reference the registry  [Sub-AC 7a]
# ===========================================================================

class TestScalexTasksHaveArtifactIds:
    """
    Verifies that every task in the canonical scalex pipeline declares at
    least one scope_artifact_id that resolves to a registered artifact.

    This is the enforcement gate: if a new task is added without artifact IDs,
    this test fails, preventing silent undeclared scope.
    """

    def _get_all_tasks(self):
        from tests.task_model.scalex_tasks import build_task_graph
        return build_task_graph()

    def test_all_tasks_have_scope_artifact_ids(self):
        tasks = self._get_all_tasks()
        missing = [t.name for t in tasks if not t.scope_artifact_ids]
        assert not missing, (
            f"The following tasks have empty scope_artifact_ids (must declare "
            f"at least one artifact reference from the registry): {missing}"
        )

    def test_all_task_artifact_ids_are_valid(self):
        tasks = self._get_all_tasks()
        errors = []
        for task in tasks:
            for ref in task.scope_artifact_ids:
                try:
                    parse_artifact_ref(ref)
                except ArtifactRefError as exc:
                    errors.append(f"  task={task.name!r} ref={ref!r}: {exc}")
        assert not errors, (
            "Invalid artifact references found in scalex pipeline tasks:\n"
            + "\n".join(errors)
        )

    def test_all_task_artifact_ids_cover_expected_granularities(self):
        """
        Every task in the pipeline should reference at least one of:
          cluster, service, sdi, network, module, or node.
        (FILE-only scope is unusual and may indicate insufficient coverage.)
        """
        tasks = self._get_all_tasks()
        non_file_granularities = {
            ArtifactGranularity.CLUSTER, ArtifactGranularity.SERVICE,
            ArtifactGranularity.SDI, ArtifactGranularity.NETWORK,
            ArtifactGranularity.MODULE, ArtifactGranularity.NODE,
        }
        file_only_tasks = []
        for task in tasks:
            if not task.scope_artifact_ids:
                continue
            parsed = [parse_artifact_ref(r) for r in task.scope_artifact_ids]
            granularities = {a.granularity for a in parsed}
            if not granularities.intersection(non_file_granularities):
                file_only_tasks.append(task.name)

        assert not file_only_tasks, (
            f"These tasks reference only FILE-granularity artifacts, which may "
            f"indicate under-specified scope: {file_only_tasks}"
        )

    def test_task_names_in_artifact_aspect_are_informative(self):
        """
        Verify that artifact refs with aspects are not empty-string aspects.
        (An aspect of '' is meaningless noise.)
        """
        tasks = self._get_all_tasks()
        bad = []
        for task in tasks:
            for ref in task.scope_artifact_ids:
                try:
                    aid = parse_artifact_ref(ref)
                    if aid.aspect is not None and not aid.aspect.strip():
                        bad.append((task.name, ref))
                except ArtifactRefError:
                    pass  # caught by another test
        assert not bad, f"Empty aspect strings found: {bad}"
