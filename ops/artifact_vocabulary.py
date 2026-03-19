"""
ops/artifact_vocabulary.py — ScaleX-POD-mini P2 Operational Hardening  [Sub-AC 7c]

Controlled vocabulary for artifact identifiers used in the task model.

Every evidence key used in:
  - Task.produces_evidence_key
  - EvidentialDep.evidence_key

MUST be registered here with an explicit GranularityLevel.

GranularityLevel taxonomy
─────────────────────────
  ATOMIC  — single binary or scalar check (reachable/not, applied/not, valid/not)
  FINE    — structured artifact capturing a specific named resource set or
            per-instance state (e.g., VM inventory list, hardware facts dict,
            per-domain virsh output, cluster snapshot)
  COARSE  — composite evidence spanning multiple sub-systems or nodes
            (e.g., cluster_healthy covers API server + all nodes + CNI)

Rule: every key string MUST follow the "<task_name>:<aspect>" convention,
where <aspect> names the specific artifact or property captured — not the
lifecycle verb.  Vague verbs like ":completion" or ":done" are NOT accepted;
use a noun or state descriptor instead (e.g., ":vm_list", ":token_valid").

Usage::

    from ops.artifact_vocabulary import ARTIFACT_REGISTRY, validate_artifact_id

    # Will raise if key is not registered:
    validate_artifact_id("check_ssh_connectivity:reachability")

    # Full registry lookup:
    descriptor = ARTIFACT_REGISTRY["sdi_init:vm_list"]
    print(descriptor.granularity)   # GranularityLevel.FINE
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from typing import Dict


class GranularityLevel(str, Enum):
    """
    Explicit granularity classification for each registered artifact identifier.

    ATOMIC  — single binary or scalar observable; smallest meaningful unit of evidence.
              Examples: "reachable/not", "api_reachable", "token_valid"

    FINE    — structured artifact capturing a specific named resource set or
              per-instance state.  More than one field but scoped to one concern.
              Examples: hardware facts dict, VM inventory list, per-domain virsh status,
              cluster snapshot blob

    COARSE  — composite evidence spanning multiple sub-systems or nodes; useful as
              a gate but requires follow-up FINE/ATOMIC checks for diagnosis.
              Examples: kubespray_tower:cluster_healthy (covers API + nodes + CNI),
                        argocd_sync_healthy:all_synced (covers all ArgoCD apps)
    """

    ATOMIC = "atomic"
    FINE = "fine"
    COARSE = "coarse"


@dataclass(frozen=True)
class ArtifactDescriptor:
    """
    A single entry in the controlled vocabulary.

    Attributes:
        key:          Full identifier string: "<task_name>:<aspect>"
        granularity:  Explicit GranularityLevel — MUST NOT be omitted.
        description:  One-line human description of what this artifact captures.
        produced_by:  Task name that writes this artifact into the evidence store.
    """

    key: str
    granularity: GranularityLevel
    description: str
    produced_by: str

    def __post_init__(self) -> None:
        # Enforce "<task_name>:<aspect>" format
        if ":" not in self.key:
            raise ValueError(
                f"ArtifactDescriptor key {self.key!r} must follow "
                "'<task_name>:<aspect>' format"
            )
        task_part, aspect_part = self.key.split(":", 1)
        if not task_part.strip() or not aspect_part.strip():
            raise ValueError(
                f"ArtifactDescriptor key {self.key!r}: both task-name and "
                "aspect parts must be non-empty"
            )
        # Reject vague lifecycle verbs as aspects (non-noun / non-state descriptors)
        _BANNED_ASPECTS = {"completion", "done", "finished", "ran", "executed", "ok"}
        if aspect_part.lower() in _BANNED_ASPECTS:
            raise ValueError(
                f"ArtifactDescriptor key {self.key!r}: aspect {aspect_part!r} is "
                f"a vague lifecycle verb; use a noun or state descriptor instead. "
                f"Banned aspect names: {sorted(_BANNED_ASPECTS)}"
            )
        if not isinstance(self.granularity, GranularityLevel):
            raise TypeError(
                f"ArtifactDescriptor.granularity must be a GranularityLevel member, "
                f"got {type(self.granularity).__name__!r}"
            )
        if not self.description.strip():
            raise ValueError(
                f"ArtifactDescriptor {self.key!r}: description must not be empty"
            )
        if not self.produced_by.strip():
            raise ValueError(
                f"ArtifactDescriptor {self.key!r}: produced_by must not be empty"
            )


# ---------------------------------------------------------------------------
# Canonical controlled vocabulary
# ---------------------------------------------------------------------------
# Every key used in scalex_tasks.py produces_evidence_key / EvidentialDep.evidence_key
# MUST appear in this registry.  Add new entries here before using a new key.
# ---------------------------------------------------------------------------

_REGISTRY_ENTRIES: list[ArtifactDescriptor] = [

    # ── Layer 0: SSH / network ────────────────────────────────────────────
    ArtifactDescriptor(
        key="check_ssh_connectivity:reachability",
        granularity=GranularityLevel.ATOMIC,
        description=(
            "Binary SSH reachability check: all playbox nodes responded to "
            "a probe connection within timeout.  True=all reachable, False=one+ failed."
        ),
        produced_by="check_ssh_connectivity",
    ),

    # ── Layer 1: Hardware facts ───────────────────────────────────────────
    ArtifactDescriptor(
        key="gather_hardware_facts:hw_facts",
        granularity=GranularityLevel.FINE,
        description=(
            "Structured hardware inventory: per-node CPU count, RAM (GiB), "
            "disk layout, and GPU presence.  Collected via 'scalex facts --all'."
        ),
        produced_by="gather_hardware_facts",
    ),

    # ── Layer 2: SDI (VM pool) ────────────────────────────────────────────
    ArtifactDescriptor(
        key="sdi_init:vm_list",
        granularity=GranularityLevel.FINE,
        description=(
            "Inventory of VMs created by 'scalex sdi init': per-VM name, "
            "host, role, MAC address, and initial power state.  "
            "Produced after all domains in sdi-specs.yaml have been defined."
        ),
        produced_by="sdi_init",
    ),
    ArtifactDescriptor(
        key="sdi_verify_vms:vm_ready",
        granularity=GranularityLevel.ATOMIC,
        description=(
            "Binary gate: every VM in sdi-specs.yaml is in RUNNING state "
            "and SSH-reachable.  True=all ready, False=one+ not ready."
        ),
        produced_by="sdi_verify_vms",
    ),
    ArtifactDescriptor(
        key="sdi_health_check:virsh_status",
        granularity=GranularityLevel.FINE,
        description=(
            "Per-domain virsh state table from all bare-metal nodes: "
            "domain name, ID, and state string (running/paused/shut off)."
        ),
        produced_by="sdi_health_check",
    ),

    # ── Layer 3: Kubernetes provisioning ─────────────────────────────────
    ArtifactDescriptor(
        key="kubespray_tower:cluster_healthy",
        granularity=GranularityLevel.COARSE,
        description=(
            "Composite gate: Kubespray tower cluster provisioning succeeded "
            "— API server responding, all nodes Ready, CNI pods Running.  "
            "Coarse because it aggregates multiple sub-system checks."
        ),
        produced_by="kubespray_tower",
    ),
    ArtifactDescriptor(
        key="kubespray_sandbox:cluster_healthy",
        granularity=GranularityLevel.COARSE,
        description=(
            "Composite gate: Kubespray sandbox cluster provisioning succeeded "
            "— API server responding, all nodes Ready, CNI pods Running."
        ),
        produced_by="kubespray_sandbox",
    ),
    ArtifactDescriptor(
        key="tower_post_install_verify:api_reachable",
        granularity=GranularityLevel.ATOMIC,
        description=(
            "Binary gate: tower cluster API server returned HTTP 200 on "
            "/healthz after Kubespray run.  True=reachable, False=not."
        ),
        produced_by="tower_post_install_verify",
    ),

    # ── Layer 4: GitOps bootstrap ─────────────────────────────────────────
    ArtifactDescriptor(
        key="gitops_bootstrap:spread_applied",
        granularity=GranularityLevel.ATOMIC,
        description=(
            "Binary gate: gitops/bootstrap/spread.yaml was applied to the "
            "tower cluster without error.  True=applied, False=apply failed."
        ),
        produced_by="gitops_bootstrap",
    ),
    ArtifactDescriptor(
        key="argocd_sync_healthy:all_synced",
        granularity=GranularityLevel.COARSE,
        description=(
            "Composite gate: every ArgoCD Application managed by spread.yaml "
            "is in Synced+Healthy state.  Coarse because it covers all apps."
        ),
        produced_by="argocd_sync_healthy",
    ),

    # ── Layer 5: External access ─────────────────────────────────────────
    ArtifactDescriptor(
        key="cf_tunnel_healthy:tunnel_up",
        granularity=GranularityLevel.ATOMIC,
        description=(
            "Binary gate: cloudflared pod is Running and the CF Tunnel domain "
            "returns HTTP 200 from the tower API.  True=up, False=down."
        ),
        produced_by="cf_tunnel_healthy",
    ),
    ArtifactDescriptor(
        key="dash_headless_verify:snapshot_valid",
        granularity=GranularityLevel.FINE,
        description=(
            "Structured cluster snapshot from 'scalex dash --headless': "
            "per-cluster connection status, node counts, and namespace list.  "
            "Valid means all clusters show 'connected' and node counts > 0."
        ),
        produced_by="dash_headless_verify",
    ),
    ArtifactDescriptor(
        key="scalex_dash_token_provisioned:token_valid",
        granularity=GranularityLevel.ATOMIC,
        description=(
            "Binary gate: scalex-dash ServiceAccount token exists at "
            "_generated/clusters/<name>/dash-token and is not expired.  "
            "True=valid on all clusters, False=missing or expired on one+."
        ),
        produced_by="scalex_dash_token_provisioned",
    ),

    # ── Layer 3b: CNI health re-verification  [Sub-AC 2b] ────────────────
    ArtifactDescriptor(
        key="cilium_health_verify:cni_status",
        granularity=GranularityLevel.COARSE,
        description=(
            "Periodic Cilium CNI health re-verification: cilium pod running state "
            "in kube-system, agent health, and connectivity probe result.  "
            "Coarse because it aggregates pod status + agent health + connectivity.  "
            "raw_output includes an embedded ISO-8601 timestamp so evidence age "
            "can be verified independently of the captured_at field.  [Sub-AC 2b]"
        ),
        produced_by="cilium_health_verify",
    ),

    # ── Layer 6: Policy enforcement (Kyverno) ─────────────────────────────
    ArtifactDescriptor(
        key="kyverno_policy_check:policy_audit",
        granularity=GranularityLevel.FINE,
        description=(
            "Structured audit of Kyverno ClusterPolicy manifests in "
            "gitops/common/kyverno-policies/: per-policy name, apiVersion, "
            "kind, and rule count.  Validated for Kyverno v1 API compliance.  "
            "Captured with embedded ISO timestamp for freshness assertion."
        ),
        produced_by="kyverno_policy_check",
    ),
]

#: Immutable registry: maps evidence_key → ArtifactDescriptor
ARTIFACT_REGISTRY: Dict[str, ArtifactDescriptor] = {
    d.key: d for d in _REGISTRY_ENTRIES
}


# ---------------------------------------------------------------------------
# Public helpers
# ---------------------------------------------------------------------------

def validate_artifact_id(key: str) -> ArtifactDescriptor:
    """
    Return the ArtifactDescriptor for *key*, raising KeyError if not registered.

    Use this in task-graph construction to enforce the controlled vocabulary
    at import time rather than at runtime.

    Example::

        from ops.artifact_vocabulary import validate_artifact_id
        KEY = validate_artifact_id("sdi_init:vm_list").key
    """
    if key not in ARTIFACT_REGISTRY:
        registered = sorted(ARTIFACT_REGISTRY.keys())
        raise KeyError(
            f"Artifact identifier {key!r} is not in the controlled vocabulary.\n"
            f"Registered identifiers ({len(registered)}):\n"
            + "\n".join(f"  {k}" for k in registered)
        )
    return ARTIFACT_REGISTRY[key]


def get_all_registered_keys() -> list[str]:
    """Return a sorted list of all registered artifact identifier strings."""
    return sorted(ARTIFACT_REGISTRY.keys())
