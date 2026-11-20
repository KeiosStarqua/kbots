# kbots local IPC contract

The Bun daemon owns workflow state and side effects. GUI, TUI, and CLI are
clients. The initial transport contract is one newline-delimited JSON-RPC 2.0
request and response per Unix domain socket connection. The daemon is
implemented (v1 minimal) and runs as the `kbots-daemon` systemd user service
after `./install.sh` (opt out with `KBOTS_SKIP_DAEMON_UNIT=1`), or manually
with `bun run daemon` (equivalently `bun run src/backend/daemon.ts`) without
systemd.

The daemon listens on two Unix domain sockets by default — the primary plus a
sandbox-reachable fallback — and clients try them in the discovery order:

1. `--socket PATH` (CLI/GUI) / `KBOTS_SOCKET` (all clients)
2. `$XDG_RUNTIME_DIR/kbots.sock`
3. `$HOME/.local/run/kbots.sock`

An explicit `--socket`/`KBOTS_SOCKET` means exactly that one path. Otherwise a
client falls back to the next candidate when a connect fails, not only when a
socket is missing — a sandbox (Landlock/seccomp, container) may deny `connect()`
to `/run/user/*` with `EPERM` while `$HOME` works.

The daemon binds the primary socket from the discovery order. When that lands
in `$XDG_RUNTIME_DIR` and neither `--socket` nor `KBOTS_SOCKET` is set, it also
binds a secondary listener on `$HOME/.local/run/kbots.sock` (mode `0600`,
unlinked on SIGINT/SIGTERM, skipped without stealing if another daemon already
serves it, and never fatal to the daemon if it fails). Sandboxed agents that
cannot reach `/run/user/*` connect through the fallback path instead.

Connections beyond `KBOTS_MAX_CONNECTIONS` (default `256`) are rejected with a
JSON-RPC `-32000` `server busy` error instead of queueing unboundedly. SQLite
runs in WAL mode with `synchronous = NORMAL` and a `busy_timeout` of 5000 ms,
so many concurrent readers never block the writer longer than that window.

Example request:

```json
{"jsonrpc":"2.0","id":1,"method":"tasks.create","params":{"title":"Implement authentication","project":"api-service"}}
```

Example response:

```json
{"jsonrpc":"2.0","id":1,"result":{"taskId":"TASK-104","status":"planning"}}
```

Project registration request:

```json
{"jsonrpc":"2.0","id":1,"method":"projects.register","params":{"path":"/home/keios/repos/mein/kt-system","name":"kt-system","botRoles":["ceo","git","ops"]}}
```

Initial CLI methods:

| CLI command | RPC method | Parameters |
| --- | --- | --- |
| `status` | `system.status` | `{}` |
| `tasks add TITLE [--project ID]` | `tasks.create` | `title`, optional `project` |
| `tasks list [--project ID]` | `tasks.list` | optional `project` |
| `tasks show ID` | `tasks.get` | `taskId` |
| `tasks cancel ID` | `tasks.cancel` | `taskId` |
| `agents list [--project REF]` | `agents.list` | `{}` or `{project}` — scopes items/managed/liveTerminals to one project (`filteredBy` set; `registeredProjects` remains the global count) |
| `project watch [--project REF] [--off] [--interval N] [--debounce N] [--cooldown N] [--max-paths N]` | `watchers.set` | Enable/configure the daemon file-change watcher |
| `project watch` (no `--project`) | `watchers.list` | `{}` — list configured watchers |
| `agents start ROLE [--project REF] [--fresh]` | `agents.start` | `role`, optional `project`, optional `fresh` — agents are always hc terminals (headless retired) |
| `agents show ID` | `agents.show` | `botId` |
| `agents logs ID [--limit N]` | `agents.logs` | `botId`, optional `limit` (1-200) — session messages (newest first) + live terminal screen |
| `agents stop ID` | `agents.stop` | `botId` |
| `agents prompt ID TEXT` | `agents.prompt` | `botId`, `text` |
| `agents interrupt ID` | `agents.interrupt` | `botId` |
| `project add [PATH] [--name NAME]` | `projects.register` | canonical `path`, display `name`, bot roles |
| `project configure [PATH] [--overwrite]` | local CLI action | generate per-bot OpenCode configs from the manifest |
| `sessions start [--directory PATH]` | local OpenCode client action | create or reuse idle sessions for enabled roles |
| `project list` | `projects.list` | `{}` |
| `project show ID` | `projects.get` | `projectId` |
| `approve ID` | `approvals.approve` | `approvalId` |

