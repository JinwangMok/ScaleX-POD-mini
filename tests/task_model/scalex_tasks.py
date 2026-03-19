"""
task_model.scalex_tasks — ScaleX-POD-mini operational task registry  [Sub-AC 3b / 7c]

Defines the canonical task graph for the full provisioning pipeline.
Each task declares:
  - scope:         Bounded context (declared before evaluation, never discovered)
  - prerequisites: Causal deps — tasks that MUST succeed first (blocking)
  - evidence_deps: Evidential deps [Sub-AC 3b] — evidence items this task
                   relies on; executor triggers re-check when stale/missing
  - run_fn:        Callable -> Evidence  (None = probe-only / not yet wired)

[Sub-AC 7c] All artifact identifier strings used as produces_evidence_key or
EvidentialDep.evidence_key are drawn from the controlled vocabulary defined in
ops/artifact_vocabulary.py and validated at import time via validate_artifact_id().
The module-level _REGISTERED_KEYS list below performs this validation and ensures
that every key in this file is a registered identifier with an explicit granularity
level.

Combined dependency graph (edges annotated with type):
  check_ssh_connectivity
          |  [CAUSAL]         produces: "check_ssh_connectivity:reachability" [ATOMIC]
          v
  gather_hardware_facts
     evidence_deps: [check_ssh_connectivity:reachability  (TTL=600s)]
          |  [CAUSAL]
          v
  sdi_init ─────────────────────────────────────┐
     evidence_deps: [check_ssh_connectivity      |  [CAUSAL]
                     gather_hardware_facts]       v
          |  [CAUSAL]                   sdi_health_check
          v                               evidence_deps: [sdi_init:vm_list]
  sdi_verify_vms
     evidence_deps: [sdi_init:vm_list]
          |  [CAUSAL]
          ├─────────────────────────────┐
          v                             v
  kubespray_tower              kubespray_sandbox
     evidence_deps:               evidence_deps:
       [sdi_verify_vms:vm_ready]    [sdi_verify_vms:vm_ready]
          |  [CAUSAL]
          ├─────────────────────────────┐
          v                             v
  tower_post_install_verify    kubespray_sandbox (cont.)
     evidence_deps:
       [kubespray_tower:cluster_healthy]
          |  [CAUSAL]
          v
  gitops_bootstrap
     evidence_deps: [tower_post_install_verify:api_reachable]
          |  [CAUSAL]
          v
  argocd_sync_healthy
     evidence_deps: [gitops_bootstrap:spread_applied]
          |  [CAUSAL]
          v
  cf_tunnel_healthy
     evidence_deps: [argocd_sync_healthy:all_synced,
                     check_ssh_connectivity:reachability]
          |  [CAUSAL]                [CAUSAL]
          v                               v
  dash_headless_verify       scalex_dash_token_provisioned
     evidence_deps:             evidence_deps:
       [cf_tunnel_healthy:tunnel_up]  [cf_tunnel_healthy:tunnel_up]

Network safety invariant: check_ssh_connectivity is a prerequisite for EVERY
remote operation.  Its evidence must be fresh (<=600 s) -- evidential deps on
check_ssh_connectivity trigger automatic re-check before any remote task runs.
"""

from __future__ import annotations

import subprocess
import time
from typing import List

from tests.task_model.model import Evidence, EvidentialDep, Task
from ops.artifact_vocabulary import validate_artifact_id  # [Sub-AC 7c]

# ---------------------------------------------------------------------------
# [Sub-AC 7c] Import-time controlled-vocabulary enforcement
#
# Every artifact identifier used as produces_evidence_key or EvidentialDep.evidence_key
# MUST appear in the controlled vocabulary (ops/artifact_vocabulary.py) with an
# explicit GranularityLevel.  validate_artifact_id() raises KeyError immediately if
# the key is not registered, preventing an unregistered identifier from entering the
# task graph.
#
# Non-compliance corrected in this pass:
#   sdi_init:completion  ->  sdi_init:vm_list
#     Reason: ":completion" is a banned lifecycle verb (no noun / state descriptor).
#             ":vm_list" names the specific artifact produced (per the graph diagram).
# ---------------------------------------------------------------------------
_REGISTERED_KEYS = [
    validate_artifact_id("check_ssh_connectivity:reachability").key,
    validate_artifact_id("gather_hardware_facts:hw_facts").key,
    validate_artifact_id("sdi_init:vm_list").key,
    validate_artifact_id("sdi_verify_vms:vm_ready").key,
    validate_artifact_id("sdi_health_check:virsh_status").key,
    validate_artifact_id("kubespray_tower:cluster_healthy").key,
    validate_artifact_id("kubespray_sandbox:cluster_healthy").key,
    validate_artifact_id("tower_post_install_verify:api_reachable").key,
    validate_artifact_id("gitops_bootstrap:spread_applied").key,
    validate_artifact_id("argocd_sync_healthy:all_synced").key,
    validate_artifact_id("cf_tunnel_healthy:tunnel_up").key,
    validate_artifact_id("dash_headless_verify:snapshot_valid").key,
    validate_artifact_id("scalex_dash_token_provisioned:token_valid").key,
]


