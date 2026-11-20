# Bot roles

Define CEO planning/review, serialized GIT operations, serialized OPS work, and
coding-worker contracts here. Deterministic permissions and gates remain in
TypeScript; bots do not grant themselves additional capabilities.

Modules:

- `agents.ts` — managed-agent lifecycle (`agents.*` handlers), `reconcileAgents`,
  `stopAgentCore`, `resumeAgentCore`.
- `health.ts` — the derived bot report (activity + health + bound task + session
  model) used by `agents.list`, `agents.show`, `system.status`, the CLI and the
  TUI. Pure and self-checked by `health.selfcheck.ts`
  (`bun run src/backend/bots/health.selfcheck.ts`).
- `opencode.ts` — OpenCode client factory, and session model health
  (`availableModelKeys`, `sessionModelRef`, `pickFallbackModel`,
  `switchSessionModel`). See `docs/opencode-control.md`.
