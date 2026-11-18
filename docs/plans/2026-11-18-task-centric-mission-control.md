# Task-centric Mission Control — Design & Roadmap

Date: 2026-11-18 · Status: agreed direction, implementation started same day.
Supersedes the bot-centric interaction model. First-class principle:

**You manage goals → kbots manages tasks → the CEO manages coding agents → `hc` manages terminals.**

On Moshi (or any terminal) you see *work you are doing*, not a list of CEO
sessions. One kbots TUI can sit in front of dozens of CEO sessions and
hundreds of workers.

## What exists today (verified in-tree, 2026-11-18)

- `bun run tsc --noEmit` clean baseline; daemon v1 serves JSON-RPC 2.0 over a
  Unix socket (`docs/kbots-ipc.md`), SQLite WAL at `${XDG_STATE_HOME}/kbots/kbots.db`,
  `SCHEMA_VERSION = 3` (adds `agents`, `watchers`).
- Managed agent lifecycle exists: `agents.start/show/stop/prompt/interrupt`,
  exact session binding (SDK session first, then `hc` terminal or headless),
  boot-time `reconcileAgents()`, prompts refused unless the agent is idle,
  one active `git`/`ops` agent per project (partial unique index
  `idx_agents_active_project_role`).
- `tasks.create` records a task (`status: "planning"`) and logs an event.
  Nothing dispatches it anywhere. Tasks are not wired to OpenCode.
- `events.list`, `approvals.list`, `agents.list` (read-only, incl. `managed`,
  `liveTerminals`) are implemented.
- Ratatui TUI renders demo data only; no socket connection. Slint GUI does
  connect. CLI does connect.

## Decisions pinned by this design

1. **Keep the stack.** Bun + Effect v4, SQLite WAL, Rust + Ratatui, Slint GUI
   untouched, OpenCode SDK for conversation control, `hc` for terminals.
   No Redis, no Kubernetes, no web backend.
2. **Task-centric UI, session-aware backend.** The TUI never asks you to pick
   a session. Every task owns at most one CEO; every prompt you type with an
   `@N` prefix routes to task N's CEO.
3. **One request/response per socket connection stays.** `events.subscribe`
   (streaming) is deferred; the TUI polls on a timer plus after actions.
4. **Reuse the `agents` registry and `events` log.** No duplicate bookkeeping.
   `task_agents` links tasks to registry rows. No separate `task_events`.
5. **Deterministic policy in TypeScript.** Bots never grant themselves
   capabilities; concurrency caps, git/ops serialization, and approval gates
   live in the daemon.

## Honest limitations (do not paper over)

- **Parallel CEOs share a working tree.** Until worktree isolation lands
  (P3), two CEOs on the same project can edit the same files. Mitigations for
  now: per-project dispatch concurrency cap (default 4), one active task per
  *project tree* for code-writing roles unless the user accepts the risk, and
  clear warnings in task output. GIT/OPS stay single per project, always.
- **The daemon cannot see inside a running CEO run.** Automatic "CEO asked a
  question" detection needs OpenCode session events. Until then the Inbox
  surfaces: pending approvals, failed/blocked tasks, and the queued-message
  backlog. `tasks.message` is the explicit follow-up path.
- **Status transitions after dispatch are advisory.** `queued → researching`
  is set at dispatch; `done/failed` come from explicit marks, approvals, or
  reconcile findings, not from peeking at the model.
- `approvals.approve` still records the decision only; nothing auto-executes.

## Data model — schema v4 (migration from v3, `user_version` 3→4)

Rebuild `tasks` (SQLite cannot alter a CHECK) preserving ids, mapping legacy
statuses: `planning → queued`, `in_progress → running`, `completed → done`,
`canceled` unchanged. New enum:

```
queued | researching | planning | delegating | running | review | blocked | done | failed | canceled
```

New columns on `tasks`: `goal TEXT` (the original ask, verbatim),
`detail TEXT` (optional expanded brief).

New tables:

