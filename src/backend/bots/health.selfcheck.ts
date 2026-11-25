/**
 * Stand-alone self-check for the bot report rules. NOT imported by the
 * daemon. Pure: no database, no herdr, no network.
 *
 * Run: bun run src/backend/bots/health.selfcheck.ts
 */
import assert from "node:assert/strict"

import { describeBot, isProblem, problemLine } from "./health.ts"
import type { AgentRow, TaskRow } from "../db/repositories.ts"

const agent = (overrides: Partial<AgentRow> = {}): AgentRow => ({
  id: "AGT-1",
  projectId: "PRJ-1",
  role: "ceo",
  mode: "terminal",
  directory: "/repo/.agents/bot/ceo",
  status: "running",
  sessionId: "ses_1",
  hcAddress: "t12a",
  name: "kbots-ceo",
  createdAt: "2026-11-20T00:00:00.000Z",
  updatedAt: "2026-11-20T00:00:00.000Z",
  stoppedAt: null,
  lastError: null,
  lastOutcome: null,
  lastOutcomeAt: null,
  stopReason: null,
  ...overrides,
});

const task = (overrides: Partial<TaskRow> = {}): TaskRow => ({
  taskId: "TASK-1",
  title: "Fix the parser",
  status: "running",
  projectId: "PRJ-1",
  projectPath: "/repo",
  createdAt: "2026-11-20T00:00:00.000Z",
  updatedAt: new Date().toISOString(),
  canceledAt: null,
  goal: null,
  detail: null,
  recoveries: 0,
  lastRecoveryAt: null,
  ...overrides,
});

// -- working -----------------------------------------------------------------
{
  const report = describeBot(agent(), { state: "live", status: "busy", turn: 7 }, task())
  assert.equal(report.activity.state, "working")
  assert.equal(report.activity.detail, "working · turn 7")
  assert.equal(report.health.level, "ok")
  assert.equal(isProblem(report), false)
  assert.equal(report.task?.taskId, "TASK-1")
}

// -- idle with an open task: what the nudger kicks ----------------------------
{
  const report = describeBot(agent(), { state: "live", status: "idle", turn: 3 }, task())
  assert.equal(report.health.level, "warning")
  assert.equal(report.health.code, "idle_with_open_task")
  assert.match(report.health.message, /TASK-1 is running/)
  assert.equal(isProblem(report), true)
}

// -- idle with no task is fine -----------------------------------------------
{
  const report = describeBot(agent(), { state: "live", status: "idle" }, null)
  assert.equal(report.health.level, "ok")
  assert.equal(report.health.code, "idle")
}

// -- idle while the task waits on a human ------------------------------------
{
  const report = describeBot(agent(), { state: "live", status: "idle" }, task({ status: "review" }))
  assert.equal(report.activity.state, "waiting")
  assert.equal(report.health.level, "waiting")
  assert.equal(isProblem(report), false)
}

// -- idle with a finished task: harmless, the reaper handles it --------------
{
  const report = describeBot(agent(), { state: "live", status: "idle" }, task({ status: "done" }))
  assert.equal(report.health.level, "ok")
  assert.equal(report.health.code, "idle_task_finished")
}

// -- blocked program ---------------------------------------------------------
{
  const report = describeBot(agent(), { state: "live", status: "blocked", reason: "approval" }, task())
  assert.equal(report.activity.state, "blocked")
  assert.equal(report.health.code, "terminal_blocked")
  assert.equal(report.health.level, "warning")
}

// -- dead terminals ----------------------------------------------------------
{
  const missing = describeBot(agent(), { state: "missing" }, task())
  assert.equal(missing.health.code, "terminal_missing")
  assert.equal(missing.health.level, "error")

  const finished = describeBot(agent(), { state: "finished", reason: "exit", exitCode: 2 }, task())
  assert.equal(finished.health.code, "terminal_finished")
  assert.match(finished.health.message, /exit 2/)
}

// -- unreadable probe --------------------------------------------------------
{
  const report = describeBot(agent(), null, task())
  assert.equal(report.health.code, "probe_failed")
  assert.equal(report.health.level, "warning")
}