Result shapes for the read and create methods:

| Method | Result |
| --- | --- |
| `tasks.create` | `{taskId, status}` (`status` is `"planning"`) |
| `tasks.list` | `{items: [...]}` |
| `tasks.get` | task object with `taskId` |
| `tasks.cancel` | `{taskId, status}` |
| `approvals.list` | `{items: [...]}` |
| `events.list` | `{items: [...]}` (newest first; `limit` defaults to 100, max 500) |
| `agents.list` | `{items: [{projectId?, projectPath, role, enabled, sessionID\|null, directory, registered, herdr?}], liveTerminals: [{address, agent?, name?, cwd, state?, status?, turn?, projectId?, role?}], filteredOut, registeredProjects, generatedAt, herdrUnavailable?, managed: [ManagedAgent]}` — `herdr` is attached on an exact role-directory match; `liveTerminals` is the read-only `hc ls` view, filtered to terminals inside registered project paths (`filteredOut` counts those hidden); `managed` lists every agent in the managed registry with its probed live status |
| `agents.start` | ManagedAgent object: `{id, projectId, projectName?, role, mode, directory, status, sessionId, hcAddress, name, createdAt, updatedAt, stoppedAt, lastError?, live}` — idempotent while running; resumes the stopped agent's session |
| `agents.show` | ManagedAgent object for one bot |
| `agents.logs` | `{agentId, projectId, role, mode, status, sessionId, hcAddress, live, screen, messages: [{type, outcome, createdAt, text}]}` — read-only debug view: OpenCode session history plus the hc terminal screen (last 120 lines) for terminal agents |
| `agents.stop` | `{id, projectId, role, mode, status, sessionId, stoppedAt, …}` — terminal is killed; the OpenCode session and history are kept |
| `agents.prompt` | `{botId, mode, accepted, turn?}` — refuses unless the agent is idle (never `force`s past a blocked program) |
| `agents.interrupt` | `{botId, mode, key?, outcome?}` — stop key (esc) to the hc terminal |
| `watchers.set` | One watcher row: `{projectId, projectName?, projectPath?, enabled, intervalSeconds, debounceSeconds, cooldownSeconds, maxPaths, lastChangeAt, lastDeliveredAt}` — opt-in; `off` disables |
| `watchers.list` | `{items: [watcher…]}` — all configured watchers |

The desktop GUI also requests `approvals.list` and `events.list` (with an
optional `limit`) to populate the approvals and activity views. These read
methods return an `items` array in `result`, as the GUI expects. The daemon is
implemented (v1), so GUI operations complete when it is running; the Ratatui
TUI is still demo-only and does not connect.

For `projects.register`, the CLI first canonicalizes the existing directory,
creates missing `.agents/bot/{ceo,git,ops,router}` folders and starter `AGENTS.md`
files, and generates the initial manifest/configs only when no per-bot
`opencode.jsonc` already exists. It does not overwrite existing files or copy
bot configs from another project. The daemon upserts registrations by
canonical path so repeated adds are idempotent.

`kbots project configure [PATH]` is local and does not need the daemon. The
editable `.agents/bot/kbots.json` has `version: 1` and a `bots` object with
`ceo`, `git`, and `ops` entries. Each entry has `enabled` and an `opencode`
object containing OpenCode config fields such as `instructions`, `plugins`,
`model`, and `permission`. It writes `.agents/bot/<role>/opencode.jsonc` for
enabled roles and preserves existing outputs by default. Use `--overwrite` to
regenerate from the manifest. Local plugin paths are made absolute so they
resolve consistently when OpenCode runs from a service working directory.
When a project already has OpenCode configs but no manifest, create the
manifest from those existing settings before using `--overwrite`.