```sql
CREATE TABLE task_agents(
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
  role TEXT NOT NULL CHECK(role IN ('ceo','worker','git','ops')),
  created_at TEXT NOT NULL,
  PRIMARY KEY (task_id, agent_id)
);
CREATE UNIQUE INDEX idx_task_agents_ceo ON task_agents(task_id) WHERE role = 'ceo';
CREATE INDEX idx_task_agents_agent ON task_agents(agent_id);

CREATE TABLE task_messages(
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  ts TEXT NOT NULL,
  direction TEXT NOT NULL CHECK(direction IN ('user','system')),
  text TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('queued','delivered','canceled')),
  delivered_at TEXT
);
CREATE INDEX idx_task_messages_task ON task_messages(task_id, status);

CREATE TABLE task_briefs(
  task_id TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
  summary TEXT,
  decisions TEXT,      -- newline-separated, newest last
  blockers TEXT,       -- newline-separated; empty when none
  next_step TEXT,
  updated_at TEXT NOT NULL
);
```

Registry change: `idx_agents_active_project_role` is narrowed to
`role != 'ceo'` (many active CEOs per project allowed; git/ops unchanged).
CEOs are bound per task through `task_agents`; `idx_task_agents_ceo`
guarantees at most one CEO per task, which is also the dispatch
idempotency key.

`events.kind` gains: `task.dispatched`, `task.dispatch_failed`, `task.deferred`,
`task.message_queued`, `task.message_delivered`, `task.status`, `brief.updated`.
Existing kinds unchanged. Events carry `data.taskId` by convention.

## CEO Session Pool

All CEOs for a project share `.agents/bot/ceo/` (role config, instructions,
model). No `ceo-1`, `ceo-2` directories. Each dispatched task gets its own
OpenCode session with metadata `{kbotsBotId, kbotsRole: "ceo", kbotsProjectId,
kbotsTaskId}`; mode `headless` by default, `terminal` on demand.

Dispatch (in one SQLite transaction + compensation, mirroring the managed
agent pattern):

1. Validate task exists, status `queued`, project known.
2. Check per-project active-CEO cap (default 4; over cap → stay `queued`,
   event `task.deferred`, retried by reconcile/next action).
3. Create SDK session in the project's ceo directory with the metadata above.
4. Insert `agents` row (role `ceo`, status `starting→running`) and
   `task_agents` link. Commit.
5. Queue first prompt (delivery rules below): task title + goal + constraints;
   status → `researching`; event `task.dispatched`.

Idempotency: `idx_task_agents_ceo` + transaction; a re-run of `tasks.create`
dispatch or a daemon restart mid-dispatch converges to one CEO per task.
Boot-time reconcile extends `reconcileAgents()` to task CEOs and to flushing
queued `task_messages` to idle CEOs.

Follow-up routing (`tasks.message`): resolve task → CEO agent. Idle →
deliver via the same path as `agents.prompt` (never forced). Busy/blocked →
insert into `task_messages` (`status: queued`). On delivery set
`delivered` and log `task.message_delivered`.

Completion: CEO sessions are kept after finishing (review/continue). Nothing
deletes a session. `--fresh` is not a concept for task CEOs; cancel the task
instead.

## RPC surface (v2 additions; v1 unchanged, one request/response per connection)

| Method | Params | Result |
| --- | --- | --- |
| `tasks.create` | `title`, `goal?`, `project?`, `autoDispatch?` (default `true`) | `{taskId, status, ceoAgentId?, dispatchError?}` |
| `tasks.list` | `project?`, `status?` (NEW filter) | `{items: [...]}` |
| `tasks.get` | `taskId` | task object + `ceo` link + queued message count |
| `tasks.message` | `taskId`, `text` | `{taskId, outcome: "delivered" \| "queued", messageId?}` |
| `tasks.messages` | `taskId` | `{items: [...]}` queued/delivered backlog |
| `tasks.brief` | `taskId?` | no arg → global overview: `{generatedAt, counts, needsAction: [...], running: [...], done: [...]}`; with arg → `{taskId, title, status, goal, summary, decisions, blockers, nextStep, ceo, workers, pendingMessages}` |
| `tasks.inbox` | `{}` | `{items: [{kind: "approval" \| "blocked" \| "failed" \| "queued" \| "review", taskId?, ...}]}` — merge of pending approvals, blocked/failed/review tasks, and queued user messages |
| `tasks.update` | `taskId`, `status` (allow: `review`, `blocked`, `done`, `failed`, `canceled`), `note?` | `{taskId, status}` — explicit operator control; appends to `task_briefs.decisions` when `note` given |
| `tasks.markWorked` | *(not added)* | deferred |

