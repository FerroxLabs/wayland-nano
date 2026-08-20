# CI adoption for verify receipts

WP-3 supplies reviewable workflow consumers under `docs/verify/ci/` only. It
does not promote them into `.github/workflows/**`, change repository settings,
or declare its own check required. That ownership boundary prevents an agent
from self-approving the control it authored.

The receipt consumer pins `waylandnano@0.3.0`, fetches full Git history, and
examines the pull request's real `git diff --name-status` output. Added and
modified receipts are verified. Deleted or renamed receipts fail, closing the
empty-loop deletion hole. Unexpected statuses fail. No receipt changes passes
because nothing attested changed. Every nonzero verifier exit (including 2 and
6) fails the job; the detailed exit remains visible in the step log.

## Promotion sequence (owner only)

Stop at the WP-4 dependency. After WP-4's sealed mutants and mutation battery
land and pass review, the repository owner may:

1. Review the pinned version and both files under `docs/verify/ci/` against the
   released schema.
2. Promote the approved consumers into `.github/workflows/` without changing
   their A/M and D/R selection semantics.
3. Observe the promoted jobs succeed on a pull request containing the WP-4
   mutation battery.
4. In repository Settings -> Branches, protect `master`, enable **Require
   status checks to pass**, and select the promoted `verify-receipts` status.

Branch protection is an owner action, never an agent action. The dogfood
consumer remains dormant in the docs-owned location until that post-WP-4
promotion. This contract introduces no WP-5/WP-6, profiles, memory, MCP,
DeepSeek, or external-agent behavior.
