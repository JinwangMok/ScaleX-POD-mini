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
import uuid
import logging
from dataclasses import dataclass, field
from enum import Enum, auto
from typing import Callable, Dict, List, Optional, Set

# Evidence older than this (seconds) must be re-captured before use.
MAX_EVIDENCE_AGE_S: int = 600  # 10 minutes

logger = logging.getLogger("task_model")


@dataclass
class RunContext:
    """
    Captures the identity and wall-clock start time of a single executor run.  [Sub-AC 2a]

    Every evidence artifact produced during this run is tagged with run_id so
    that artifacts from separate runs are never confused.

    Attributes:
        run_id:     UUID4 string uniquely identifying this executor run.
        started_at: Unix wall-clock timestamp (time.time()) when the run began.
    """
    run_id: str
    started_at: float


def _new_run_context() -> RunContext:
    """Factory — create a fresh RunContext with a unique run_id and current wall time."""
    return RunContext(run_id=str(uuid.uuid4()), started_at=time.time())


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
        run_id:      Run-scoped UUID set by TaskExecutor when the evidence is
                     stored.  None only for evidence created outside an executor
                     context (e.g. in unit-test stubs or pre-seeded test state).
                     [Sub-AC 2a] All executor-produced artifacts carry run_id.
    """
    captured_at: float
    raw_output: str
    summary: str
    run_id: Optional[str] = None  # tagged by executor; None for externally-created stubs

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
        scope_artifact_ids: Controlled-vocabulary artifact references [Sub-AC 7a].
                       Each entry must be a valid "<granularity>:<name>[:<aspect>]"
                       string registered in ops/artifact_registry.ARTIFACT_REGISTRY.
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
    scope_artifact_ids: List[str] = field(default_factory=list)
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
    Executes a set of Tasks in dependency order, enforcing both edge types:

      CAUSAL (prerequisites):
        Hard blockers.  If a prerequisite FAILED or BLOCKED, descendants are
        BLOCKED.

      EVIDENTIAL (evidence_deps):  [Sub-AC 3b]
        Before running a task, the executor checks each EvidentialDep:
          - MISSING → log RECHECK_TRIGGERED(reason=MISSING) and re-execute
                      source task to capture fresh evidence.
          - STALE   → log RECHECK_TRIGGERED(reason=STALE, age=Xs > TTLs) and
                      re-execute source task.
          - FRESH   → log evidence_check=OK(age=Xs) and proceed.
        In dry-run mode, re-checks are planned and logged but NOT executed.

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

        # Run-scoped context: unique run_id + wall-clock start time.  [Sub-AC 2a]
        # Every evidence artifact produced during this run is tagged with run_id.
        self.run_context: RunContext = _new_run_context()

        # Evidence store: maps evidence_key → Evidence
        # Populated when tasks run (or pre-seeded with stale evidence for testing).
        self._evidence_store: Dict[str, Evidence] = {}

        # Configure logger
        if not logger.handlers:
            handler = logging.StreamHandler()
            handler.setFormatter(logging.Formatter("[%(levelname)s] %(message)s"))
            logger.addHandler(handler)
        logger.setLevel(log_level)

        logger.info(
            "[RUN_CONTEXT] run_id=%s started_at=%.3f",
            self.run_context.run_id,
            self.run_context.started_at,
        )

        self._validate_graph()

    def seed_evidence(
        self,
        evidence_key: str,
        raw_output: str,
        summary: str,
        age_seconds: float = 0.0,
    ) -> None:
        """
        Pre-seed the evidence store (for testing or restoring persisted state).

        age_seconds: how old the evidence should appear (0 = captured just now,
                     >600 = stale, triggers re-check).
        """
        captured_at = time.time() - age_seconds
        self._evidence_store[evidence_key] = Evidence(
            captured_at=captured_at,
            raw_output=raw_output,
            summary=summary,
        )

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

    # ------------------------------------------------------------------
    # Evidential dependency enforcement  [Sub-AC 3b]
    # ------------------------------------------------------------------

    def _check_evidential_deps(self, task: Task) -> None:
        """
        Inspect every EvidentialDep declared by the task.

        For each dep:
          - MISSING  → log RECHECK_TRIGGERED(reason=MISSING) and, in non-dry-run
                       mode, re-execute the source task to capture fresh evidence.
          - STALE    → log RECHECK_TRIGGERED(reason=STALE, age=Xs > TTLs) and,
                       in non-dry-run mode, re-execute the source task.
          - FRESH    → log evidence_dep OK(age=Xs) and continue.

        This method never blocks execution; it only ensures evidence is fresh
        before the consuming task runs (or logs what would happen in dry-run).
        """
        if not task.evidence_deps:
            return

        for dep in task.evidence_deps:
            ev = self._evidence_store.get(dep.evidence_key)

            if ev is None:
                # Evidence MISSING — must re-run source task
                logger.warning(
                    "[RECHECK_TRIGGERED] task=%s evidence_key=%r "
                    "reason=MISSING source_task=%s",
                    task.name,
                    dep.evidence_key,
                    dep.source_task_name,
                )
                if not self.dry_run:
                    self._run_recheck(dep.source_task_name, dep.evidence_key)
                else:
                    logger.info(
                        "[DRY-RUN][RECHECK] would re-run '%s' to capture "
                        "missing evidence '%s'",
                        dep.source_task_name,
                        dep.evidence_key,
                    )

            elif not ev.is_fresh(dep.max_age_s):
                # Evidence STALE — must re-run source task
                age = time.time() - ev.captured_at
                logger.warning(
                    "[RECHECK_TRIGGERED] task=%s evidence_key=%r "
                    "reason=STALE age=%.0fs ttl=%ds source_task=%s",
                    task.name,
                    dep.evidence_key,
                    age,
                    dep.max_age_s,
                    dep.source_task_name,
                )
                if not self.dry_run:
                    self._run_recheck(dep.source_task_name, dep.evidence_key)
                else:
                    logger.info(
                        "[DRY-RUN][RECHECK] would re-run '%s' to refresh "
                        "stale evidence '%s' (age=%.0fs > ttl=%ds)",
                        dep.source_task_name,
                        dep.evidence_key,
                        age,
                        dep.max_age_s,
                    )

            else:
                # Evidence FRESH — no action needed
                age = time.time() - ev.captured_at
                logger.debug(
                    "[EVIDENTIAL_DEP] task=%s evidence_key=%r status=FRESH "
                    "age=%.0fs ttl=%ds",
                    task.name,
                    dep.evidence_key,
                    age,
                    dep.max_age_s,
                )

    def _run_recheck(self, source_task_name: str, evidence_key: str) -> None:
        """
        Re-execute the named source task to refresh a piece of evidence.

        Called when evidence is MISSING or STALE.  The task's run_fn is
        invoked directly (not re-queued through the full executor) so that
        fresh evidence is available before the consuming task proceeds.
        """
        source_task = self.tasks.get(source_task_name)
        if source_task is None:
            logger.error(
                "[RECHECK_ERROR] source task '%s' not found in registry — "
                "cannot re-capture evidence '%s'",
                source_task_name,
                evidence_key,
            )
            return

        logger.info(
            "[RECHECK_RUNNING] re-executing '%s' to capture fresh evidence '%s'",
            source_task_name,
            evidence_key,
        )

        if source_task.run_fn is None:
            logger.warning(
                "[RECHECK_SKIP] '%s' has no run_fn — evidence '%s' remains stale",
                source_task_name,
                evidence_key,
            )
            return

        try:
            fresh_ev = source_task.run_fn()
            # Tag re-captured evidence with this run's identity.  [Sub-AC 2a]
            fresh_ev.run_id = self.run_context.run_id
            self._evidence_store[evidence_key] = fresh_ev
            logger.info(
                "[RECHECK_OK] '%s' re-executed — evidence '%s' is now FRESH "
                "(captured_at=%.0f run_id=%s summary=%s)",
                source_task_name,
                evidence_key,
                fresh_ev.captured_at,
                fresh_ev.run_id,
                fresh_ev.summary,
            )
        except Exception as exc:  # noqa: BLE001
            logger.error(
                "[RECHECK_FAIL] '%s' re-execution failed — evidence '%s' "
                "remains unavailable: %s",
                source_task_name,
                evidence_key,
                exc,
            )

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

        # --- Evidential dep enforcement [Sub-AC 3b] ---
        # Check (and potentially re-capture) all evidence this task relies on
        # BEFORE executing it.  In dry-run mode this logs the re-check plan.
        self._check_evidential_deps(task)

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
            # Tag evidence with run-scoped identity.  [Sub-AC 2a]
            evidence.run_id = self.run_context.run_id
            logger.info("[OK] %s → %s", task.name, evidence.summary)
            # Store produced evidence so downstream tasks can use it
            ev_key = task.evidence_store_key()
            self._evidence_store[ev_key] = evidence
            logger.debug(
                "[EVIDENCE_STORED] key=%r run_id=%s age=0s (just captured)",
                ev_key,
                evidence.run_id,
            )
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
