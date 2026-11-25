# Orchestrator

Own workflow state transitions, scheduling, event ingestion, and restart
reconciliation. The daemon continues operating when the GUI disconnects.

Background loops (one timer each, all `unref`-ed and stopped on shutdown):

- `watchers.ts` — file-change watcher per watched project (1 s tick).
- `nudger.ts` — auto-kick for stalled task CEOs (60 s tick, `KBOTS_NUDGE`).
- `maintenance.ts` — periodic `reconcileAgents()`, the finished-terminal
  reaper, the orphan-terminal sweep, dead-CEO recovery, and stale-task
  continuation (60 s tick, `KBOTS_REAP_INTERVAL_MS`). The sweep kills live
  terminals carrying a kbots bot label (`kbotsBotId`/`kbotsRole`) that no agent
  row references, after two consecutive sightings so a spawn in progress is safe;
  `KBOTS_ORPHAN_REAP=off` disables it. The reaper stops CEO terminals whose
  task already reached `done`/`failed`/`canceled` and has been quiet for
  `KBOTS_REAP_GRACE_MS`; it reuses `stopAgentCore`, so the OpenCode session and
  its history are kept — kbots never deletes sessions. Recovery resumes a
  crashed CEO inside `KBOTS_RECOVER_WINDOW_MS`; stale-task continuation does
  the same for older tasks so they cannot stay in `researching` forever, using
  the same budget/cooldown/cap. Both resume through `resumeAgentCore`, which
  records terminal mode so the continuation prompt is deliverable.
  `KBOTS_REAP=off` skips the reaper,
  `KBOTS_STALE_CONTINUE=off` skips stale continuation, and `KBOTS_RECOVER=off`
  disables both resumes; the reconcile tick always runs.

Module ownership: `dispatch.ts` (task → CEO session), `tasks.ts` (task handlers),
`delivery.ts` (queued messages), `approvals.ts`, `events.ts`, `projects.ts`,
`system.ts`, `router.ts` (+ self-check), `nudger.ts`, `watchers.ts`,
`maintenance.ts` (+ self-check for the orphan-selection rule).
