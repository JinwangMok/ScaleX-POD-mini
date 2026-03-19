"""
ops/task_model.py — ScaleX-POD-mini P2 Operational Hardening

Task data model with:
- Explicit scope_boundary (declared before evaluation, never discovered during)
- Evidence freshness (timestamp + TTL; evidence older than EVIDENCE_TTL_SECONDS is stale)
- Known-acceptable-degradation inventory (explicit list, not prose)
- Verdict lifecycle: PENDING → RUNNING → PASS | FAIL | STALE_EVIDENCE
- CauseKind taxonomy: constrained enum for root-cause classification (Sub-AC 6a)
- scope_artifact_ids: controlled vocabulary references [Sub-AC 7a]; each entry
  must be a valid artifact reference string registered in ops/artifact_registry.py.
"""

from __future__ import annotations

import time
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Optional


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


class CauseKind(str, Enum):
    """
    Constrained taxonomy for classifying the root cause of a task failure
    or a known-acceptable-degradation item.  [Sub-AC 6a]

    Every RootCause record MUST carry exactly one CauseKind value.  Adding a
    new classification requires extending this enum AND updating the
    documentation table below.

    Valid classifications
    ─────────────────────────────────────────────────────────────────────────
    ARCHITECTURAL_ASSUMPTION
        The failure stems from a design-level choice or assumption that was
        made at architecture time and is not easily changed at runtime.
        Examples:
          - Single-node etcd in dev clusters (by design)
          - No HA for CF Tunnel in POD-mini (resource constraint accepted
            during scoping)

    CODE_DEFECT
        A defect in the implementation — incorrect logic, off-by-one, missing
        null-check, etc. — that should be fixed in a future sprint.
        Examples:
          - scalex sdi init exits 0 on partial failure (OPS-17)
          - kubeconfig merge drops context entries under race condition

    EXTERNAL_DEPENDENCY
        The failure is caused by an external system or service outside the
        ScaleX codebase (cloud API quota, upstream registry outage, etc.).
        Examples:
          - Cloudflare API rate-limit during tunnel provisioning
          - quay.io registry unavailable during kubespray image pull

    CONFIGURATION_ERROR
        An operator-supplied or environment-specific configuration value is
        wrong or missing.  The code is correct; the input is not.
        Examples:
          - KUBECONFIG not set in the environment
          - sdi-specs.yaml references a bridge that does not exist on the host

    KNOWN_LIMITATION
        A deliberate capability boundary of the current release scope.  Not a
        defect; not expected to be fixed within this iteration.
        Examples:
          - Metrics-server not deployed in sandbox cluster (out-of-scope P2)
          - No automated token rotation (deferred to P3)
    ─────────────────────────────────────────────────────────────────────────
    """

    ARCHITECTURAL_ASSUMPTION = "architectural_assumption"
    CODE_DEFECT = "code_defect"
    EXTERNAL_DEPENDENCY = "external_dependency"
    CONFIGURATION_ERROR = "configuration_error"
    KNOWN_LIMITATION = "known_limitation"


@dataclass
class RootCause:
    """
    A root-cause record attached to a DegradationItem or a FAIL verdict.

    Fields:
        cause_kind:   Constrained classification from the CauseKind taxonomy.
                      MUST be one of the CauseKind enum values; free-text
                      categorisation is explicitly rejected.
        description:  One-line human summary of the specific cause.
        ticket:       Optional tracking reference (e.g. "OPS-42", "GH-123").
        mitigation:   Optional short description of the current mitigation or
                      workaround in place.
    """

    cause_kind: CauseKind
    description: str
    ticket: Optional[str] = None
    mitigation: Optional[str] = None

    def __post_init__(self) -> None:
        if not isinstance(self.cause_kind, CauseKind):
            raise TypeError(
                f"cause_kind must be a CauseKind enum member, got {type(self.cause_kind).__name__!r}. "
                f"Valid values: {[c.value for c in CauseKind]}"
            )
        if not self.description.strip():
            raise ValueError("RootCause.description must not be empty")


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
    - root_cause: optional RootCause record carrying a cause_kind classification.
                  Sub-AC 6b requires every item in the canonical inventory to carry
                  a root_cause with a non-None cause_kind.  New items added without
                  root_cause are accepted (field defaults to None) but the
                  test_causekind_backfill suite will flag them.
    """
    id: str
    description: str
    affects_task_ids: list[str]
    ticket: str | None = None
    root_cause: Optional[RootCause] = None


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
        scope_artifact_ids:       Controlled-vocabulary artifact references [Sub-AC 7a].
                                  Each entry MUST be a valid artifact reference string
                                  of the form "<granularity>:<name>[:<aspect>]" registered
                                  in ops/artifact_registry.ARTIFACT_REGISTRY.
                                  Validated by validate() when non-empty.
                                  Empty list is accepted for backward compatibility
                                  but the test_artifact_registry suite will flag tasks
                                  that declare zero artifact refs.
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

    # Controlled-vocabulary artifact references [Sub-AC 7a]
    scope_artifact_ids: list[str] = field(default_factory=list)

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
        # Validate scope_artifact_ids against the controlled vocabulary [Sub-AC 7a]
        if self.scope_artifact_ids:
            from ops.artifact_registry import validate_artifact_refs, ArtifactRefError
            try:
                validate_artifact_refs(self.scope_artifact_ids)
            except ArtifactRefError as exc:
                raise ValueError(
                    f"Task {self.id!r}: invalid scope_artifact_ids — {exc}"
                ) from exc

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
