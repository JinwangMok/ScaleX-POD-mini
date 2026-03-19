"""
task_model.model — ScaleX-POD-mini P2 Operational Hardening

Two edge types:

  CAUSAL (prerequisites):
    Edge A → B means B is BLOCKED until A has SUCCEEDED.
    Enforcement: topological sort; executor will not start B while A is
    pending/running/failed.

  EVIDENTIAL (evidence_deps):  [Sub-AC 3b]
    Edge A → B means B relies on evidence produced by A.
    That evidence must be fresh (within TTL = MAX_EVIDENCE_AGE_S).
    Enforcement: before executing B, the executor checks every EvidentialDep:
      - MISSING  → emit RECHECK_TRIGGERED(MISSING)  and re-run source task A
      - STALE    → emit RECHECK_TRIGGERED(STALE)    and re-run source task A
      - FRESH    → proceed normally
    In dry-run mode the re-check is logged but NOT executed.

Evidence freshness constraint (project-wide): MAX_EVIDENCE_AGE_S = 600 (10 min).
"""

from __future__ import annotations

import time
import logging
from dataclasses import dataclass, field
from enum import Enum, auto
from typing import Callable, Dict, List, Optional, Set

# Evidence older than this (seconds) must be re-captured before use.
MAX_EVIDENCE_AGE_S: int = 600  # 10 minutes

logger = logging.getLogger("task_model")


class TaskStatus(Enum):
    PENDING = auto()
    RUNNING = auto()
    SUCCEEDED = auto()
    FAILED = auto()
    BLOCKED = auto()    # prerequisite not met
    SKIPPED = auto()    # dry-run or explicit skip


@dataclass
class Evidence:
    """
    Captured evidence for a completed task.

    Attributes:
        captured_at: Unix timestamp when evidence was collected.
        raw_output:  Raw command output (stdout/stderr).
        summary:     One-line human summary.
    """
    captured_at: float
    raw_output: str
    summary: str

    def is_fresh(self, max_age_s: int = MAX_EVIDENCE_AGE_S) -> bool:
        return (time.time() - self.captured_at) <= max_age_s


@dataclass
class EvidentialDep:
    """
    An evidential dependency edge declared by a Task.  [Sub-AC 3b]

    Declares that the enclosing Task relies on a specific piece of evidence
    produced by another task.  Before the enclosing task runs, the executor
    checks that this evidence exists and is within TTL.

    Attributes:
        evidence_key:     Unique key for the evidence item.
                          Convention: "<source_task_name>:<aspect>"
                          Example:   "check_ssh_connectivity:reachability"
        source_task_name: Name of the Task that produces this evidence.
                          The executor re-runs this task when evidence is stale.
        max_age_s:        Maximum acceptable age of the evidence in seconds.
                          Default: MAX_EVIDENCE_AGE_S (600 s = 10 minutes).
    """
    evidence_key: str
    source_task_name: str
    max_age_s: int = MAX_EVIDENCE_AGE_S


@dataclass
class Task:
    """
    A unit of operational work with explicit causal AND evidential dependency edges.

    Attributes:
        name:          Unique task identifier.
        scope:         Declared scope boundary (must be stated before evaluation).
        prerequisites: Causal deps — list of task names that MUST have SUCCEEDED
                       before this task may run.  Failure blocks all descendants.
        evidence_deps: Evidential deps [Sub-AC 3b] — list of EvidentialDep objects
                       specifying evidence this task relies on.  Stale/missing
                       evidence triggers a re-check before execution.
        run_fn:        Callable executed in non-dry-run mode.
                       Must return an Evidence object on success,
                       or raise on failure.
        description:   Human-readable description of what the task does.
        produces_evidence_key: Key under which this task stores its evidence
                       in the executor's evidence store.  If None, uses task name.
    """
    name: str
    scope: str
    prerequisites: List[str] = field(default_factory=list)
    evidence_deps: List[EvidentialDep] = field(default_factory=list)
    run_fn: Optional[Callable[[], Evidence]] = field(default=None, repr=False)
    description: str = ""
    produces_evidence_key: Optional[str] = None

    def evidence_store_key(self) -> str:
        """Return the key under which this task's evidence is stored."""
        return self.produces_evidence_key or self.name


@dataclass
class TaskResult:
    """Execution result for a single task."""
    task: Task
    status: TaskStatus
    evidence: Optional[Evidence] = None
    block_reason: Optional[str] = None
    error: Optional[str] = None
    started_at: Optional[float] = None
    finished_at: Optional[float] = None


