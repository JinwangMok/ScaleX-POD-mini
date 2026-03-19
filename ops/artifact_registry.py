"""
ops/artifact_registry.py — ScaleX-POD-mini P2 Operational Hardening

Controlled vocabulary of artifact identifiers with explicit granularity levels.
[Sub-AC 7a]

Every Task record MUST reference at least one artifact identifier from this
registry in its `scope_artifact_ids` field.  Adding a new artifact requires:
  1. Registering it in ARTIFACT_REGISTRY under the correct granularity level.
  2. (Optional) Updating this module's documentation table below.

Granularity levels (from finest to coarsest)
─────────────────────────────────────────────
  FILE      A specific file on disk (e.g. a config file, a generated kubeconfig).
  MODULE    A code module or tooling component (e.g. scalex-cli, ops, gitops).
  SERVICE   A running workload or system service (e.g. argocd, cloudflared).
  CLUSTER   A Kubernetes cluster (e.g. tower, sandbox).
  NODE      A bare-metal host or VM node (e.g. playbox-0, tower-vm-0).
  SDI       Software-Defined Infrastructure layer (libvirt VM pools).
  NETWORK   Network-level artifact (e.g. ssh, br0, cf-tunnel).

Artifact identifier string format:
  "<granularity>:<name>"           — e.g. "cluster:tower"
  "<granularity>:<name>:<aspect>"  — e.g. "service:argocd:sync-state"

  Where:
    granularity  must be a value of ArtifactGranularity (case-insensitive)
    name         must be in ARTIFACT_REGISTRY[granularity]
    aspect       optional free-form sub-qualifier (not validated against registry)

Scope boundary (declared before evaluation):
  This module is loaded purely in Python; it makes no network calls and
  touches no files outside the ops/ directory.

Canonical artifact inventory
─────────────────────────────────────────────────────────────────────────────
Granularity  Name                       Notes
──────────── ────────────────────────── ─────────────────────────────────────
FILE         config/sdi-specs.yaml      VM pool definitions
FILE         config/k8s-clusters.yaml   Cluster definitions
FILE         credentials/.baremetal-init.yaml  SSH init credentials
FILE         ops/task_model.py          Task data model
FILE         ops/dep_graph.py           Dependency graph implementation
FILE         ops/executor.py            Task executor
FILE         ops/artifact_registry.py   Artifact registry (this file)
FILE         gitops/bootstrap/spread.yaml  Root ArgoCD bootstrap manifest
MODULE       scalex-cli                 Rust CLI (facts/SDI/cluster/dash)
MODULE       ops                        Python operational hardening layer
MODULE       gitops                     ArgoCD-managed GitOps manifests
MODULE       kubespray                  Kubespray provisioner
MODULE       ansible                    Ansible node-prep playbooks
SERVICE      argocd                     GitOps controller (tower cluster)
SERVICE      cloudflared                Cloudflare Tunnel daemon
SERVICE      keycloak                   OIDC identity provider
SERVICE      coredns                    In-cluster DNS
SERVICE      kube-vip                   VIP/ARP announcement daemon
SERVICE      scalex-dash                Dashboard SA + RBAC
SERVICE      cilium                     CNI / network policy
SERVICE      cert-manager               TLS certificate controller
SERVICE      kyverno                    Policy controller
CLUSTER      tower                      Management cluster
CLUSTER      sandbox                    Workload cluster
NODE         playbox-0                  Bare-metal host 0
NODE         playbox-1                  Bare-metal host 1
NODE         playbox-2                  Bare-metal host 2
NODE         playbox-3                  Bare-metal host 3
SDI          vm-pool                    Libvirt VM pool (all nodes)
SDI          libvirt-domain             Individual libvirt domain
NETWORK      ssh                        SSH connectivity to any host
NETWORK      br0                        Linux bridge interface (host networking)
NETWORK      bond0                      NIC bond interface
NETWORK      cf-tunnel                  Cloudflare Tunnel end-to-end path
NETWORK      tailscale                  Tailscale overlay network
─────────────────────────────────────────────────────────────────────────────
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from typing import FrozenSet, Mapping


# ---------------------------------------------------------------------------
# Granularity levels
# ---------------------------------------------------------------------------

class ArtifactGranularity(str, Enum):
    """
    Explicit granularity levels for artifact identifiers.

    Ordered from finest (FILE) to coarsest (NETWORK/CLUSTER).
    Every artifact registered in ARTIFACT_REGISTRY belongs to exactly one level.
    """
    FILE    = "file"     # a specific file on disk
    MODULE  = "module"   # a code module or tooling component
    SERVICE = "service"  # a running workload / system service
    CLUSTER = "cluster"  # a Kubernetes cluster
    NODE    = "node"     # a bare-metal host or VM node
    SDI     = "sdi"      # Software-Defined Infrastructure layer
    NETWORK = "network"  # network-level artifact


# ---------------------------------------------------------------------------
# Canonical registry: granularity → frozenset of valid artifact names
# ---------------------------------------------------------------------------

ARTIFACT_REGISTRY: Mapping[ArtifactGranularity, FrozenSet[str]] = {
    ArtifactGranularity.FILE: frozenset({
        "config/sdi-specs.yaml",
        "config/k8s-clusters.yaml",
        "credentials/.baremetal-init.yaml",
        "ops/task_model.py",
        "ops/dep_graph.py",
        "ops/executor.py",
        "ops/artifact_registry.py",
        "gitops/bootstrap/spread.yaml",
    }),

    ArtifactGranularity.MODULE: frozenset({
        "scalex-cli",
        "ops",
        "gitops",
        "kubespray",
        "ansible",
    }),

    ArtifactGranularity.SERVICE: frozenset({
        "argocd",
        "cloudflared",
        "keycloak",
        "coredns",
        "kube-vip",
        "scalex-dash",
        "cilium",
        "cert-manager",
        "kyverno",
    }),

    ArtifactGranularity.CLUSTER: frozenset({
        "tower",
        "sandbox",
    }),

    ArtifactGranularity.NODE: frozenset({
        "playbox-0",
        "playbox-1",
        "playbox-2",
        "playbox-3",
    }),

    ArtifactGranularity.SDI: frozenset({
        "vm-pool",
        "libvirt-domain",
    }),

    ArtifactGranularity.NETWORK: frozenset({
        "ssh",
        "br0",
        "bond0",
        "cf-tunnel",
        "tailscale",
    }),
}


# ---------------------------------------------------------------------------
# ArtifactId — parsed representation of an artifact reference string
# ---------------------------------------------------------------------------

@dataclass(frozen=True)
class ArtifactId:
    """
    A parsed artifact identifier reference.

    Attributes:
        granularity:  The granularity level of this artifact.
        name:         The canonical artifact name (must be in ARTIFACT_REGISTRY).
        aspect:       Optional sub-qualifier (not validated; free-form).

    String format: "<granularity>:<name>" or "<granularity>:<name>:<aspect>"
    Example:        "cluster:tower"  or  "service:argocd:sync-state"
    """
    granularity: ArtifactGranularity
    name: str
    aspect: str | None = None

    def __str__(self) -> str:
        base = f"{self.granularity.value}:{self.name}"
        return f"{base}:{self.aspect}" if self.aspect else base


# ---------------------------------------------------------------------------
# Parsing and validation helpers
# ---------------------------------------------------------------------------

class ArtifactRefError(ValueError):
    """Raised when an artifact reference string is malformed or not in the registry."""


def parse_artifact_ref(ref: str) -> ArtifactId:
    """
    Parse an artifact reference string into an ArtifactId.

    Format: "<granularity>:<name>" or "<granularity>:<name>:<aspect>"

    Raises:
        ArtifactRefError: if the string is malformed (wrong number of parts),
                          the granularity is unknown, or the name is not
                          registered under that granularity.

    Examples:
        >>> parse_artifact_ref("cluster:tower")
        ArtifactId(granularity=<ArtifactGranularity.CLUSTER: 'cluster'>, name='tower', aspect=None)
        >>> parse_artifact_ref("service:argocd:sync-state")
        ArtifactId(granularity=<ArtifactGranularity.SERVICE: 'service'>, name='argocd', aspect='sync-state')
    """
    parts = ref.strip().split(":", 2)
    if len(parts) < 2:
        raise ArtifactRefError(
            f"Invalid artifact reference {ref!r}: expected "
            f"'<granularity>:<name>[:<aspect>]', got {len(parts)} part(s)."
        )

    raw_gran, name = parts[0].lower(), parts[1]
    aspect = parts[2] if len(parts) == 3 else None

    # Validate granularity
    try:
        granularity = ArtifactGranularity(raw_gran)
    except ValueError:
        valid = [g.value for g in ArtifactGranularity]
        raise ArtifactRefError(
            f"Unknown granularity {raw_gran!r} in artifact reference {ref!r}. "
            f"Valid granularity levels: {valid}"
        ) from None

    # Validate name against registry
    valid_names = ARTIFACT_REGISTRY.get(granularity, frozenset())
    if name not in valid_names:
        raise ArtifactRefError(
            f"Artifact name {name!r} is not registered under granularity "
            f"{granularity.value!r} in ARTIFACT_REGISTRY. "
            f"Registered names: {sorted(valid_names)}"
        )

    return ArtifactId(granularity=granularity, name=name, aspect=aspect)


def validate_artifact_refs(refs: list[str]) -> list[ArtifactId]:
    """
    Validate a list of artifact reference strings and return parsed ArtifactIds.

    All refs must be valid; raises ArtifactRefError on the first invalid entry.
    An empty list is accepted (field is optional for backward compatibility),
    but callers that require at least one ref should check len() themselves.

    Returns:
        List of parsed ArtifactId objects.
    """
    return [parse_artifact_ref(ref) for ref in refs]


def get_all_valid_refs() -> list[str]:
    """
    Return a sorted list of all valid artifact reference strings (name-only, no aspect).

    Useful for documentation generation and auto-complete hints.
    """
    refs = []
    for gran, names in ARTIFACT_REGISTRY.items():
        for name in sorted(names):
            refs.append(f"{gran.value}:{name}")
    return sorted(refs)


def get_registry() -> dict[str, list[str]]:
    """
    Return a JSON-serialisable snapshot of the registry.

    Maps granularity-level string → sorted list of artifact names.
    """
    return {
        gran.value: sorted(names)
        for gran, names in ARTIFACT_REGISTRY.items()
    }