`tasks.brief` is composed from the DB; it never round-trips to a CEO.
`/brief` in the TUI is `tasks.brief` with no arg.

## TUI — mobile-first, one column ≤ 112 cols (already the compact path)

P0 (this wave): connect to the socket at startup (same discovery order as CLI:
`--socket` not available in TUI → `KBOTS_SOCKET` → `$XDG_RUNTIME_DIR/kbots.sock` →
`$HOME/.local/run/kbots.sock`); render Overview (counts), Tasks (real rows,
status-colored), Agents (managed registry + `herdr` tags), Approvals, Activity
(`events.list`); loading / error / disconnected banner; reconnect on action
failure; poll every 5 s only when visible; no busy-redraw (render on
input/tick/data change only).

P2 (this wave): composer line — plain text creates a task (default project =
only registered project, else asks), `@N text` → `tasks.message`, slash commands:
`/brief` (global brief overlay), `/inbox` (needs-action list, Enter to act),
`/tasks`, `/dispatch N` (retry dispatch for a queued task), `/done N`.
Inbox badge on the header (`Inbox 2`) when items need action.

Touch/Moshi: keep existing `Ctrl+B` chords and wheel navigation; no synthetic
swipe events. Exit restores the terminal (RAII guard already in place).

## Phasing

- **P0 — TUI live on daemon.** Real data, error states, reconnect. (This wave.)
- **P1 — Auto-dispatch + CEO pool + Task Memory.** Schema v4, dispatcher,
  `tasks.message`, idempotent restart. (This wave.)
- **P2 — Inbox + Briefing.** `tasks.brief`, `tasks.inbox`, `/brief`, composer. (This wave.)
- **P3 — hc lifecycle + isolation + gates.** Worker terminals with labels
  `kbots.task / kbots.role / kbots.owner / kbots.project`; per-task git
  worktree isolation; concurrency caps; serialized GIT/OPS queue; approval
  gates before destructive ops; `events.subscribe` streaming. (Roadmap only.)

## Implementation waves (conflict-free file ownership)

- **Agent TUI** → `tui/**` only. P0 + P2 UI against this contract; tolerate
  missing methods with a clear in-app error until the daemon ships them.
- **Agent BACKEND** → `src/backend/**` only. Schema v4, repositories,
  dispatcher, new RPC methods, reconcile extension. No TUI, CLI, or docs edits.
- **Agent DOCS** → `docs/**` only. Sync `docs/kbots-ipc.md` + status tables in
  `ARCHITECTURE.md`/`README.md` with the v2 surface.
- Excluded this wave: `cli/` (automation subcommands for new methods), `gui/`.

## Verification

`bun run tsc --noEmit`; `cargo build --release --manifest-path tui/control-center/Cargo.toml`;
smoke dispatch against a scratch project under `/tmp/opencode`; restart-idempotency
check (kill daemon mid-dispatch → single CEO); `./install.sh` before handoff.

## Open questions

- Per-task git worktree UX: auto-create on `delegating`, or opt-in flag?
- Should `tasks.update(done)` require an approval when the task ran GIT ops?
- Default model per project for CEO sessions (manifest `model` already exists — honor it)?

---

## Revision 2026-11-18 (evening) — Hybrid Dispatcher: Deterministic Scheduler + AI Router

Agreed design layer above the deterministic dispatcher. Principles:

1. **Deterministic-first.** Explicit task id / project / `@N` in the TUI never
   reaches a model — direct routing.
2. **AI proposes, scheduler decides.** The AI Router only answers *"which task
   does this belong to, and where does it go?"* It returns a structured
   `RouteDecision`; the scheduler validates it (project match, task exists,
   not terminal, concurrency caps) and executes. The router never calls
   `agents.start` / `terminal.run`. The router is not a super-CEO and owns
   no goal execution.
