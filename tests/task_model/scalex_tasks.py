"""
task_model.scalex_tasks — ScaleX-POD-mini operational task registry  [Sub-AC 3b]

Defines the canonical task graph for the full provisioning pipeline.
Each task declares:
  - scope:         Bounded context (declared before evaluation, never discovered)
  - prerequisites: Causal deps — tasks that MUST succeed first (blocking)
  - evidence_deps: Evidential deps [Sub-AC 3b] — evidence items this task
                   relies on; executor triggers re-check when stale/missing
  - run_fn:        Callable → Evidence  (None = probe-only / not yet wired)

Combined dependency graph (edges annotated with type):
  check_ssh_connectivity
          │  [CAUSAL]         produces: "check_ssh_connectivity" evidence
          ▼
  gather_hardware_facts
     evidence_deps: [check_ssh_connectivity:reachability  (TTL=600s)]
          │  [CAUSAL]
          ▼
  sdi_init ─────────────────────────────────────┐
     evidence_deps: [check_ssh_connectivity]     │  [CAUSAL]
          │  [CAUSAL]                            ▼
          ▼                            sdi_health_check
  sdi_verify_vms                         evidence_deps: [sdi_init:completion]
     evidence_deps: [sdi_init:vm_list]
          │  [CAUSAL]
          ├─────────────────────────────┐
          ▼                             ▼
  kubespray_tower              kubespray_sandbox
     evidence_deps:               evidence_deps:
       [sdi_verify_vms:vm_ready]    [sdi_verify_vms:vm_ready]
          │  [CAUSAL]
          ├─────────────────────────────┐
          ▼                             ▼
  tower_post_install_verify    kubespray_sandbox (cont.)
     evidence_deps:
       [kubespray_tower:cluster_healthy]
          │  [CAUSAL]
          ▼
  gitops_bootstrap
     evidence_deps: [tower_post_install_verify:api_reachable]
          │  [CAUSAL]
          ▼
  argocd_sync_healthy
     evidence_deps: [gitops_bootstrap:spread_applied]
          │  [CAUSAL]
          ▼
  cf_tunnel_healthy
     evidence_deps: [argocd_sync_healthy:all_synced,
                     check_ssh_connectivity:reachability]
          │  [CAUSAL]                [CAUSAL]
          ▼                               ▼
  dash_headless_verify       scalex_dash_token_provisioned
     evidence_deps:             evidence_deps:
       [cf_tunnel_healthy:tunnel_up]  [cf_tunnel_healthy:tunnel_up]

Network safety invariant: check_ssh_connectivity is a prerequisite for EVERY
remote operation.  Its evidence must be fresh (≤600 s) — evidential deps on
check_ssh_connectivity trigger automatic re-check before any remote task runs.
"""

from __future__ import annotations

import subprocess
import time
from typing import List

from tests.task_model.model import Evidence, EvidentialDep, Task

# Artifact reference strings follow the controlled vocabulary in
# ops/artifact_registry.py [Sub-AC 7a]: "<granularity>:<name>[:<aspect>]"
# All names must be registered in ARTIFACT_REGISTRY before being used here.

