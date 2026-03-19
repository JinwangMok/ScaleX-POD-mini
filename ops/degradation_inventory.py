"""
ops/degradation_inventory.py — ScaleX-POD-mini P2 Operational Hardening

Canonical known-acceptable-degradation inventory.  [Sub-AC 6b]

Every DegradationItem defined here MUST carry a root_cause with a valid
CauseKind classification.  This is the single authoritative source; the
config/known_degradations.yaml mirrors it in a human-readable form.

Classification decisions
────────────────────────────────────────────────────────────────────────
DEG-001  CoreDNS ContainerNotReady
  → ARCHITECTURAL_ASSUMPTION
    The bootstrap sequence for bare-metal Kubernetes (kubeadm/kubespray)
    inherently races between CNI readiness and the CoreDNS readiness probe.
    Accepting this transient is a deliberate design trade-off made at
    architecture time; the cluster reaches healthy state without manual
    intervention within 60 s.

DEG-002  ArgoCD dex-server CrashLoopBackOff
  → KNOWN_LIMITATION
    OIDC/Keycloak integration is out of scope for the current iteration
    (POD-mini P2).  The dex-server crash is expected and does not impair
    GitOps sync; it will be resolved when OIDC is wired (tracked in
    project_followup_cf_tunnel.md).

DEG-003  kube-vip NodeNotReady
  → ARCHITECTURAL_ASSUMPTION
    Single-node bare-metal VIP using ARP-based announcement is structural
    to the POD-mini networking design.  The brief NodeNotReady window while
    ARP propagates on br0 is an accepted consequence of this approach and
    resolves within 30 s without operator action.
────────────────────────────────────────────────────────────────────────

Scope boundary (declared before evaluation):
  This module is loaded purely in Python; it never makes network calls.
"""

from __future__ import annotations

from ops.task_model import CauseKind, DegradationItem, RootCause

# ---------------------------------------------------------------------------
# Canonical inventory
# ---------------------------------------------------------------------------

KNOWN_DEGRADATIONS: list[DegradationItem] = [
    DegradationItem(
        id="DEG-001",
        description=(
            "CoreDNS pods transiently report ContainerNotReady during the "
            "first 60 s after cluster bootstrap while kube-dns endpoints propagate."
        ),
        affects_task_ids=["all_nodes_ready"],
        ticket="N/A",
        root_cause=RootCause(
            cause_kind=CauseKind.ARCHITECTURAL_ASSUMPTION,
            description=(
                "Bare-metal kubeadm/kubespray bootstrap races CNI readiness with "
                "the CoreDNS readiness probe; transient failure is an accepted "
                "consequence of the single-node bootstrap sequence."
            ),
            ticket=None,
            mitigation=(
                "Health re-verification window is 60 s; checks that run after "
                "this window will see CoreDNS in Ready state."
            ),
        ),
    ),

    DegradationItem(
        id="DEG-002",
        description=(
            "argocd-dex-server CrashLoopBackOff because OIDC is not yet "
            "wired to a Keycloak realm in this lab environment."
        ),
        affects_task_ids=["argocd_synced"],
        ticket="docs/superpowers/plans/2026-03-17-dash-oidc-auth-future.md",
        root_cause=RootCause(
            cause_kind=CauseKind.KNOWN_LIMITATION,
            description=(
                "OIDC/Keycloak integration is intentionally deferred to a "
                "future iteration (post-P2); dex-server is non-functional "
                "until OIDC secrets are provisioned."
            ),
            ticket="docs/superpowers/plans/2026-03-17-dash-oidc-auth-future.md",
            mitigation=(
                "ArgoCD ApplicationController and Server continue to operate "
                "normally; only SSO login via dex is unavailable."
            ),
        ),
    ),

    DegradationItem(
        id="DEG-003",
        description=(
            "kube-vip pods briefly show NodeNotReady while the ARP announcement "
            "for the VIP propagates on br0 (resolves within 30 s)."
        ),
        affects_task_ids=["all_nodes_ready"],
        ticket="N/A",
        root_cause=RootCause(
            cause_kind=CauseKind.ARCHITECTURAL_ASSUMPTION,
            description=(
                "Bare-metal VIP via ARP announcement on br0 is structural to "
                "the POD-mini networking design; ARP propagation latency is an "
                "accepted consequence of this architecture."
            ),
            ticket=None,
            mitigation=(
                "Re-verification window for node-readiness checks is 30 s; "
                "kube-vip reaches stable state within this window."
            ),
        ),
    ),
]


def get_inventory() -> list[DegradationItem]:
    """Return the canonical known-acceptable-degradation inventory."""
    return list(KNOWN_DEGRADATIONS)
