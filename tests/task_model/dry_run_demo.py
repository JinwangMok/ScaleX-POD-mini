#!/usr/bin/env python3
"""
dry_run_demo.py — AC 3a evidence capture script

Scope boundary: local execution only, no SSH/VMs touched.

Demonstrates:
  1. Full ScaleX pipeline dry-run (all tasks SKIPPED)
  2. SSH-failure scenario where all downstream tasks are BLOCKED (logged)
"""

from __future__ import annotations

import logging
import sys
import time

# Allow running from project root
sys.path.insert(0, ".")

from tests.task_model.model import Evidence, TaskExecutor, TaskStatus
from tests.task_model.scalex_tasks import build_task_graph


def section(title: str) -> None:
    print(f"\n{'#'*60}")
    print(f"# {title}")
    print(f"{'#'*60}")


def main() -> None:
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s [%(levelname)s] %(message)s",
        datefmt="%H:%M:%S",
    )

    # ── Demo 1: Full dry-run ─────────────────────────────────────────────
    section("DEMO 1: Full pipeline dry-run — no tasks executed")
    tasks = build_task_graph()
    executor = TaskExecutor(tasks, dry_run=True, log_level=logging.INFO)
    results = executor.run()
    executor.print_plan()

    skipped = sum(1 for r in results.values() if r.status == TaskStatus.SKIPPED)
    blocked = sum(1 for r in results.values() if r.status == TaskStatus.BLOCKED)
    print(f"Summary: {skipped} SKIPPED, {blocked} BLOCKED (expected: all SKIPPED)\n")

    # ── Demo 2: SSH failure → full pipeline blocked ──────────────────────
    section("DEMO 2: SSH check FAILS — entire pipeline is BLOCKED")
    tasks = build_task_graph()

    # Wire check_ssh_connectivity to fail
    for task in tasks:
        if task.name == "check_ssh_connectivity":
            def failing_ssh() -> Evidence:
                raise RuntimeError(
                    "SSH unreachable: playbox-0 refused connection (simulated)"
                )
            task.run_fn = failing_ssh

    executor2 = TaskExecutor(tasks, dry_run=False, log_level=logging.INFO)
    results2 = executor2.run()
    executor2.print_plan()

    failed = sum(1 for r in results2.values() if r.status == TaskStatus.FAILED)
    blocked2 = sum(1 for r in results2.values() if r.status == TaskStatus.BLOCKED)
    print(f"Summary: {failed} FAILED, {blocked2} BLOCKED\n")

    # Verify all downstream are BLOCKED
    downstream_ok = all(
        results2[n].status == TaskStatus.BLOCKED
        for n in [
            "gather_hardware_facts", "sdi_init", "sdi_verify_vms",
            "kubespray_tower", "gitops_bootstrap", "argocd_sync_healthy",
            "cf_tunnel_healthy", "dash_headless_verify",
        ]
    )
    print(f"All downstream tasks BLOCKED: {downstream_ok}")

    # ── Demo 3: Partial failure — sdi_init fails, independent paths ok ───
    section("DEMO 3: sdi_init FAILS — downstream SDI/K8s blocked, SSH check ok")
    tasks = build_task_graph()

    # Wire SSH to succeed, but sdi_init to fail
    for task in tasks:
        if task.name == "check_ssh_connectivity":
            task.run_fn = lambda: Evidence(
                captured_at=time.time(),
                raw_output="SSH ok (stub)",
                summary="ssh_check exit=0",
            )
        elif task.name == "gather_hardware_facts":
            task.run_fn = lambda: Evidence(
                captured_at=time.time(),
                raw_output="facts gathered (stub)",
                summary="hardware_facts exit=0",
            )
        elif task.name == "sdi_init":
            def fail_sdi() -> Evidence:
                raise RuntimeError("libvirt pool creation failed: insufficient disk")
            task.run_fn = fail_sdi

    executor3 = TaskExecutor(tasks, dry_run=False, log_level=logging.INFO)
    results3 = executor3.run()
    executor3.print_plan()

    print(f"check_ssh_connectivity: {results3['check_ssh_connectivity'].status.name}")
    print(f"gather_hardware_facts:  {results3['gather_hardware_facts'].status.name}")
    print(f"sdi_init:               {results3['sdi_init'].status.name}")
    print(f"sdi_verify_vms:         {results3['sdi_verify_vms'].status.name}")
    print(f"kubespray_tower:        {results3['kubespray_tower'].status.name}")
    print(f"gitops_bootstrap:       {results3['gitops_bootstrap'].status.name}")


if __name__ == "__main__":
    main()
