# Orchestrator

Own workflow state transitions, scheduling, event ingestion, and restart
reconciliation. The daemon continues operating when the GUI disconnects.

Background loops (one timer each, all `unref`-ed and stopped on shutdown):

- `watchers.ts` — file-change watcher per watched project (1 s tick).
- `nudger.ts` — auto-kick for stalled task CEOs (60 s tick, `KBOTS_NUDGE`).
- `maintenance.ts` — periodic `reconcileAgents()` plus the finished-terminal
  reaper (60 s tick, `KBOTS_REAP_INTERVAL_MS`). The reaper stops CEO terminals
  whose task already reached `done`/`failed`/`canceled` and has been quiet for
  `KBOTS_REAP_GRACE_MS`. It reuses `stopAgentCore`, so the OpenCode session and
  its history are kept — kbots never deletes sessions. `KBOTS_REAP=off` skips
  the reaper step while leaving the reconcile tick in place.

Module ownership: `dispatch.ts` (task → CEO session), `tasks.ts` (task handlers),
`delivery.ts` (queued messages), `approvals.ts`, `events.ts`, `projects.ts`,
`system.ts`, `router.ts` (+ self-check), `nudger.ts`, `watchers.ts`,
`maintenance.ts`.
