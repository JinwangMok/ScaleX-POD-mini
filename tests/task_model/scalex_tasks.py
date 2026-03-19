"""
task_model.scalex_tasks — ScaleX-POD-mini operational task registry

Defines the canonical task graph for the full provisioning pipeline.
Each task declares:
  - scope:         Bounded context (e.g., "bare-metal", "sdi", "k8s-tower")
  - prerequisites: Causal deps (names of tasks that MUST succeed first)
  - run_fn:        Callable → Evidence  (None = probe-only / not yet wired)

Causal dependency graph (ASCII):
  check_ssh_connectivity
          │
          ▼
  gather_hardware_facts
          │
          ▼
  sdi_init ─────────────────────────────────┐
          │                                 │
          ▼                                 ▼
  sdi_verify_vms                  sdi_health_check
          │
          ▼
  kubespray_tower ──────────────────────────┐
          │                                 │
          ▼                                 ▼
  kubespray_sandbox             tower_post_install_verify
          │
          ▼
  gitops_bootstrap
          │
          ▼
  argocd_sync_healthy
          │
          ▼
  cf_tunnel_healthy ──────────────────────┐
          │                               │
          ▼                               ▼
  dash_headless_verify          scalex_dash_token_provisioned

Network safety: check_ssh_connectivity is a prerequisite for EVERY
remote operation and must re-run if evidence is stale (> 10 min).
"""

from __future__ import annotations

import subprocess
import time
from typing import List

from tests.task_model.model import Evidence, Task

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

    Prerequisites encode causal dependency edges:
      downstream tasks CANNOT run if an upstream task has not SUCCEEDED.
    """
    return [
        # ── Layer 0: Network safety pre-condition ───────────────────────
        Task(
            name="check_ssh_connectivity",
            scope="bare-metal: all playbox nodes reachable via SSH",
            prerequisites=[],
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
            prerequisites=["check_ssh_connectivity"],
            description=(
                "Gather hardware facts from all bare-metal nodes via "
                "scalex facts --all.  Blocked until SSH is verified."
            ),
            # run_fn intentionally None — wired in integration tests only
        ),

        # ── Layer 2: SDI init ────────────────────────────────────────────
        Task(
            name="sdi_init",
            scope="sdi: libvirt VM pool creation on all bare-metal nodes",
            prerequisites=["gather_hardware_facts"],
            description=(
                "Run scalex sdi init to create VM pools from sdi-specs.yaml."
            ),
        ),

        Task(
            name="sdi_verify_vms",
            scope="sdi: verify all VMs are running post-init",
            prerequisites=["sdi_init"],
            description=(
                "Verify every VM defined in sdi-specs.yaml is in RUNNING state "
                "and reachable over SSH.  Blocks Kubespray until VMs are ready."
            ),
        ),

        Task(
            name="sdi_health_check",
            scope="sdi: libvirt domain health on all bare-metal nodes",
            prerequisites=["sdi_init"],
            description=(
                "Check libvirt domain health (virsh list) on every bare-metal "
                "node.  Causal dep from sdi_init."
            ),
        ),

        # ── Layer 3: Kubernetes provisioning ────────────────────────────
        Task(
            name="kubespray_tower",
            scope="k8s-tower: Kubespray provision of tower cluster VMs",
            prerequisites=["sdi_verify_vms"],
            description=(
                "Run Kubespray to provision the tower (management) cluster. "
                "Blocked until all tower VMs are verified running."
            ),
        ),

        Task(
            name="tower_post_install_verify",
            scope="k8s-tower: API server reachable, all nodes Ready",
            prerequisites=["kubespray_tower"],
            description=(
                "Verify tower cluster: kubectl get nodes, all nodes Ready. "
                "Causal dep ensures Kubespray completed successfully."
            ),
        ),

        Task(
            name="kubespray_sandbox",
            scope="k8s-sandbox: Kubespray provision of sandbox cluster VMs",
            prerequisites=["sdi_verify_vms"],
            description=(
                "Run Kubespray to provision the sandbox (workload) cluster. "
                "Independent of tower_post_install_verify (parallel-safe)."
            ),
        ),

        # ── Layer 4: GitOps bootstrap ────────────────────────────────────
        Task(
            name="gitops_bootstrap",
            scope="gitops: ArgoCD bootstrap via spread.yaml on tower cluster",
            prerequisites=["tower_post_install_verify"],
            description=(
                "Apply gitops/bootstrap/spread.yaml to tower cluster. "
                "Blocked until tower API server is verified."
            ),
        ),

        Task(
            name="argocd_sync_healthy",
            scope="gitops: all ArgoCD Applications in Synced+Healthy state",
            prerequisites=["gitops_bootstrap"],
            description=(
                "Wait for all ArgoCD Applications to reach Synced+Healthy. "
                "Blocked until gitops_bootstrap completes."
            ),
        ),

        # ── Layer 5: External access ─────────────────────────────────────
        Task(
            name="cf_tunnel_healthy",
            scope="cf-tunnel: cloudflared pod running and API accessible",
            prerequisites=["argocd_sync_healthy"],
            description=(
                "Verify cloudflared tunnel pod is Running and tower API is "
                "reachable via CF Tunnel domain."
            ),
        ),

        Task(
            name="dash_headless_verify",
            scope="dash: scalex dash --headless returns valid cluster snapshot",
            prerequisites=["cf_tunnel_healthy"],
            description=(
                "Run scalex dash --headless and verify all clusters show "
                "connected status with recent snapshot timestamps."
            ),
        ),

        Task(
            name="scalex_dash_token_provisioned",
            scope="dash: scalex-dash SA token cached at _generated/clusters/*/dash-token",
            prerequisites=["cf_tunnel_healthy"],
            description=(
                "Verify scalex-dash ServiceAccount token is provisioned and "
                "non-expired on all clusters."
            ),
        ),
    ]
