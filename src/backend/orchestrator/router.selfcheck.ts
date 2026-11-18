/**
 * Stand-alone self-check for the AI router internals. NOT imported by the
 * daemon. Must not touch the network, the Codex CLI, or the database.
 *
 * Run: bun run src/backend/orchestrator/router.selfcheck.ts
 */
import assert from "node:assert/strict"

import { __routerInternals } from "./router.ts"

const {
  parseLenientJson,
  validateDecision,
  findTaskReference,
  routerMinConfidence,
  extractFirstJsonObject,
  RouteValidationError,
} = __routerInternals

// ---------------------------------------------------------------------------
// parseLenientJson
// ---------------------------------------------------------------------------

const raw = parseLenientJson('{"action":"ask_user","question":"q","confidence":0.5}')
assert.equal((raw as Record<string, unknown>).action, "ask_user")

const fenced = parseLenientJson('```json\n{"action":"ask_user","question":"q","confidence":0.5}\n```')
assert.equal((fenced as Record<string, unknown>).confidence, 0.5)

const prose = parseLenientJson(
  'Sure! Here is the decision:\n{"action":"create_task","projectId":"PRJ-1","title":"t","confidence":0.9}\nHope that helps.',
)
assert.equal((prose as Record<string, unknown>).action, "create_task")

const nested = parseLenientJson(
  '{"action":"create_task","projectId":"PRJ-1","title":"fix { the } parser { bug","confidence":0.8}',
)
assert.equal((nested as Record<string, unknown>).title, "fix { the } parser { bug")

assert.throws(() => parseLenientJson("no json here at all"), RouteValidationError)
assert.throws(() => parseLenientJson('{"action":'), RouteValidationError)

assert.equal(
  extractFirstJsonObject('prefix {"a":{"b":1}} suffix'),
  '{"a":{"b":1}}',
)

// ---------------------------------------------------------------------------
// validateDecision (ctx stubbed — no database)
// ---------------------------------------------------------------------------

const sameProject = new Set(["TASK-1", "TASK-2"])
const ctx = {
  projectId: "PRJ-1",
  isSameProjectTask: (id: string) => sameProject.has(id),
}

const created = validateDecision(
  { action: "create_task", projectId: "PRJ-1", title: "Ship it", confidence: 0.9 },
  ctx,
)
assert.deepEqual(created, { action: "create_task", projectId: "PRJ-1", title: "Ship it", confidence: 0.9 })

assert.throws(
  () => validateDecision({ action: "create_task", projectId: "PRJ-1", title: "t", confidence: 1.5 }, ctx),
  RouteValidationError,
)
assert.throws(
  () =>
    validateDecision(
      { action: "create_task", projectId: "PRJ-1", title: "t", confidence: "0.9" },
      ctx,
    ),
  RouteValidationError,
)

assert.throws(
  () =>
    validateDecision(
      { action: "continue_task", projectId: "PRJ-1", taskId: "TASK-99", message: "m", confidence: 0.9 },
      ctx,
    ),
  RouteValidationError,
)

const otherProjectCtx = {
  projectId: "PRJ-1",
  isSameProjectTask: () => false,
}
assert.throws(
  () =>
    validateDecision(
      { action: "continue_task", projectId: "PRJ-1", taskId: "TASK-1", message: "m", confidence: 0.9 },
      otherProjectCtx,
    ),
  RouteValidationError,
)

const continued = validateDecision(
  { action: "continue_task", projectId: "PRJ-1", taskId: "TASK-1", message: "please finish", confidence: 0.8 },
  ctx,
)
assert.equal(continued.action, "continue_task")
assert.equal(continued.taskId, "TASK-1")

const filtered = validateDecision(
  {
    action: "ask_user",
    question: "Which one?",
    candidateTaskIds: ["TASK-1", "TASK-99", "TASK-2", "TASK-1"],
    confidence: 0.4,
  },
  ctx,
)
assert.deepEqual(filtered, {
  action: "ask_user",
  question: "Which one?",
  candidateTaskIds: ["TASK-1", "TASK-2"],
  confidence: 0.4,
})

assert.throws(() => validateDecision({ action: "ask_user", confidence: 0.5 }, ctx), RouteValidationError)
assert.throws(() => validateDecision("not an object", ctx), RouteValidationError)
assert.throws(() => validateDecision(["not", "an", "object"], ctx), RouteValidationError)

// ---------------------------------------------------------------------------
// findTaskReference
// ---------------------------------------------------------------------------

assert.equal(findTaskReference("@TASK-12 fix it"), "TASK-12")
assert.equal(findTaskReference("look at TASK-3 please"), "TASK-3")
assert.equal(findTaskReference("no id here"), null)
assert.equal(findTaskReference("task-7 lowercase"), "TASK-7")
assert.equal(findTaskReference("TASK-12 and TASK-3"), "TASK-12")

// ---------------------------------------------------------------------------
// routerMinConfidence
// ---------------------------------------------------------------------------

const originalEnv = process.env.KBOTS_ROUTER_MIN_CONFIDENCE
try {
  delete process.env.KBOTS_ROUTER_MIN_CONFIDENCE
  assert.equal(routerMinConfidence(), 0.65)
  process.env.KBOTS_ROUTER_MIN_CONFIDENCE = "0.8"
  assert.equal(routerMinConfidence(), 0.8)
  process.env.KBOTS_ROUTER_MIN_CONFIDENCE = "2"
  assert.equal(routerMinConfidence(), 1)
  process.env.KBOTS_ROUTER_MIN_CONFIDENCE = "abc"
  assert.equal(routerMinConfidence(), 0.65)
} finally {
  if (originalEnv === undefined) delete process.env.KBOTS_ROUTER_MIN_CONFIDENCE
  else process.env.KBOTS_ROUTER_MIN_CONFIDENCE = originalEnv
}

console.log("router self-check OK")