# ---------------------------------------------------------------------------
# Helper — run a shell command and return Evidence
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
      - evidence_deps:  Evidential dep edges [Sub-AC 3b] — evidence this task
                        relies on; executor triggers RECHECK_TRIGGERED when stale

    Evidence keys follow the convention "<producing_task_name>:<aspect>".
    The executor stores evidence under task.evidence_store_key() (default = task.name).
    """
    return [
        # ── Layer 0: Network safety pre-condition ───────────────────────
        Task(
            name="check_ssh_connectivity",
            scope="bare-metal: all playbox nodes reachable via SSH",
            scope_artifact_ids=[
                "network:ssh:reachability",
                "node:playbox-0",
                "node:playbox-1",
                "node:playbox-2",
                "node:playbox-3",
            ],
            prerequisites=[],
            # No evidential deps — this IS the root evidence source.
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

        # ── Layer 1: Hardware facts ──────────────────────────────────────
        Task(
            name="gather_hardware_facts",
            scope="bare-metal: CPU/RAM/disk/GPU facts for all nodes",
            scope_artifact_ids=[
                "node:playbox-0",
                "node:playbox-1",
                "node:playbox-2",
                "node:playbox-3",
                "module:scalex-cli",
            ],
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
            # run_fn intentionally None — wired in integration tests only
        ),

        # ── Layer 2: SDI init ────────────────────────────────────────────
        Task(
            name="sdi_init",
            scope="sdi: libvirt VM pool creation on all bare-metal nodes",
            scope_artifact_ids=[
                "sdi:vm-pool:creation",
                "file:config/sdi-specs.yaml",
                "module:scalex-cli",
            ],
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
            produces_evidence_key="sdi_init:completion",
            description=(
                "Run scalex sdi init to create VM pools from sdi-specs.yaml. "
                "Evidence deps: SSH reachability + hardware facts must be fresh."
            ),
        ),

        Task(
            name="sdi_verify_vms",
            scope="sdi: verify all VMs are running post-init",
            scope_artifact_ids=[
                "sdi:vm-pool",
                "sdi:libvirt-domain",
            ],
            prerequisites=["sdi_init"],
            # Evidential deps: SDI init completion + SSH reachability (remote op)
            evidence_deps=[
                EvidentialDep(
                    evidence_key="sdi_init:completion",
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
                "and reachable over SSH.  Evidence deps: sdi_init completion + "
                "SSH reachability (network safety for remote op)."
            ),
        ),

        Task(
            name="sdi_health_check",
            scope="sdi: libvirt domain health on all bare-metal nodes",
            scope_artifact_ids=[
                "sdi:libvirt-domain",
                "network:ssh:virsh-status",
            ],
            prerequisites=["sdi_init"],
            # Evidential deps: SDI init completion + SSH reachability (remote op)
            evidence_deps=[
                EvidentialDep(
                    evidence_key="sdi_init:completion",
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
                "node.  Evidence deps: sdi_init:completion + SSH reachability "
                "(network safety for remote op)."
            ),
        ),

        # ── Layer 3: Kubernetes provisioning ────────────────────────────
        Task(
            name="kubespray_tower",
            scope="k8s-tower: Kubespray provision of tower cluster VMs",
            scope_artifact_ids=[
                "cluster:tower:provisioning",
                "module:kubespray",
                "file:config/k8s-clusters.yaml",
            ],
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
            scope_artifact_ids=[
                "cluster:tower:api-reachable",
            ],
            prerequisites=["kubespray_tower"],
            # Evidential dep: Kubespray run completion evidence
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
            scope_artifact_ids=[
                "cluster:sandbox:provisioning",
                "module:kubespray",
                "file:config/k8s-clusters.yaml",
            ],
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

        # ── Layer 4: GitOps bootstrap ────────────────────────────────────
        Task(
            name="gitops_bootstrap",
            scope="gitops: ArgoCD bootstrap via spread.yaml on tower cluster",
            scope_artifact_ids=[
                "module:gitops",
                "file:gitops/bootstrap/spread.yaml",
                "service:argocd:bootstrap",
                "cluster:tower",
            ],
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
            scope_artifact_ids=[
                "service:argocd:sync-state",
                "cluster:tower",
            ],
            prerequisites=["gitops_bootstrap"],
            # Evidential dep: gitops bootstrap completion must be fresh
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

        # ── Layer 5: External access ─────────────────────────────────────
        Task(
            name="cf_tunnel_healthy",
            scope="cf-tunnel: cloudflared pod running and API accessible",
            scope_artifact_ids=[
                "service:cloudflared:tunnel-up",
                "network:cf-tunnel",
                "cluster:tower",
            ],
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
            scope_artifact_ids=[
                "module:scalex-cli",
                "network:cf-tunnel",
                "cluster:tower",
                "cluster:sandbox",
            ],
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
            scope_artifact_ids=[
                "service:scalex-dash:token",
                "cluster:tower",
                "cluster:sandbox",
            ],
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
