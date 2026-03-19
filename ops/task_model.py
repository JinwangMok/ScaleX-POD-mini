"""
ops/task_model.py — ScaleX-POD-mini P2 Operational Hardening

Task data model with:
- Explicit scope_boundary (declared before evaluation, never discovered during)
- Evidence freshness (timestamp + TTL; evidence older than EVIDENCE_TTL_SECONDS is stale)
- Known-acceptable-degradation inventory (explicit list, not prose)
- Verdict lifecycle: PENDING → RUNNING → PASS | FAIL | STALE_EVIDENCE
"""

from __future__ import annotations

import time
from dataclasses import dataclass, field
from enum import Enum
from typing import Any


# Evidence older than this many seconds MUST be re-captured before use in a verdict.
EVIDENCE_TTL_SECONDS: int = 600  # 10 minutes


class Verdict(str, Enum):
    PENDING = "PENDING"
    RUNNING = "RUNNING"
    PASS = "PASS"
    FAIL = "FAIL"
    STALE_EVIDENCE = "STALE_EVIDENCE"   # evidence expired; must re-capture
    BLOCKED = "BLOCKED"                  # causal dep not yet satisfied
    SKIPPED = "SKIPPED"                  # explicitly skipped (e.g. known-degraded)


@dataclass
class DegradationItem:
    """
    A single known-acceptable-degradation entry.

    All degradation items MUST be explicit records in the inventory, never
    described in prose.  Each item carries:
    - id: unique identifier (e.g. "DEG-001")
    - description: one-line human summary
    - affects_task_ids: which task IDs this degradation exempts from FAIL
    - ticket: optional tracking reference
    """
    id: str
    description: str
    affects_task_ids: list[str]
    ticket: str | None = None


@dataclass
class Evidence:
    """
    A captured piece of evidence for a task verdict.

    captured_at_epoch: Unix timestamp when evidence was captured.
    raw_output: verbatim command output or observation.
    source: how this evidence was obtained (e.g. "kubectl get nodes", "ssh check").
    """
    raw_output: str
    source: str
    captured_at_epoch: float = field(default_factory=time.time)

    def age_seconds(self) -> float:
        return time.time() - self.captured_at_epoch

    def is_stale(self, ttl_seconds: int = EVIDENCE_TTL_SECONDS) -> bool:
        return self.age_seconds() > ttl_seconds


@dataclass
class Task:
    """
    A single acceptance-criterion task in the ScaleX-POD-mini hardening pipeline.

    Fields:
        id:                       Unique identifier (e.g. "AC-3c")
        name:                     Human-readable name
        scope_boundary:           MUST be declared before evaluation (explicit string)
        evidence_ttl_seconds:     Override for this task's evidence TTL (default: global)
        known_acceptable_degradation_ids: Explicit list of DegradationItem IDs that
                                  apply to this task.  Never narrated in prose.
        verdict:                  Current verdict (starts PENDING)
        evidence:                 List of captured Evidence records
        outputs:                  Arbitrary key→value outputs produced by the task
    """
    id: str
    name: str
    scope_boundary: str          # required; raises if empty at validation time

    evidence_ttl_seconds: int = EVIDENCE_TTL_SECONDS
    known_acceptable_degradation_ids: list[str] = field(default_factory=list)
    verdict: Verdict = Verdict.PENDING
    evidence: list[Evidence] = field(default_factory=list)
    outputs: dict[str, Any] = field(default_factory=dict)

    def validate(self) -> None:
        """Raise ValueError if mandatory fields are missing or empty."""
        if not self.id.strip():
            raise ValueError("Task.id must not be empty")
        if not self.scope_boundary.strip():
            raise ValueError(
                f"Task {self.id!r}: scope_boundary must be declared explicitly "
                "before evaluation, not discovered during it"
            )

    def add_evidence(self, raw_output: str, source: str,
                     captured_at_epoch: float | None = None) -> Evidence:
        ev = Evidence(
            raw_output=raw_output,
            source=source,
            captured_at_epoch=captured_at_epoch if captured_at_epoch is not None
                              else time.time(),
        )
        self.evidence.append(ev)
        return ev

    def latest_evidence(self) -> Evidence | None:
        return self.evidence[-1] if self.evidence else None

    def evidence_is_fresh(self) -> bool:
        """True iff the latest evidence exists and is within TTL."""
        ev = self.latest_evidence()
        return ev is not None and not ev.is_stale(self.evidence_ttl_seconds)
