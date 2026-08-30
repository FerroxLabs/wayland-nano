# Activation operator lifecycle

Persistent activation is default-off. Commands below use owner-only key-reference files; they never accept private key bytes or secret environment variables. Every state change is authenticated, journaled, bound to the exact source/lock/executable triple, and emits a canonical signed receipt.

## Receipt signer rotation

`wayland-nano admin receipt-signer rotate --request-file C:\ProgramData\WaylandNano\requests\receipt-rotate.jcs --key-reference-file C:\ProgramData\WaylandNano\keys\receipt-signer.keyref`

Expected: an `activation.admin-receipt/v1` rotation receipt naming old/new key IDs, overlap expiry, epochs, journal position, and exact artifact identity. Keep both public verifier keys for the bounded overlap; never copy signer material into Nano home.

## Verifier rotation and distribution

`wayland-nano receipt verifier export --output C:\ProgramData\WaylandNano\public\receipt-verifiers.jcs`

Expected: a public, offline-verifiable evidence bundle containing active/overlap key IDs, public keys, validity windows, revocations, and bundle digest. Distribute that immutable bundle beside receipts; verifiers reject absent or expired evidence.

## Retention

`wayland-nano receipt retain --request-file C:\ProgramData\WaylandNano\requests\retention.jcs`

Expected: a retention-policy receipt binding the retained receipt/effect ranges and verifier evidence. Retention never deletes unresolved `unknown_outcome`, revocation, rollback, or tombstone evidence.

## Revocation

`wayland-nano admin receipt-signer revoke --request-file C:\ProgramData\WaylandNano\requests\receipt-revoke.jcs`

Expected: a durable revocation receipt. Revocation takes precedence over overlap, cached admission, enablement, resume, and artifact compatibility; affected activation becomes disabled before acknowledgment.

## Compromise recovery

`wayland-nano admin recover --request-file C:\ProgramData\WaylandNano\requests\offline-recovery.jcs --key-reference-file C:\ProgramData\WaylandNano\keys\recovery-root.keyref`

Expected: recovery receipts for containment, replacement epochs, verifier distribution, reconciliation of every pending effect, and fresh exact-artifact enablement. Missing recovery authority leaves activation disabled and requires a new Nano home; it never inherits old authority.

## Rollback and default-off

`wayland-nano activation disable --request-file C:\ProgramData\WaylandNano\requests\disable-artifact.jcs`

Expected: a journaled disable receipt before rollback. A missing, stale, mismatched, expired, revoked, partially written, or rolled-back enablement anchor keeps activation disabled; an older executable never falls back to unauthenticated persistence.

## Platform key references

`wayland-nano admin key-reference inspect --key-reference-file C:\ProgramData\WaylandNano\keys\admin-root.keyref`

Expected: a metadata-only custody report. Windows requires a canonical local non-reparse file with owner-only DACL; Unix requires a regular non-symlink file owned by effective UID with mode 0600 and safe parents. The command never prints key bytes.

## Offline verification

`wayland-nano receipt verify --receipt-file C:\ProgramData\WaylandNano\receipts\activation.jcs --verifier-bundle C:\ProgramData\WaylandNano\public\receipt-verifiers.jcs --offline`

Expected: a typed verification result binding canonical bytes, signature, revocation evidence, assertion and policy digests, journal positions, epochs, result/effect state, source commit, Cargo.lock digest, and executable digest. `unknown_outcome` requires operator reconciliation and is never redispatched automatically.

## No-secret rules

`wayland-nano admin key-reference inspect --key-reference-file C:\ProgramData\WaylandNano\keys\receipt-signer.keyref`

Expected: a metadata-only result with no private value. Never place private keys in argv, environment variables, project files, fixtures, journals, receipts, logs, clipboard examples, or shell interpolation. Supply only an owner-controlled key-reference path.
