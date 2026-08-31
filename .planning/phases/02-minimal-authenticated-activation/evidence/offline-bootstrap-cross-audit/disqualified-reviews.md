# Disqualified provider attempts

Candidate audit identity before High-finding changes: `81d64b10d846c0c4e08e0f7afa9b8d0068049bbd62bc8994ca77bf509122e24f`, 18964 bytes.

## Moonshot Kimi K3 — disqualified after three strikes

- CLI: Kimi Code `0.38.0`
- requested model: `k3` (configured default `kimi-code/k3`)
- provider configuration: `managed:kimi-code`, OAuth; `kimi doctor` reported valid configuration
- status: crashed; never produced review content; does not count
- attempt 1 (prior continuation): candidate/options prompt crashed with the lifecycle exception below
- attempt 2: empty skills directory, empty working directory, explicit `-m k3`; same exception
- attempt 3 isolated proof: empty skills directory, empty working directory, prompt exactly `Reply only OK.`; same exception

Verbatim terminal exception common to the final two attempts:

```text
Error: Agent event 'agent.activity.updated' has no active lifecycle context
    at EventBusService.publish (...main.cjs:255710:112)
    at AgentEventBusView.publish (...main.cjs:255760:13)
    at EventDispatcherService.executeEvent (...main.cjs:256279:56)
    at EventDispatcherService.runDispatch (...main.cjs:256224:9)
    at EventDispatcherService.dispatch (...main.cjs:256194:10)
    at AgentActivityView.publish (...main.cjs:273146:20)
    at AgentActivityView.dispose (...main.cjs:273006:9)
Node.js v24.15.0
```

Disposition: client-wide lifecycle failure proven independently of skills, repository, candidate, and prompt. No fourth attempt and no dependency upgrade.

## Anthropic Claude Fable — disqualified as unavailable

- requested model: `fable`, effort `max`, read-only tool permission
- status: process remained responsive but emitted no review output for more than nine minutes; bounded call was interrupted once and not retried
- review content: none; does not count

Disposition: provider call unavailable for the exact candidate. A prior options-stage Fable review was useful research but was not bound to these candidate bytes and therefore is not counted.

## NVIDIA Nemotron 3 Ultra — disqualified on provider error

- runner-selected model: `opencode/nemotron-3-ultra-free`
- status: exact-candidate review failed before findings; does not count
- verbatim error:

```text
Error: "Streaming response failed: [502] Upstream error from Nvidia: Service temporarily overloaded"
```

Disposition: provider-side overload; no retry was attempted.

## Google Gemini — disqualified before this audit round

- installed client authentication/tier probe returned `IneligibleTierError`
- status: unavailable; no candidate review; does not count

## Counting reviews

Only the completed Xiaomi MiMo V2.5 and inclusionAI Ling 3.0 Flash Fin sessions count. Both were invoked through the pure OpenCode runner, consumed and verified the exact candidate, produced substantive findings, and completed the one permitted changed-High re-audit. The initial MiMo output contains a model-generated `Reviewer: OpenAI o3` line inconsistent with the immutable runner session metadata (`provider_id: opencode`, `model_id: mimo-v2.5-free`); that self-label is treated as an output anomaly, not provenance. The runner session metadata and Xiaomi's official MiMo model identity are authoritative.