// -- registry error wins over the probe --------------------------------------
{
  const report = describeBot(agent({ status: "error", lastError: "spawn failed" }), { state: "live", status: "idle" }, task())
  assert.equal(report.health.code, "agent_error")
  assert.equal(report.health.message, "spawn failed")
}

// -- a stopped CEO with an open task is a problem ----------------------------
{
  const report = describeBot(agent({ status: "stopped", stopReason: "operator" }), null, task())
  assert.equal(report.health.code, "ceo_stopped_with_open_task")
  assert.equal(report.health.level, "error")
  assert.match(report.health.hint ?? "", /agents start ceo/)
}

// -- a non-operator stop with an open task is left to the recovery loop ------
{
  const report = describeBot(agent({ status: "stopped", stopReason: "exit" }), null, task())
  assert.equal(report.health.code, "ceo_recovering")
  assert.equal(report.health.level, "warning")
}

// -- ...old tasks are continued by the maintenance routine -------------------
{
  const old = task({ updatedAt: new Date(Date.now() - 5 * 60 * 60 * 1000).toISOString() })
  const report = describeBot(agent({ status: "stopped", stopReason: "exit" }), null, old)
  assert.equal(report.health.code, "ceo_stale_with_open_task")
  assert.match(report.health.hint ?? "", /maintenance routine/)
  assert.match(report.health.message, /maintenance routine will continue it/)
}

// -- ...unless stale continuation is switched off ----------------------------
{
  const saved = process.env.KBOTS_STALE_CONTINUE
  try {
    process.env.KBOTS_STALE_CONTINUE = "off"
    const old = task({ updatedAt: new Date(Date.now() - 5 * 60 * 60 * 1000).toISOString() })
    const report = describeBot(agent({ status: "stopped", stopReason: "exit" }), null, old)
    assert.equal(report.health.code, "ceo_stale_with_open_task")
    assert.match(report.health.hint ?? "", /agents start ceo/)
    assert.match(report.health.message, /too old for automatic recovery/)
  } finally {
    if (saved === undefined) delete process.env.KBOTS_STALE_CONTINUE
    else process.env.KBOTS_STALE_CONTINUE = saved
  }
}

// -- a stopped CEO with no open task is fine ---------------------------------
{
  const report = describeBot(agent({ status: "stopped", stopReason: "operator" }), null, task({ status: "done" }))
  assert.equal(report.health.level, "stopped")
  assert.equal(isProblem(report), false)
}

// -- not probed (system.status): DB-only warning for an open task ------------
{
  const report = describeBot(agent(), undefined, task())
  assert.equal(report.health.level, "warning")
  assert.equal(report.health.code, "idle_with_open_task")
}

// -- problem line ------------------------------------------------------------
{
  const report = describeBot(agent(), { state: "missing" }, task())
  assert.equal(problemLine(report, agent()), "AGT-1 ceo · TASK-1: the terminal is gone but the agent is still registered as live")
}

// -- an idle bot whose session model is gone cannot start a turn -------------
{
  const report = describeBot(
    agent(),
    { state: "live", status: "idle", model: { providerID: "anthropic", id: "claude-x" }, modelAvailable: false },
    task(),
  )
  assert.equal(report.activity.state, "idle")
  assert.equal(report.health.code, "model_unavailable")
  assert.equal(report.health.level, "warning")
  assert.match(report.health.message, /anthropic\/claude-x is unavailable/)
  assert.match(report.health.hint ?? "", /KBOTS_MODEL_FALLBACK/)
  assert.equal(report.model?.id, "claude-x")
  assert.equal(report.model?.available, false)
}

// -- a blocked terminal is reported as blocked, not as a model problem -------
{
  const report = describeBot(
    agent(),
    { state: "live", status: "blocked", reason: "approval", model: { providerID: "anthropic", id: "claude-x" }, modelAvailable: false },
    task(),
  )
  assert.equal(report.activity.state, "blocked")
  assert.equal(report.health.code, "terminal_blocked")
}

// -- an available model leaves the idle states alone -------------------------
{
  const report = describeBot(
    agent(),
    { state: "live", status: "idle", model: { providerID: "anthropic", id: "claude-y" }, modelAvailable: true },
    null,
  )
  assert.equal(report.health.code, "idle")
  assert.equal(report.model?.available, true)
}

console.log("health selfcheck: ok (20 cases)")
