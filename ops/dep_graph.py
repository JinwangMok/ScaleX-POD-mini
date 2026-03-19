"""
ops/dep_graph.py — ScaleX-POD-mini P2 Operational Hardening

Dependency graph with two edge types:

  CAUSAL:
    Edge  A ──causal──> B
    Meaning: B is BLOCKED until A has verdict PASS.
    Enforcement: topological sort; executor will not start B while A is pending/running/fail.

  EVIDENTIAL:
    Edge  A ──evidential──> B
    Meaning: B uses evidence produced by A; that evidence must be fresh (within TTL).
    Enforcement: before executing B, check A's latest evidence freshness.
                 If stale → emit RECHECK_TRIGGERED event and re-execute A first.

Graph invariants:
  - No self-loops
  - No duplicate edges (same src, dst, type)
  - No cycles (validated by topological sort)
"""

from __future__ import annotations

from collections import defaultdict, deque
from dataclasses import dataclass, field
from enum import Enum
from typing import Iterator


class EdgeType(str, Enum):
    CAUSAL = "causal"          # dependency completion blocks downstream
    EVIDENTIAL = "evidential"  # fresh evidence required; stale → re-check


@dataclass(frozen=True)
class Edge:
    src: str       # task id
    dst: str       # task id
    edge_type: EdgeType


@dataclass
class DepGraph:
    """
    Directed dependency graph for ScaleX-POD-mini task execution.

    Usage:
        g = DepGraph()
        g.add_task("A")
        g.add_task("B")
        g.add_edge("A", "B", EdgeType.CAUSAL)     # B blocked until A passes
        g.add_edge("A", "B", EdgeType.EVIDENTIAL) # B needs fresh evidence from A

        order = g.topological_sort()   # raises CycleError if cycle detected
        causal_preds = g.causal_predecessors("B")
        evid_preds   = g.evidential_predecessors("B")
    """

    _task_ids: set[str] = field(default_factory=set, repr=False)
    _edges: list[Edge] = field(default_factory=list, repr=False)

    # adjacency: src → list[Edge]
    _out: dict[str, list[Edge]] = field(
        default_factory=lambda: defaultdict(list), repr=False
    )
    # in-degree per task (causal edges only, used for Kahn's algorithm)
    _causal_in_degree: dict[str, int] = field(
        default_factory=lambda: defaultdict(int), repr=False
    )

    def add_task(self, task_id: str) -> None:
        if task_id in self._task_ids:
            return
        self._task_ids.add(task_id)
        self._causal_in_degree[task_id]  # ensure key exists (defaultdict)

    def add_edge(self, src: str, dst: str, edge_type: EdgeType) -> Edge:
        """
        Add a directed edge src → dst with the given type.
        Both task ids must have been registered via add_task().
        Duplicate edges (same src/dst/type) are silently ignored.
        """
        if src not in self._task_ids:
            raise ValueError(f"Unknown source task: {src!r}")
        if dst not in self._task_ids:
            raise ValueError(f"Unknown destination task: {dst!r}")
        if src == dst:
            raise ValueError(f"Self-loop not allowed: {src!r}")

        # Dedup
        e = Edge(src=src, dst=dst, edge_type=edge_type)
        if e in self._edges:
            return e

        self._edges.append(e)
        self._out[src].append(e)
        if edge_type == EdgeType.CAUSAL:
            self._causal_in_degree[dst] += 1

        return e

    def causal_predecessors(self, task_id: str) -> list[str]:
        """Return task ids that have a CAUSAL edge pointing to task_id."""
        return [
            e.src for e in self._edges
            if e.dst == task_id and e.edge_type == EdgeType.CAUSAL
        ]

    def evidential_predecessors(self, task_id: str) -> list[str]:
        """Return task ids that have an EVIDENTIAL edge pointing to task_id."""
        return [
            e.src for e in self._edges
            if e.dst == task_id and e.edge_type == EdgeType.EVIDENTIAL
        ]

    def topological_sort(self) -> list[str]:
        """
        Kahn's algorithm over CAUSAL edges only.

        Returns a list of task ids in an order where every causal dependency
        appears before its dependents.

        Raises CycleError if a cycle is detected among causal edges.
        """
        in_deg: dict[str, int] = dict(self._causal_in_degree)
        queue: deque[str] = deque(
            sorted(t for t in self._task_ids if in_deg.get(t, 0) == 0)
        )
        result: list[str] = []

        while queue:
            node = queue.popleft()
            result.append(node)
            for edge in self._out.get(node, []):
                if edge.edge_type != EdgeType.CAUSAL:
                    continue
                in_deg[edge.dst] -= 1
                if in_deg[edge.dst] == 0:
                    queue.append(edge.dst)

        if len(result) != len(self._task_ids):
            cycle_nodes = self._task_ids - set(result)
            raise CycleError(
                f"Cycle detected among causal edges involving: {sorted(cycle_nodes)}"
            )

        return result

    def all_edges(self) -> Iterator[Edge]:
        yield from self._edges

    def task_ids(self) -> list[str]:
        return sorted(self._task_ids)


class CycleError(Exception):
    """Raised when a cycle is detected in the causal dependency graph."""
