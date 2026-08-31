# Plan 02-09 Summary

The legacy Desktop ACP compatibility stack is gated through the shared Plan 02-08 producer at Desktop commit `3d77e37df`.

- Only explicit `waylandNanoBindingRef` resolution through an owner-injected composition seam can produce immutable Nano activation input; mutable conversation/backend/name/cwd fields never become authority.
- Final child-stdin `session/new` and `session/load` frames carry exact activation metadata; cancel/pause frames carry exact signed controls.
- Any resolved-Nano load/auth/drift/revocation failure is terminal with no unauthenticated fresh fallback. Non-Nano fallback and bytes remain unchanged.
- The exact verified binary identity token is consumed once immediately around the real `shell:false` generic spawn; missing/stale identity yields zero child.
- Production remains default nonpersistent until Plan 02-10 supplies explicit owner composition and immutable artifact expectations. No registry or self-hashing trust source was added.
- Focused final-wire tests: 8/8; typecheck, scoped lint/format, and `git diff --check`: passed.
- The older `AcpConnection` stack is covered for contract parity but is not misrepresented as the production manager path; Plan 02-15 covers the live AcpAgentV2/SDK chain.
