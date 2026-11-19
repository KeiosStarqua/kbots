/**
 * Auto-kick (nudge): the daemon periodically asks any task CEO that has not
 * finished its task to make progress — or to report what blocks it. Tasks
 * never stall silently because
 *   - the agent went idle without marking the task done, or
 *   - the turn ended without a recorded outcome.
 *
 * Rules are deliberately narrow and safe:
 *  - Only dispatched, non-terminal, non-human-blocked statuses are nudged
 *    (`researching`, `planning`, `delegating`, `running`). `blocked` and
 *    `review` wait on a human decision, not on the agent.
 *  - The CEO must be a live terminal-mode managed agent (hc). Prompts use
 *    the same `promptAgentCore` path as `agents.prompt`; a busy or blocked
 *    program is never forced — the nudge is skipped (cooldown bumped, no
 *    budget spent).
 *  - Budget is bounded per task (default 12 nudges) and rate-limited
 *    (default 15 minutes between nudges). `KBOTS_NUDGE=off` disables the
 *    loop entirely; `KBOTS_NUDGE_COOLDOWN_MS` and `KBOTS_NUDGE_MAX` tune it.
 */
import {
  addEvent,
  getAgent,
  listNudgeCandidates,
  recordNudge,
  touchNudgeCooldown,
  type TaskStatus,
} from "../db/repositories.ts"
import { promptAgentCore } from "../bots/agents.ts"

const TICK_MS = 60_000

function cooldownMs(): number {
  const raw = Number.parseInt(process.env.KBOTS_NUDGE_COOLDOWN_MS ?? "", 10)
  return Number.isFinite(raw) && raw >= 60_000 ? raw : 15 * 60 * 1000
}

function maxNudges(): number {
  const raw = Number.parseInt(process.env.KBOTS_NUDGE_MAX ?? "", 10)
  return Number.isFinite(raw) && raw >= 1 ? raw : 12
}

export function nudgePrompt(task: { taskId: string; title: string; status: TaskStatus }): string {
  return [
    `kbots auto-check: task ${task.taskId} ("${task.title}") is not marked done yet (status: ${task.status}).`,
    "",
    `If the work is complete, run: kb tasks update ${task.taskId} --status done --note "<one-line result>"`,
    "If it is blocked, say exactly what blocks it.",
    "Otherwise keep working and keep this task thread updated; do not pick up unrelated work.",
  ].join("\n")
}

export async function findEligibleTasks(): Promise<ReturnType<typeof listNudgeCandidates>> {
  return listNudgeCandidates(cooldownMs(), maxNudges())
}

export async function tick(): Promise<void> {
  const candidates = findEligibleTasks()
  for (const candidate of await candidates) {
    const ceo = getAgent(candidate.ceoAgentId)
    if (!ceo) {
      touchNudgeCooldown(candidate.taskId, new Date().toISOString())
      continue
    }
    try {
      await promptAgentCore(ceo, nudgePrompt(candidate))
      const at = new Date().toISOString()
      recordNudge(candidate.taskId, at)
      addEvent(
        "task.nudged",
        `Task ${candidate.taskId} nudged ${candidate.nudges + 1}/${maxNudges()} (${ceo.id})`,
        { taskId: candidate.taskId, agentId: ceo.id, nudge: candidate.nudges + 1 },
      )
    } catch {
      // Busy / blocked / terminal refused — try again after the cooldown.
      // No budget is spent on a refused delivery; only the timestamp moves.
      touchNudgeCooldown(candidate.taskId, new Date().toISOString())
    }
  }
}

export function startNudgeLoop(): () => void {
  if (process.env.KBOTS_NUDGE === "off") return () => {}
  const timer = setInterval(() => {
    void tick().catch((error: unknown) => {
      console.error(`kbots-daemon: nudge tick failed: ${error instanceof Error ? error.message : String(error)}`)
    })
  }, TICK_MS)
  timer.unref()
  // Prime one pass at boot; failures are logged inside tick.
  void tick().catch(() => {})
  return () => clearInterval(timer)
}
