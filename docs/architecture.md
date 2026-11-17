# Architecture

> Stub. Expands as the code is written.

## Layers

How `domain`, `application`, `adapters`, `harness`, and `cli` fit together, and which way
dependencies may point. The rules themselves are in [AGENTS.md](../AGENTS.md#architecture-ddd--clean--hexagonal).

## Request flow

- **Edit phase:** the flow from a harness hook payload to a verdict.
- **Stop phase:** the flow from a session's change set to related tests and a verdict.

## Baseline

How fingerprints are taken, stored, and compared, so only new findings are reported.

## Failure handling

Timeouts, missing tools, and internal errors, and how each becomes a degraded, non-blocking
outcome.