`kbots sessions start` reads the enabled roles from the manifest and creates
one idle session in each role directory. It records IDs in the user's local
state directory and checks saved IDs against OpenCode before reusing them.
Session creation does not prompt the agents. This local action talks directly
to the OpenCode service and remains usable without the workflow daemon.
Its sibling, `kbots agents start`, is the managed path: the daemon owns the
session, the terminal, and the registry entry.

## File-change watcher

`kbots project watch --project REF [--off] [--interval N] [--debounce N]
[--cooldown N] [--max-paths N]` enables a per-project watcher in the daemon
(SQLite `watchers` table; disabled by default). The daemon polls
`git status --porcelain=v1` every `intervalSeconds`; once the tree has been
quiet for `debounceSeconds` and the managed git agent for the project is
idle, it prompts that agent with the delta (paths, capped at `maxPaths`) and
a fixed instruction referencing the project's git-bot policy. If the agent is
busy/blocked the delta is re-computed next tick and retried — the prompt is
never forced and never dropped silently. After every delivery a `cooldownSeconds`
quiet period suppresses further prompts so the agent's own edits don't
re-trigger it (no prompt ping-pong). Each delivery (or deferral) is logged as
a `watch.changed`/`watch.deferred` event. Only projects you explicitly enable
are watched. The watcher disables itself with a `watch.disabled` event when
the project is not a git repository or `git` fails.

## Managed agents (phase 1)

`agents.start` spawns or resumes a bot through the daemon. Exact binding:
the OpenCode session is created first via SDK with
`metadata={kbotsBotId, kbotsRole, kbotsProjectId}`; a terminal-mode start
then spawns `opencode --session <sessionId>` in the role directory via the
Herdr adapter (`hc run`), passing `kbotsBotId` as a label. The mapping
`bot_id ↔ hc_address ↔ session_id ↔ directory` is written in SQLite right at
spawn time. All managed agents are hc terminals; the headless mode was
retired on 2026-11-19 — existing headless rows are closed (marked `stopped`,
sessions kept) automatically at daemon boot.

Lifecycle states: `starting → running → stopping → stopped`, plus `error`.
The partial-failure path is handled by compensation (a stray terminal is
killed by label when the registry write fails; the failure is recorded on
the row and as an event) and by `reconcileAgents()`, which runs at daemon
boot and before `start/stop/show/prompt/interrupt/list`: it probes every
live agent, closes rows whose terminal exited (non-zero exit → `error`) or
disappeared, and marks broken sessions unreachable.

One active agent per project+role at most (partial unique index). Stopping
kills the terminal but keeps the OpenCode session and its history —
`agents.start` later resumes the exact same session. `--fresh` starts a new
one instead. `prompt` refuses unless the agent is idle (never `force`s past a
blocked program): it probes the `hc` status word. `interrupt` sends the stop
key (esc) to the terminal. Nothing deletes a session.

Model health: the maintenance loop compares each session's pinned model
against OpenCode's active model list (`client.model.list`). A model that
disappeared is reported as health code `model_unavailable`; when the terminal
is idle, the session is switched to `KBOTS_MODEL_FALLBACK` (comma-separated
`provider/model`), otherwise to another active model — free models before paid
ones, same provider before others — and the switch is logged as
`agent.model_switched`. If no model is left, the bot keeps the warning and
nothing is switched. A prompt that cannot start a turn because the session
model is unavailable now fails with an explicit, actionable error instead of
leaving the text in the input box; `agents.logs` shows the terminal screen and
session history for diagnosis.
Single-writer: all mutations flow through the daemon; the TUI/GUI render the
same registry.

### Bot report (what a bot is doing, and what is wrong)

Every `agents.list` item, `agents.show`, and `system.status.bots` carries a
derived report (`src/backend/bots/health.ts`) — never stored:

```json
{
  "activity": { "state": "idle", "detail": "idle · turn 3" },
  "health": {
    "level": "warning",
    "code": "idle_with_open_task",
    "message": "idle while task TASK-7 is running",
    "hint": "the nudger kicks this task automatically (KBOTS_NUDGE)"
  },
  "task": { "taskId": "TASK-7", "title": "…", "status": "running", "updatedAt": "…" },
  "live": { "state": "live", "status": "idle", "turn": 3 },
  "lastOutcome": null,
  "lastOutcomeAt": null
}
```

