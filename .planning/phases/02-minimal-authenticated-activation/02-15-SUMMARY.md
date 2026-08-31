# Plan 02-15 Summary

The production Desktop AcpAgentV2/SDK stack is gated through the same owner-resolved Plan 02-08/02-09 producer at Desktop commit `ebc2d0616`.

- `OldAcpAgentConfig` carries resolved activation input separately from conversation-derived `agentId`; no mutable product field becomes Nano authority.
- `LegacyConnectorFactory` was the necessary one-file plan amendment because it exclusively owns the actual production spawn. It consumes the verified one-use binary token immediately at that spawn and passes the single producer into `ProcessAcpClient`.
- Actual child-stdin bytes after SDK serialization preserve create/load activation metadata and signed cancel/wire-pause controls. Local permission-timer pause remains separate.
- Missing binding or stale identity yields zero child. Nano auth/drift/revocation/load refusal is terminal with no history replay or fresh-session fallback; non-Nano fallback remains unchanged.
- ACP repository persistence remains disabled.
- Plan 15 integration: 6/6; binding/launcher filter: 2/2; Plan 15 plus existing factory/client compatibility: 17/17. Typecheck, scoped lint/format, and diff check passed.
- Existing Electron/platform-service baseline collection failures were not modified.
