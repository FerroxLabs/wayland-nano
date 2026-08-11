> Adopted from Track A (nano/docs/adr/g0-windows-provisioning-boundary.md, commit dd856312f) during the Track A salvage audit (shared/reviews/tracka-comparison.md). Verbatim below; the boundary analysis applies to the Wayland Nano provisioning flow unchanged.

# G0 Windows provisioning boundary

Status: accepted for the inherited G0 baseline on 2026-08-10.

## Decision

Windows sandbox provisioning must run during a quiescent same-user window. The
G0 baseline does not claim resistance to a separate same-user process creating
an NTFS hard link concurrently with ACL mutation. Provisioning fails closed for
hard links present during its stable-tree preflight and rolls back partial ACL
changes on detected failure.

The same G0 precondition excludes concurrent setup helpers from other app
instances, Terminal Services sessions, or users. A session-local/default-DACL
named mutex is not accepted as proof of machine-global serialization: sandbox
accounts and network policy are machine-global, and a robust lock requires the
installed privileged broker planned for P2. G0 instead requires a parent-created
per-home in-progress sentinel, bounded helper waits, cooperative cancellation
during ACL traversal, and durable fail-closed readiness after any unconfirmed
helper exit. These controls bound inherited setup failures; they do not claim
hostile-concurrency containment.

The built-in shell remains disabled outside the test harness. Before any
user-facing shell is enabled, P2 must prove a kernel-enforced containment or an
isolated-workspace design that closes concurrent same-user filesystem races.
An ACL rescan or final link-count check is not sufficient evidence.

## Evidence

Native testing proved that `CreateHardLink` succeeds while the source file is
held with a retained handle that excludes both write and delete sharing. A
final link-count scan therefore has an unavoidable check-to-return race and
cannot honestly provide the required concurrent-attacker guarantee.

Independent review also required exact DACL rollback across all allow and deny
roots, explicit propagation of rollback failures, full ACL allocation snapshots,
protected-reparse handling without target traversal, and handle-bound identity
checks. Those requirements remain blocking for the transactional correction.

## Consequences

- G0 can validate the inherited substrate without pretending ACLs provide a
  kernel isolation guarantee they do not provide.
- Setup diagnostics and evidence must state the quiescent precondition.
- Cancellation is best-effort outside ACL traversal; identity provisioning,
  firewall calls, directory materialization, and blocking Win32 calls may require
  forced termination. In that case the home remains incomplete/tainted until a
  later full audited repair succeeds.
- G0 evidence cannot be reused as P2 production-containment evidence.
- P2 may select a kernel policy, brokered/isolated workspace, or another design,
  but must prove the same-user race is closed before shell enablement.

The non-binding P2-B6 source audit and recommended isolated-workspace design are
documented in [`../spikes/windows-hardlink-containment.md`](../spikes/windows-hardlink-containment.md).
That spike does not alter this ADR's accepted G0 boundary or its quiescent
provisioning precondition.