Levels: `ok`, `waiting` (task waits on a human: `review`/`blocked`),
`warning` (kbots has an automatic path — e.g. an idle CEO with an open task is
what the nudger kicks), `error` (the bot cannot work until something is fixed),
`stopped` (deliberate, not a problem), `unknown` (not probed). Codes worth
knowing: `working`, `idle`, `idle_with_open_task`, `idle_task_finished`,
`waiting_on_human`, `terminal_blocked`, `terminal_missing`,
`terminal_finished`, `agent_error`, `ceo_stopped_with_open_task`,
`ceo_recovering`, `ceo_stale_with_open_task`, `probe_failed`.

`agents.list` also returns `bots.byHealth` (counts), `bots.problemCount`, and
`problems` — the short actionable list (`agentId`, `role`, `level`, `code`,
`message`, `hint`, `taskId`) that the CLI, TUI, and GUI render.
`system.status` reports `bots` from the database only (no herdr probe, it is
polled every second) and caps `problems` at 10.

## Auto-kick (nudge) — v6

The daemon runs an auto-kick routine so tasks never stall silently when a
CEO session goes idle without marking its task done. Semantics:

- Every 60 s the daemon selects *nudge candidates*: dispatched, non-terminal
  tasks (`researching` | `planning` | `delegating` | `running`) whose linked
  CEO is a live `terminal` managed agent, with `nudges < KBOTS_NUDGE_MAX`
  (default 12) and `last_nudged_at` older than `KBOTS_NUDGE_COOLDOWN_MS`
  (default 15 min), or never nudged. `blocked` and `review` are NOT nudged —
  those wait on a human.
- Each candidate gets a short prompt via the same path as `agents.prompt`
  (`promptAgentCore`): the agent is NOT forced past a blocked program. On a
  refused delivery the cooldown timestamp moves (retry in 15 min) but no
  budget is spent. On a delivered nudge, `nudges` increments and `last_nudged_at`
  is set, and a `task.nudged` event is logged (with `data: {taskId, agentId, nudge}`).
- The prompt asks the agent to either complete the work and run
  `kb tasks update <id> --status done --note "…"`, or to state exactly what
  blocks it — plain text, no forced action, no file edits required by the
  router itself.
- Set `KBOTS_NUDGE=off` in the daemon environment to disable the loop;
  `KBOTS_NUDGE_COOLDOWN_MS` and `KBOTS_NUDGE_MAX` tune rate and budget.
- Nudges are not user messages and do not enter `tasks.brief`/`tasks.inbox`.
  They are operational events, visible in `events.list` as `task.nudged`.

## Maintenance loop (reconcile + reaper + recovery) — v7

The daemon runs one more timer besides the watcher and the nudger: a
maintenance tick, default every 60 s (`KBOTS_REAP_INTERVAL_MS`, minimum 10 s).
It does three things, in order:

1. `reconcileAgents()` — the same probe every `agents.*` mutation runs, now
   also on a timer, so a terminal that exits between RPCs is noticed instead
   of waiting for the next client call.
2. The reaper — stops a CEO terminal whose task already reached a terminal
   status (`done`, `failed`, `canceled`) and has been quiet for the grace
   period (`KBOTS_REAP_GRACE_MS`, default 10 min, measured from the task's
   `updated_at`).
3. Dead-CEO recovery — resumes a CEO whose terminal failed while its task is
   still open (next section).

Rules:

- Only `role = ceo`, `mode = terminal`, `status = running` agents with a
  `task_agents` link are reaped. git/ops bots are long-lived per project;
  agents without a task link are not attributable and are never touched.
- Stopping uses the `agents.stop` path (`stopAgentCore`): the terminal is
  killed and the row moves to `stopped`. **The OpenCode session is kept**,
  together with its history — same contract as a manual stop. Nothing is
  deleted.
- A failed kill marks the row `error` and logs `agent.reap_failed`; the loop
  continues with the other agents and retries on the next tick.
