## Conflict Detection Report

### BLOCKERS (0)

None.

### WARNINGS (0)

None.

### INFO (2)

[INFO] Auto-resolved: MEMORY-CONTRACT precedence over PROFILES-CONTRACT on persona semantics
  Found: `.planning/sources/PROFILES-CONTRACT.md` describes `system_prompt_file` as a narrow-only reference, while `.planning/sources/MEMORY-CONTRACT.md` §6.9 requires overlay-only persona content over an immutable kernel-owned core.
  Note: The explicit manifest precedence makes MEMORY-CONTRACT the winner; `.planning/sources/NANO-PROGRAM-PLAN.md` assigns the corresponding PROFILES-CONTRACT v1.1 amendment to P-PROF.

[INFO] Auto-resolved: finalized owner call narrows P-BOT-5b resume wording
  Found: `.planning/sources/NANO-PROGRAM-PLAN.md` P-BOT-5b says the package builds both continuity paths and owner call Q2 picks the default, while the later decided Q2 section makes memory-primary the sole verified default and transcript replay only an audit/fallback path.
  Note: The later explicit owner decision in the same source governs the synthesized P-BOT-5b requirement.