# ---------------------------------------------------------------------------
# Helper -- run a shell command and return Evidence
# ---------------------------------------------------------------------------

def _run_cmd(cmd: str, summary_prefix: str) -> Evidence:
    """Run a shell command; return Evidence with raw output."""
    result = subprocess.run(
        cmd,
        shell=True,
        capture_output=True,
        text=True,
        timeout=30,
    )
    raw = f"$ {cmd}\n--- stdout ---\n{result.stdout}\n--- stderr ---\n{result.stderr}\nexit_code: {result.returncode}"
    if result.returncode != 0:
        raise RuntimeError(
            f"Command failed (exit {result.returncode}):\n{raw}"
        )
    summary = f"{summary_prefix} exit={result.returncode}"
    return Evidence(
        captured_at=time.time(),
        raw_output=raw,
        summary=summary,
    )


# ---------------------------------------------------------------------------
# Task definitions
# ---------------------------------------------------------------------------

def build_task_graph() -> List[Task]:
    """
    Return the full list of Task objects representing the ScaleX pipeline.

    Each task declares:
      - prerequisites:  Causal dep edges (blocking; upstream MUST succeed first)
      - evidence_deps:  Evidential dep edges [Sub-AC 3b] -- evidence this task
                        relies on; executor triggers RECHECK_TRIGGERED when stale

    All produces_evidence_key and EvidentialDep.evidence_key values are
    registered in ops/artifact_vocabulary.py [Sub-AC 7c].  The _REGISTERED_KEYS
    list at module level validates this at import time.
    """
    return [
        # -- Layer 0: Network safety pre-condition --------------------------
        Task(
            name="check_ssh_connectivity",
            scope="bare-metal: all playbox nodes reachable via SSH",
            prerequisites=[],
            # No evidential deps -- this IS the root evidence source.
            evidence_deps=[],
            produces_evidence_key="check_ssh_connectivity:reachability",
            description=(
                "Verify SSH connectivity to all bare-metal nodes BEFORE any "
                "remote operation.  Re-run whenever evidence is stale (>10 min)."
            ),
            run_fn=lambda: _run_cmd(
                "echo 'SSH connectivity check (probe-only in test mode)' && hostname",
                "ssh_check",
            ),
        ),

        # -- Layer 1: Hardware facts ----------------------------------------
        Task(
            name="gather_hardware_facts",
            scope="bare-metal: CPU/RAM/disk/GPU facts for all nodes",
            prerequisites=["check_ssh_connectivity"],
            # Evidential dep: relies on fresh SSH reachability evidence
            evidence_deps=[
                EvidentialDep(
                    evidence_key="check_ssh_connectivity:reachability",
                    source_task_name="check_ssh_connectivity",
                    max_age_s=600,
                ),
            ],
            produces_evidence_key="gather_hardware_facts:hw_facts",
            description=(
                "Gather hardware facts from all bare-metal nodes via "
                "scalex facts --all.  Blocked until SSH is verified. "
                "Evidence dep: SSH reachability must be fresh."
            ),
            # run_fn intentionally None -- wired in integration tests only
        ),

        # -- Layer 2: SDI init ----------------------------------------------
        Task(
            name="sdi_init",
            scope="sdi: libvirt VM pool creation on all bare-metal nodes",
            prerequisites=["gather_hardware_facts"],
            # Evidential deps: SSH reachability + hardware facts must be fresh
            evidence_deps=[
                EvidentialDep(
                    evidence_key="check_ssh_connectivity:reachability",
                    source_task_name="check_ssh_connectivity",
                    max_age_s=600,
                ),
                EvidentialDep(
                    evidence_key="gather_hardware_facts:hw_facts",
                    source_task_name="gather_hardware_facts",
                    max_age_s=600,
                ),
            ],
            # [Sub-AC 7c] was "sdi_init:completion" -- replaced with registered
            # identifier "sdi_init:vm_list" (FINE granularity, names the artifact).
            produces_evidence_key="sdi_init:vm_list",
            description=(
                "Run scalex sdi init to create VM pools from sdi-specs.yaml. "
                "Produces sdi_init:vm_list -- inventory of all VMs created. "
                "Evidence deps: SSH reachability + hardware facts must be fresh."
            ),
        ),

        Task(
            name="sdi_verify_vms",
            scope="sdi: verify all VMs are running post-init",
            prerequisites=["sdi_init"],
            # Evidential deps: SDI VM list + SSH reachability (remote op)
            evidence_deps=[
                EvidentialDep(
                    # [Sub-AC 7c] was "sdi_init:completion" -- updated to registered key
                    evidence_key="sdi_init:vm_list",
                    source_task_name="sdi_init",
                    max_age_s=600,
                ),
                EvidentialDep(
                    evidence_key="check_ssh_connectivity:reachability",
                    source_task_name="check_ssh_connectivity",
                    max_age_s=600,
                ),
            ],
            produces_evidence_key="sdi_verify_vms:vm_ready",
            description=(
                "Verify every VM defined in sdi-specs.yaml is in RUNNING state "
                "and reachable over SSH.  Evidence deps: sdi_init:vm_list + "
                "SSH reachability (network safety for remote op)."
            ),
        ),

        Task(
            name="sdi_health_check",
            scope="sdi: libvirt domain health on all bare-metal nodes",
            prerequisites=["sdi_init"],
            # Evidential deps: SDI VM list + SSH reachability (remote op)
            evidence_deps=[
                EvidentialDep(
                    # [Sub-AC 7c] was "sdi_init:completion" -- updated to registered key
                    evidence_key="sdi_init:vm_list",
                    source_task_name="sdi_init",
                    max_age_s=600,
                ),
                EvidentialDep(
                    evidence_key="check_ssh_connectivity:reachability",
                    source_task_name="check_ssh_connectivity",
                    max_age_s=600,
                ),
            ],
            produces_evidence_key="sdi_health_check:virsh_status",
            description=(
                "Check libvirt domain health (virsh list) on every bare-metal "
                "node.  Evidence deps: sdi_init:vm_list + SSH reachability "
                "(network safety for remote op)."
            ),
        ),

        # -- Layer 3: Kubernetes provisioning --------------------------------
        Task(
            name="kubespray_tower",
            scope="k8s-tower: Kubespray provision of tower cluster VMs",
            prerequisites=["sdi_verify_vms"],
            # Evidential dep: VM readiness evidence + SSH reachability
            evidence_deps=[
                EvidentialDep(
                    evidence_key="sdi_verify_vms:vm_ready",
                    source_task_name="sdi_verify_vms",
                    max_age_s=600,
                ),
                EvidentialDep(
                    evidence_key="check_ssh_connectivity:reachability",
                    source_task_name="check_ssh_connectivity",
                    max_age_s=600,
                ),
            ],
            produces_evidence_key="kubespray_tower:cluster_healthy",
            description=(
                "Run Kubespray to provision the tower cluster. "
                "Evidence deps: VM readiness + SSH reachability."
            ),
        ),

        Task(
            name="tower_post_install_verify",
            scope="k8s-tower: API server reachable, all nodes Ready",
            prerequisites=["kubespray_tower"],
            # Evidential dep: Kubespray cluster_healthy evidence
            evidence_deps=[
                EvidentialDep(
                    evidence_key="kubespray_tower:cluster_healthy",
                    source_task_name="kubespray_tower",
                    max_age_s=600,
                ),
            ],
            produces_evidence_key="tower_post_install_verify:api_reachable",
            description=(
                "Verify tower cluster: kubectl get nodes, all nodes Ready. "
                "Evidence dep: kubespray_tower:cluster_healthy."
            ),
        ),

        Task(
            name="kubespray_sandbox",
            scope="k8s-sandbox: Kubespray provision of sandbox cluster VMs",
            prerequisites=["sdi_verify_vms"],
            # Evidential dep: VM readiness + SSH reachability
            evidence_deps=[
                EvidentialDep(
                    evidence_key="sdi_verify_vms:vm_ready",
                    source_task_name="sdi_verify_vms",
                    max_age_s=600,
                ),
                EvidentialDep(
                    evidence_key="check_ssh_connectivity:reachability",
                    source_task_name="check_ssh_connectivity",
                    max_age_s=600,
                ),
            ],
            produces_evidence_key="kubespray_sandbox:cluster_healthy",
            description=(
                "Run Kubespray to provision the sandbox cluster. "
                "Evidence deps: VM readiness + SSH reachability."
            ),
        ),

        # -- Layer 4: GitOps bootstrap ---------------------------------------
        Task(
            name="gitops_bootstrap",
            scope="gitops: ArgoCD bootstrap via spread.yaml on tower cluster",
            prerequisites=["tower_post_install_verify"],
            # Evidential dep: tower API must be reachable (fresh evidence)
            evidence_deps=[
                EvidentialDep(
                    evidence_key="tower_post_install_verify:api_reachable",
                    source_task_name="tower_post_install_verify",
                    max_age_s=600,
                ),
            ],
            produces_evidence_key="gitops_bootstrap:spread_applied",
            description=(
                "Apply gitops/bootstrap/spread.yaml to tower cluster. "
                "Evidence dep: tower API reachable (fresh)."
            ),
        ),

        Task(
            name="argocd_sync_healthy",
            scope="gitops: all ArgoCD Applications in Synced+Healthy state",
            prerequisites=["gitops_bootstrap"],
            # Evidential dep: gitops bootstrap spread_applied must be fresh
            evidence_deps=[
                EvidentialDep(
                    evidence_key="gitops_bootstrap:spread_applied",
                    source_task_name="gitops_bootstrap",
                    max_age_s=600,
                ),
            ],
            produces_evidence_key="argocd_sync_healthy:all_synced",
            description=(
                "Wait for all ArgoCD Applications to reach Synced+Healthy. "
                "Evidence dep: gitops_bootstrap:spread_applied."
            ),
        ),

        # -- Layer 5: External access ----------------------------------------
        Task(
            name="cf_tunnel_healthy",
            scope="cf-tunnel: cloudflared pod running and API accessible",
            prerequisites=["argocd_sync_healthy"],
            # Evidential deps: ArgoCD sync state + SSH reachability (network safety)
            evidence_deps=[
                EvidentialDep(
                    evidence_key="argocd_sync_healthy:all_synced",
                    source_task_name="argocd_sync_healthy",
                    max_age_s=600,
                ),
                EvidentialDep(
                    evidence_key="check_ssh_connectivity:reachability",
                    source_task_name="check_ssh_connectivity",
                    max_age_s=600,
                ),
            ],
            produces_evidence_key="cf_tunnel_healthy:tunnel_up",
            description=(
                "Verify cloudflared tunnel pod is Running and tower API is "
                "reachable via CF Tunnel domain. "
                "Evidence deps: argocd all_synced + SSH reachability."
            ),
        ),

        Task(
            name="dash_headless_verify",
            scope="dash: scalex dash --headless returns valid cluster snapshot",
            prerequisites=["cf_tunnel_healthy"],
            # Evidential dep: CF tunnel must be up (fresh)
            evidence_deps=[
                EvidentialDep(
                    evidence_key="cf_tunnel_healthy:tunnel_up",
                    source_task_name="cf_tunnel_healthy",
                    max_age_s=600,
                ),
            ],
            produces_evidence_key="dash_headless_verify:snapshot_valid",
            description=(
                "Run scalex dash --headless and verify all clusters show "
                "connected status.  Evidence dep: cf_tunnel_healthy:tunnel_up."
            ),
        ),

        Task(
            name="scalex_dash_token_provisioned",
            scope="dash: scalex-dash SA token cached at _generated/clusters/*/dash-token",
            prerequisites=["cf_tunnel_healthy"],
            # Evidential dep: CF tunnel must be up (fresh)
            evidence_deps=[
                EvidentialDep(
                    evidence_key="cf_tunnel_healthy:tunnel_up",
                    source_task_name="cf_tunnel_healthy",
                    max_age_s=600,
                ),
            ],
            produces_evidence_key="scalex_dash_token_provisioned:token_valid",
            description=(
                "Verify scalex-dash SA token is provisioned and non-expired "
                "on all clusters.  Evidence dep: cf_tunnel_healthy:tunnel_up."
            ),
        ),
    ]