- `agent.reaped` is logged with `data: {agentId, sessionId, taskId, reason}`.
- `KBOTS_REAP=off` skips the reaper step only; the reconcile tick keeps
  running.
- Overlapping ticks are skipped, so a slow reconcile cannot pile up.

## Dead-CEO recovery — v7

A task in `researching`/`planning`/`delegating`/`running` whose CEO terminal
died would otherwise stall forever: the nudger only kicks live CEOs.

- Recovery resumes the CEO against its **existing session** (`resumeAgentCore`
  → the `agents.start` resume path), so the agent keeps its context; it then
  prompts "your terminal was restarted, continue task X". The session is
  never recreated and never deleted.
- Eligible rows: `error` (failed terminal, non-zero exit, broken probe), and
  `stopped` rows whose `stop_reason` is not `operator`. `agents.stop` sets
  `stop_reason = operator` (schema v8), so a stop the user asked for is never
  overridden — that case surfaces as `ceo_stopped_with_open_task` in the bot
  report (hint: `kbots agents start ceo`). Reconcile stamps the other reasons
  (`exit`, `crash`, `terminal_missing`, `legacy_headless`).
- Only tasks updated within `KBOTS_RECOVER_WINDOW_MS` (default 2 h) are
  resumed; older work is reported for a human instead of restarted.
- Recovery respects the per-project CEO cap (`KBOTS_CEO_CAP`, default 4): at
  the cap the attempt is deferred with a `task.ceo_recovery_deferred` event.
- Bounded: each attempt spends one unit of `tasks.recoveries` (schema v7,
  with `tasks.last_recovery_at`), max `KBOTS_RECOVER_MAX` (default 2),
  rate-limited by `KBOTS_RECOVER_COOLDOWN_MS` (default 5 min). When the
  budget is gone the task moves to `blocked` with a blocker note and a
  `task.ceo_unrecoverable` event, so a crash loop ends in the inbox rather
  than in an endless restart cycle. A failed attempt still spends a unit.
- `KBOTS_RECOVER=off` disables the step; `KBOTS_RECOVER_MAX`,
  `KBOTS_RECOVER_COOLDOWN_MS`, and `KBOTS_RECOVER_WINDOW_MS` tune it.
- Events: `bot.resumed`, `bot.resume_failed`, `task.ceo_recovered`,
  `task.ceo_recovery_failed`, `task.ceo_unrecoverable`.

## v2 methods (task-centric dispatch)

Implemented alongside the Mission Control work; see
[`plans/2026-11-18-task-centric-mission-control.md`](plans/2026-11-18-task-centric-mission-control.md).
Nothing here is claimed deployed beyond what the repo contains — the daemon
may still be serving v1 until `./install.sh` redeploys it. The v1 framing is
unchanged: one request/response per connection, no streaming.

