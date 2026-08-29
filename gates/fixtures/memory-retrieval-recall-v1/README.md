# memory-retrieval-recall-v1

This is the independently reviewed acceptance instrument required by `MEMORY-CONTRACT.md` §11.1. It contains exactly 50 facts, 10 decisions, and 20 human-readable labeled queries.

Every semantic row has an identical counterpart re-attributed across a project boundary, an agent boundary, or both. The duplicate is intentional: omission of either pre-retrieval filter leaves an indistinguishable wrong-partition result competing with a labeled row. IDs and partition fields are the only differences within each pair.

Run the structural and fixture-honesty checks with:

```text
node gates/validate-memory-recall-fixture.cjs
```

This fixture does not assert retrieval quality by itself. The implementation PR consumes this reviewed instrument to measure recall@10 and leakage. Labels must not be changed in response to retrieval output; evidence that honest labels cannot meet the contract triggers contract-amendment review.
