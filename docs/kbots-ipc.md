# kbots local IPC contract

The Bun daemon owns workflow state and side effects. GUI, TUI, and CLI are
clients. The initial transport contract is one newline-delimited JSON-RPC 2.0
request and response per Unix domain socket connection. The daemon is
implemented (v1 minimal) and runs as the `kbots-daemon` systemd user service
after `./install.sh` (opt out with `KBOTS_SKIP_DAEMON_UNIT=1`), or manually
with `bun run daemon` (equivalently `bun run src/backend/daemon.ts`) without
systemd.

The CLI discovers the socket in this order:

1. `--socket PATH`
2. `KBOTS_SOCKET`
3. `$XDG_RUNTIME_DIR/kbots.sock`
4. `$HOME/.local/run/kbots.sock`

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
| `agents start ROLE [--mode terminal\|headless] [--project REF] [--fresh]` | `agents.start` | `role`, optional `mode` (default `terminal`), optional `project`, optional `fresh` |
| `agents show ID` | `agents.show` | `botId` |
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
| `agents.stop` | `{id, projectId, role, mode, status, sessionId, stoppedAt, …}` — terminal is killed; the OpenCode session and history are kept |
| `agents.prompt` | `{botId, mode, accepted, turn?}` — refuses unless the agent is idle (never `force`s past a blocked program) |
| `agents.interrupt` | `{botId, mode, key?, outcome?}` — `terminal` mode: stop key (esc); `headless` mode: OpenCode session interrupt |
| `watchers.set` | One watcher row: `{projectId, projectName?, projectPath?, enabled, intervalSeconds, debounceSeconds, cooldownSeconds, maxPaths, lastChangeAt, lastDeliveredAt}` — opt-in; `off` disables |
| `watchers.list` | `{items: [watcher…]}` — all configured watchers |

The desktop GUI also requests `approvals.list` and `events.list` (with an
optional `limit`) to populate the approvals and activity views. These read
methods return an `items` array in `result`, as the GUI expects. The daemon is
implemented (v1), so GUI operations complete when it is running; the Ratatui
TUI is still demo-only and does not connect.

For `projects.register`, the CLI first canonicalizes the existing directory,
creates missing `.agents/bot/{ceo,git,ops}` folders and starter `AGENTS.md`
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
spawn time. `headless` mode stops after the session creation.

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
one instead. `prompt` refuses unless the agent is idle (they are never sent
with `force`): terminal mode probes the `hc` status word, headless mode
probes the session outcome; `interrupt` sends the stop key in terminal mode
or interrupts the session in headless mode. Nothing deletes a session.
Single-writer: all mutations flow through the daemon; the TUI/GUI render the
same registry.

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

### CEO Session Pool and dispatch semantics

- All CEOs for a project share the project's `.agents/bot/ceo/` directory
  (role config, instructions, model). There are no `ceo-1`, `ceo-2`
  directories.
- Each dispatched task gets its own OpenCode session created in that
  directory, with metadata `{kbotsBotId, kbotsTaskId, kbotsRole: "ceo",
  kbotsProjectId}`. Mode `headless` by default, `terminal` on demand.
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
`task.status`, `brief.updated`. Existing kinds are unchanged. Events carry
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
`0600`, and cleans up the socket on SIGINT/SIGTERM. Unknown methods return
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