| Method | Params | Result |
| --- | --- | --- |
| `tasks.create` | `title`, `goal?`, `project?`, `autoDispatch?` (default `true`) | `{taskId, status, ceoAgentId?, dispatchError?}` |
| `tasks.list` | `project?`, `status?` (NEW filter) | `{items: [...]}` |
| `tasks.get` | `taskId` | task object + `ceo` link + `pendingMessageCount` |
| `tasks.message` | `taskId`, `text` | `{taskId, outcome: "delivered" \| "queued", messageId?}` |
| `tasks.messages` | `taskId` | `{items: [...]}` queued/delivered backlog |
| `tasks.brief` | `taskId?` | no arg → global overview: `{generatedAt, counts, needsAction: [...], running: [...], done: [...]}`; with arg → `{taskId, title, status, goal, summary, decisions, blockers, nextStep, ceo, workers, pendingMessages}` |
| `tasks.inbox` | `{}` | `{items: [{kind: "approval" \| "blocked" \| "failed" \| "queued" \| "review", taskId?, …}]}` — merge of pending approvals, blocked/failed/review tasks, and queued user messages |
| `tasks.update` | `taskId`, `status` (allow: `review`, `blocked`, `done`, `failed`, `canceled`), `note?` | `{taskId, status}` — explicit operator control; appends `note` to `task_briefs.decisions` when given |
| `router.run` | `{ project?, text, selectedTaskId?, dryRun? }` | `{ decision, outcome }` — see [AI Router](#ai-router-routerrun) below |
| `tasks.markWorked` | *(not added)* | deferred |

Semantics:

- `tasks.create` with `autoDispatch` true (the default) validates the task
  and dispatches it to a CEO. A dispatch failure does **not** fail the RPC:
  the task stays `queued`, the result carries `dispatchError`, and a
  `task.dispatch_failed` event is logged. `ceoAgentId` is present when a CEO
  was bound. The v1 example near the top of this document (`status:
  "planning"`) describes v1 behavior; with v2, a fresh task starts `queued`
  and moves to `researching` at dispatch.
- `tasks.list` gains an optional `status` filter alongside `project`.
- `tasks.get` returns the task object plus its `ceo` link (from
  `task_agents`) and `pendingMessageCount` (queued `task_messages` rows).
- `tasks.message` resolves task → CEO agent. Idle → delivered via the same
  path as `agents.prompt` (never forced past a blocked program). Busy or
  blocked → a `task_messages` row with `status: queued`. On delivery the row
  is set to `delivered` and a `task.message_delivered` event is logged.
  `messageId` is returned when a message row was created.
- `tasks.update` is explicit operator control, not model feedback.
  `done`/`failed`/`canceled` are terminal; no further transitions apply once
  one is set. When `note` is given it is appended (newline-separated, newest
  last) to the task's `task_briefs.decisions`.
- `tasks.brief` is composed from SQLite (tasks, task_agents, task_messages,
  task_briefs, events); it never round-trips to the model or to a CEO
  session. With no argument it returns the global overview; the TUI's
  `/brief` command is `tasks.brief` with no argument. The per-task brief is
  composed from the DB likewise — goal, accumulated decisions, blockers, and
  next step — not generated by the CEO on demand.
- `tasks.messages` lists the queued/delivered backlog for one task.

### AI Router (router.run)

A thin proposal layer over the deterministic dispatcher. The router never
spawns agents and never calls `agents.*`; it returns a structured
`decision`, and when confident the daemon executes through the existing
`tasks.create` / `tasks.message` paths (with their gates, dispatch and
queued-message semantics unchanged). `text` is required (trimmed, 1–4000
chars). `project` accepts the usual refs (id, name or absolute path);
otherwise the project is resolved from `selectedTaskId`, else the single
registered project, else a task reference in the text that resolves to a
project-bearing task, else the call fails (zero projects) or returns an
`ask_user` listing the registered names (several, none determinable).
Candidate tasks are this project's only, newest first, capped at 25.

Semantics:

- **Deterministic bypass first**: an explicit `@TASK-42` / `TASK-42`
  reference (first match that exists and belongs to the resolved project) or
  a valid `selectedTaskId` produces `continue_task` with confidence 1 — no
  AI call. `selectedTaskId` wins over the regex; a missing task →
  `-32602 task not found`, another project's task → `-32602`.
- **Otherwise one bounded Codex turn** (single prompt → JSON, no tool loop),
  in a thread started with `sandboxMode: "danger-full-access"` and
  `approvalPolicy: "never"` (yolo mode, owner's 2026-11-19 directive): the
  bounded turn may edit files and run tools; the socket idle timeout for
  this method is 120 s (others stay at 10 s). The machine-default model
  from `~/.codex/config.toml` is used (no model override).
- **Validation + retry**: the reply is parsed leniently (code fences
  stripped, first balanced JSON object) and validated strictly; unknown
  fields are dropped, `ask_user` candidates are filtered to existing
  same-project ids. One retry with the error fed back; still invalid (or
  any AI-call exception: network, timeout, abort, missing CLI) →
  `ask_user` with confidence 0 and no side effects.
- **Confidence threshold**: create/continue decisions below
  `KBOTS_ROUTER_MIN_CONFIDENCE` (default 0.65, clamped to [0,1]) are
  converted to `ask_user` referencing the proposed title/task.
- **dryRun**: `dryRun: true` logs `router.decision` and returns
  `outcome: { kind: "dry_run" }` — no side effects.
- Every exit logs a `router.decision` event (`action`, `confidence`,
  `projectId`, `taskId?`, `dryRun`, `source`: `deterministic` | `ai` |
  `fallback`).

Result shapes:

| `decision.action` | `decision` | `outcome.kind` |
| --- | --- | --- |
| `create_task` | `{ action, projectId, title, goal?, detail?, confidence }` | `created` → `{ taskId, status, ceoAgentId?, dispatchError? }` via `tasks.create` (autoDispatch) |
| `continue_task` | `{ action, projectId, taskId, message, confidence }` | `continued` → `{ taskId, outcome: "delivered" \| "queued", agentId, messageId? }` via `tasks.message` (busy CEO → queued, not an error) |
| `ask_user` | `{ action, question, candidateTaskIds, confidence }` | `ask_user` → `{ question, candidateTaskIds, candidates: [{ taskId, title }] }` — zero side effects; the `candidates` objects carry this project's titles in `candidateTaskIds` order (unknown ids are dropped) |
| *(dry run)* | *(any of the above)* | `dry_run` → no side effects beyond the event |

Notes:

- `ask_user` outcome always carries `candidates` (empty when none match).
- The router is local and honest: it is not a guarantee of model quality —
  treat low-confidence or surprising proposals accordingly. Explicit
  `@TASK-id` routing remains the deterministic, always-available path.
- Events gain `router.decision` alongside the existing kinds.

### CEO Session Pool and dispatch semantics

- All CEOs for a project share the project's `.agents/bot/ceo/` directory
  (role config, instructions, model). There are no `ceo-1`, `ceo-2`
  directories.
- Each dispatched task gets its own OpenCode session created in that
  directory, with metadata `{kbotsBotId, kbotsTaskId, kbotsRole: "ceo",
  kbotsProjectId}`. Mode `terminal` always: dispatch spawns an `hc`
  terminal running `opencode --session <id>`, so the task CEO is always
  watchable and steerable (headless mode was retired 2026-11-19).
- Per-project active-CEO cap, default 4; override with `KBOTS_CEO_CAP`.
  Over the cap the task stays `queued`, a `task.deferred` event is logged,
  and dispatch is retried by reconcile or the next action.
- Dispatch runs in one SQLite transaction with compensation (mirroring the
  managed-agent pattern): validate the task exists, status `queued`,
  project known → check the per-project active-CEO cap → create the SDK
  session in the project's ceo directory → insert the `agents` row (role
  `ceo`, `starting → running`) and the `task_agents` link → commit → queue
  the first prompt (task title + goal + constraints), set status →
  `researching`, log `task.dispatched`.
- Idempotency: `idx_task_agents_ceo` (unique partial index on
  `task_agents(task_id) WHERE role = 'ceo'`) is the dispatch idempotency
  key. A re-run of dispatch or a daemon restart mid-dispatch converges to
  one CEO per task.
- Boot-time reconcile extends `reconcileAgents()` to task CEOs and to
  flushing queued `task_messages` to idle CEOs.
- CEO sessions are kept after finishing (review/continue). Nothing deletes
  a session. `--fresh` is not a concept for task CEOs; cancel the task
  instead.
- Follow-up routing: `tasks.message` is the explicit follow-up path; see
  `tasks.message` above.

### Schema v4 summary

Migration `user_version` 3 → 4. The `tasks` table is rebuilt (SQLite cannot
alter a CHECK) preserving ids, with legacy statuses remapped:
`planning → queued`, `in_progress → running`, `completed → done`,
`canceled` unchanged. New columns on `tasks`: `goal TEXT` (the original
ask, verbatim) and `detail TEXT` (optional expanded brief).

Status enum:

```
queued | researching | planning | delegating | running | review | blocked | done | failed | canceled
```

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
guarantees at most one CEO per task, which doubles as the dispatch
idempotency key.

`events.kind` gains: `task.dispatched`, `task.dispatch_failed`,
`task.deferred`, `task.message_queued`, `task.message_delivered`,
`task.status`, `brief.updated`, `router.decision`. Existing kinds are
unchanged. Events carry
`data.taskId` by convention.

### Honest v2 semantics

- The daemon cannot see inside a running CEO run. Automatic "CEO asked a
  question" detection needs OpenCode session events, which do not exist
  yet; until then the Inbox surfaces pending approvals, failed/blocked
  tasks, and the queued-message backlog. `tasks.message` is the explicit
  follow-up path.
- Parallel CEOs share a working tree. Until worktree isolation lands (P3),
  two CEOs on the same project can edit the same files. Mitigations for
  now: the per-project dispatch cap (default 4), keeping code-writing roles
  to one active task per project tree unless the risk is accepted, and clear
  warnings in task output. GIT/OPS stay single per project, always.
- Status transitions after dispatch are advisory. `queued → researching` is
  set at dispatch; `done/failed` come from explicit marks, approvals, or
  reconcile findings, not from peeking at the model.
- `tasks.brief` never round-trips to the model; briefing is composed from
  SQLite.
- `approvals.approve` still records the decision only; nothing
  auto-executes.
- `events.subscribe` is DEFERRED — the v1 framing (one request/response per
  connection) is unchanged; the TUI polls on a timer plus after actions.
- P3 (hc worker lifecycle, concurrency caps, serialized GIT/OPS queue, git
  worktree isolation, approval gates) is roadmap, not implemented.

## Bot folder convention

The `kt-system` and `nsac` references share the `ceo/`, `git/`, and `ops/` bot
roles. Their role instructions, OpenCode configs, plugins, watcher scripts,
schedules, and resource limits are project-specific. Registration guarantees
the common directories only; it leaves those project contracts to each repo.

## Daemon status (v1)

The daemon exists and serves the methods above. It stores state in SQLite at
`${XDG_STATE_HOME:-~/.local/state}/kbots/kbots.db` (WAL mode). It creates the
socket's parent directory, unlinks stale sockets, sets the socket mode to
`0600`, and cleans up the socket on SIGINT/SIGTERM. It also binds the
`$HOME/.local/run/kbots.sock` fallback described above unless an explicit
socket is set or the primary already is that path. Unknown methods return
JSON-RPC `-32601`.

Honest v1 semantics:

- `agents.start/show/stop/prompt/interrupt` manage bot agents through the
  daemon (see "Managed agents" above). `agents.list` additionally returns
  them in `managed`.
- `tasks.create` records the task (status `"planning"`) and logs an event.
  Nothing auto-dispatches tasks to a bot yet; the orchestrator/workflow engine
  is still planned. (Superseded by the v2 dispatch path below once the
  Mission Control backend lands: `tasks.create` auto-dispatches by default
  and status starts `queued`.)
- `tasks.cancel` is idempotent.
- `projects.register` upserts by canonical path (idempotent).
- `approvals.approve` records the approve decision; no protected operation is
  auto-executed.
- `agents.list` merges the project registry, each project's
  `.agents/bot/kbots.json` manifest, and saved session IDs from
  `${XDG_STATE_HOME:-~/.local/state}/kbots/projects/*.json`. It is read-only
  and does not prompt or start agents. It also shells out to `hc ls`
  (read-only, via the Herdr adapter) to attach live terminal info: exact
  role-directory matches get a `herdr` block on the item (address, name,
  agent, state, status, turn), and live terminals whose cwd is inside a
  registered project path are returned in `liveTerminals` (plus a
  `filteredOut` count of hidden terminals from other projects). If `hc` is
  unavailable or fails, the handler still succeeds and reports
  `herdrUnavailable` instead.
- `events.list` returns the ordered event log, newest first, with a `limit`
  parameter (default 100, max 500).
- No OpenCode prompting is wired to tasks yet. The daemon does not embed
  provider credentials, session inspection stays read-only, and clients never
  write workflow state directly.

Mutating methods are idempotent where noted above; broader permission, lock,
and approval enforcement for mutating methods is still incomplete.

## See also

- [`docs/plans/2026-11-18-task-centric-mission-control.md`](plans/2026-11-18-task-centric-mission-control.md) — the Mission Control design this v2 surface implements (source of truth for the v2 semantics above).