3. **Task memory is authoritative.** Task↔CEO↔worker↔terminal links live in
   SQLite only; never in the router's context window.

Target flow:

```text
User input (TUI composer)
    │
    ├── explicit route (@N, /cmd, taskId, single-project) ──▶ Schedule r (validation + execute)
    │
    └── natural language ──▶ AI Router (bounded, single pass, no tool loop)
                                │  input: user text + selected project +
                                │  selected task (if any) + candidate tasks
                                │  (id, title, status, one-line brief) from THAT project only
                                ▼
                          RouteDecision (schema-validated)
                                │
                          Validation ── valid ──▶ Scheduler
                                │
                                ambiguous / invalid
                                ▼
                          ask_user (never silently create a CEO / wrong conversation)
```

```ts
type RouteDecision =
  | { action: "create_task";   projectId: string; title: string; intent: string }
  | { action: "continue_task"; projectId: string; taskId: string; message: string }
  | { action: "ask_user";      question: string; candidateTaskIds: string[] }
```

**Router constraints:** bounded inference (single pass; at most one re-classify
with extra metadata fetched deterministically), schema validation of output,
candidate list filtered to the selected project, `ask_user` on any ambiguity.
OpenCode SDK is the model path (no new agent framework); SDK event streaming
may feed status updates later.

**Scheduler is the only executor.** It re-checks caps/state before acting,
persists intended actions to SQLite *before* execution (durable outbox +
operation id per spawn), then uses Effect Queue/structured concurrency for
in-memory handoff. RAM queues are not durable state. After a crash, the
reconciler compares DB ↔ OpenCode session ↔ `hc` terminal before re-executing
— no blind re-spawn (duplicate-execution avoidance via operation id +
`task_agents` unique link).

**Concurrency model:** many saved CEOs per project, but capped *simultaneous
inference* per project (env/config, default 4) and separate quotas for
worker builds/tests. GIT/OPS remain one active per project.

**Correct-path guarantees (must hold):**

| Situation | Behavior |
| --- | --- |
| New request | Router proposes `create_task`; scheduler provisions CEO |
| Follow-up on running task | Route to owning CEO; prompt or queue |
| CEO done, wants changes | Resume task CEO session if valid, else fresh session with brief + decision history |
| Daemon dies mid-spawn | Reconciler converges DB ↔ OpenCode ↔ hc; no duplicate spawn |

**Re-phased roadmap (supersedes §Phasing above):**

- **P0 — TUI live on daemon.** (Wave 1, in flight.)
- **P1 — Deterministic dispatcher + CEO pool + Task Memory.** (Wave 1, in flight — no AI anywhere; `@N`/`tasks.message` only.)
- **P2 — AI Router.** `orchestrator/router.ts` (bounded inference + RouteDecision validation), `orchestrator/dispatcher.ts` (route validation + execution split made explicit), `orchestrator/scheduler.ts` (persisted outbox, concurrency quotas), `orchestrator/reconciler.ts` (restart convergence extended to outbox), TUI composer NL path with explicit confirmation when `ask_user`. Deterministic explicit routing stays the default; router is additive, never required for the workflow to function.
- **P3 — Unified Inbox + persistent briefs + lifecycle recovery + Herdr events** (inbox/brief UI landed in wave 1; worker label lifecycle, git worktree isolation, serialized GIT/OPS, approval gates, `events.subscribe` here).

**Module map (new files, none exist yet):** `orchestrator/router.ts`,
`orchestrator/dispatcher.ts`, `orchestrator/scheduler.ts`,
`orchestrator/reconciler.ts`, `bots/ceo-pool.ts`,
`workflows/task-execution.ts`, extended `adapters/herdr.ts`, and `db/`
outbox + routing-state migration. Wave 1 ships `dispatch.ts` (deterministic);
the split into dispatcher/scheduler lands with P2 so names converge.

Guiding line: **User owns goals → Dispatcher owns routing → Scheduler owns
execution → CEO owns technical decisions → Workers own implementation.**