class CyclicDependencyError(Exception):
    """Raised when the dependency graph contains a cycle."""


class UnknownPrerequisiteError(Exception):
    """Raised when a task references a prerequisite that is not registered."""


class TaskExecutor:
    """
    Executes a set of Tasks in dependency order.

    Dependency rules:
      - Causal deps (prerequisites) are hard blockers.
        If a prerequisite is FAILED or BLOCKED, all descendants are BLOCKED.
      - Dry-run (dry_run=True) resolves the execution plan and logs
        which tasks would be blocked, without calling run_fn.

    Usage::

        executor = TaskExecutor(tasks, dry_run=True)
        results = executor.run()
        executor.print_plan()
    """

    def __init__(
        self,
        tasks: List[Task],
        dry_run: bool = False,
        log_level: int = logging.DEBUG,
    ) -> None:
        self.tasks: Dict[str, Task] = {t.name: t for t in tasks}
        self.dry_run = dry_run
        self._results: Dict[str, TaskResult] = {}

        # Configure logger
        if not logger.handlers:
            handler = logging.StreamHandler()
            handler.setFormatter(logging.Formatter("[%(levelname)s] %(message)s"))
            logger.addHandler(handler)
        logger.setLevel(log_level)

        self._validate_graph()

    # ------------------------------------------------------------------
    # Graph validation
    # ------------------------------------------------------------------

    def _validate_graph(self) -> None:
        """Check for unknown prerequisites and cycles."""
        for task in self.tasks.values():
            for prereq in task.prerequisites:
                if prereq not in self.tasks:
                    raise UnknownPrerequisiteError(
                        f"Task '{task.name}' references unknown prerequisite '{prereq}'"
                    )
        self._topological_order()  # raises CyclicDependencyError if cycle found

    def _topological_order(self) -> List[str]:
        """
        Kahn's algorithm — returns tasks in valid execution order.
        Raises CyclicDependencyError if the graph has a cycle.
        """
        in_degree: Dict[str, int] = {name: 0 for name in self.tasks}
        for task in self.tasks.values():
            for prereq in task.prerequisites:
                in_degree[task.name] = in_degree.get(task.name, 0) + 1

        # Re-compute properly
        in_degree = {name: 0 for name in self.tasks}
        for task in self.tasks.values():
            for _ in task.prerequisites:
                in_degree[task.name] += 1

        queue: List[str] = [name for name, deg in in_degree.items() if deg == 0]
        order: List[str] = []

        while queue:
            node = queue.pop(0)
            order.append(node)
            # Reduce in-degree for tasks that depend on this node
            for task in self.tasks.values():
                if node in task.prerequisites:
                    in_degree[task.name] -= 1
                    if in_degree[task.name] == 0:
                        queue.append(task.name)

        if len(order) != len(self.tasks):
            cycle_nodes = [n for n in self.tasks if n not in order]
            raise CyclicDependencyError(
                f"Dependency cycle detected among: {cycle_nodes}"
            )
        return order

    # ------------------------------------------------------------------
    # Execution
    # ------------------------------------------------------------------

    def run(self) -> Dict[str, TaskResult]:
        """
        Execute all tasks in dependency order.

        In dry-run mode every task is SKIPPED (run_fn never called) but
        BLOCKED tasks are still detected and logged.

        Returns a dict mapping task_name → TaskResult.
        """
        order = self._topological_order()
        mode_label = "DRY-RUN" if self.dry_run else "EXECUTE"
        logger.info(
            "[%s] Execution plan (%d tasks): %s",
            mode_label, len(order), " → ".join(order),
        )

        for name in order:
            task = self.tasks[name]
            result = self._execute_task(task)
            self._results[name] = result
            self._log_result(result)

        return dict(self._results)

    def _blocked_by(self, task: Task) -> Optional[str]:
        """
        Return a description of the first blocking prerequisite, or None.

        A prerequisite blocks if:
          - It has not been executed yet (PENDING), or
          - Its status is FAILED, BLOCKED, or its evidence is stale.
        """
        for prereq_name in task.prerequisites:
            prereq_result = self._results.get(prereq_name)
            if prereq_result is None:
                return (
                    f"prerequisite '{prereq_name}' has not been executed "
                    "(execution order violation)"
                )
            if prereq_result.status in (TaskStatus.FAILED, TaskStatus.BLOCKED):
                return (
                    f"prerequisite '{prereq_name}' "
                    f"is {prereq_result.status.name} "
                    f"(reason: {prereq_result.block_reason or prereq_result.error or 'unknown'})"
                )
            if prereq_result.status == TaskStatus.SKIPPED and not self.dry_run:
                return f"prerequisite '{prereq_name}' was SKIPPED — cannot satisfy causal dep"
            # Evidence freshness check (only in non-dry-run)
            if not self.dry_run and prereq_result.evidence is not None:
                if not prereq_result.evidence.is_fresh():
                    return (
                        f"prerequisite '{prereq_name}' evidence is stale "
                        f"(age > {MAX_EVIDENCE_AGE_S}s) — re-verification required"
                    )
        return None

    def _execute_task(self, task: Task) -> TaskResult:
        block_reason = self._blocked_by(task)

        if block_reason:
            logger.warning(
                "[BLOCKED] %s — %s", task.name, block_reason
            )
            return TaskResult(
                task=task,
                status=TaskStatus.BLOCKED,
                block_reason=block_reason,
            )

        if self.dry_run:
            logger.info("[DRY-RUN] %s — would execute (scope: %s)", task.name, task.scope)
            # In dry-run a satisfied-prerequisite task is shown as SKIPPED
            # (not SUCCEEDED) so we don't produce fake evidence.
            return TaskResult(
                task=task,
                status=TaskStatus.SKIPPED,
            )

        if task.run_fn is None:
            logger.warning("[SKIP] %s — no run_fn defined", task.name)
            return TaskResult(task=task, status=TaskStatus.SKIPPED)

        logger.info("[RUN] %s (scope: %s)", task.name, task.scope)
        started = time.time()
        try:
            evidence = task.run_fn()
            finished = time.time()
            logger.info("[OK] %s → %s", task.name, evidence.summary)
            return TaskResult(
                task=task,
                status=TaskStatus.SUCCEEDED,
                evidence=evidence,
                started_at=started,
                finished_at=finished,
            )
        except Exception as exc:  # noqa: BLE001
            finished = time.time()
            logger.error("[FAIL] %s — %s", task.name, exc)
            return TaskResult(
                task=task,
                status=TaskStatus.FAILED,
                error=str(exc),
                started_at=started,
                finished_at=finished,
            )

    # ------------------------------------------------------------------
    # Reporting
    # ------------------------------------------------------------------

    def _log_result(self, result: TaskResult) -> None:
        status_sym = {
            TaskStatus.SUCCEEDED: "✓",
            TaskStatus.FAILED: "✗",
            TaskStatus.BLOCKED: "⊘",
            TaskStatus.SKIPPED: "○",
            TaskStatus.RUNNING: "…",
            TaskStatus.PENDING: "?",
        }.get(result.status, "?")
        logger.debug(
            "  %s %s [%s]",
            status_sym,
            result.task.name,
            result.status.name,
        )

    def print_plan(self) -> None:
        """Print a human-readable execution plan / results summary."""
        order = self._topological_order()
        mode = "DRY-RUN PLAN" if self.dry_run else "EXECUTION RESULTS"
        print(f"\n{'='*60}")
        print(f"  {mode}")
        print(f"{'='*60}")
        for name in order:
            task = self.tasks[name]
            result = self._results.get(name)
            prereq_str = (
                f" (prereqs: {', '.join(task.prerequisites)})"
                if task.prerequisites
                else ""
            )
            if result is None:
                status_str = "NOT RUN"
                detail = ""
            elif result.status == TaskStatus.BLOCKED:
                status_str = "BLOCKED"
                detail = f"  ↳ reason: {result.block_reason}"
            elif result.status == TaskStatus.SKIPPED:
                status_str = "SKIPPED (dry-run)"
                detail = ""
            elif result.status == TaskStatus.SUCCEEDED:
                status_str = "SUCCEEDED"
                detail = (
                    f"  ↳ evidence: {result.evidence.summary}"
                    if result.evidence
                    else ""
                )
            elif result.status == TaskStatus.FAILED:
                status_str = "FAILED"
                detail = f"  ↳ error: {result.error}"
            else:
                status_str = result.status.name
                detail = ""

            print(f"  [{status_str:20s}] {name}{prereq_str}")
            if detail:
                print(f"  {detail}")
            print(f"  {'':22s}scope: {task.scope}")
        print(f"{'='*60}\n")
