import { readFileSync } from "node:fs"
import { join } from "node:path"

import {
  countPendingApprovals,
  countProjects,
  countTasks,
  getTask,
  getTaskIdForAgent,
  listAgents,
} from "../db/repositories.ts"
import { describeBot, isProblem } from "../bots/health.ts"

export const DAEMON_NAME = "kbots-daemon"
// SemVer of this codebase: the stable-release line. ./install.sh stamps each
// deploy with a CalVer version (deploy/YYYY.MM.DD.n tags, written to
// .deploy-version); the daemon reports that instead when one is recorded.
export const DAEMON_VERSION = "0.1.0"

// Resolve the deploy version recorded by the last ./install.sh run. The
// systemd unit runs from the repo root, so resolve relative to this file
// instead of the cwd. KBOTS_DEPLOY_VERSION (e.g. for manual runs) wins over
// the file. Falls back to the SemVer constant when nothing was recorded.
function deployedVersion(): string {
  const env = process.env.KBOTS_DEPLOY_VERSION?.trim()
  if (env) return env
  try {
    const repoRoot = join(import.meta.dir, "..", "..", "..")
    const text = readFileSync(join(repoRoot, ".deploy-version"), "utf8").trim()
    if (text) return text
  } catch {
    // no recorded deploy version — fall back to the SemVer constant
  }
  return DAEMON_VERSION
}

export async function systemStatusHandler(
  input: { socketPath: string; secondarySocketPath?: string; startedAt: number },
): Promise<Record<string, unknown>> {
  const uptimeSeconds = Math.floor((Date.now() - input.startedAt) / 1000)
  const agents = listAgents()
  const byStatus: Record<string, number> = {}
  const byHealth: Record<string, number> = { ok: 0, waiting: 0, warning: 0, error: 0, stopped: 0, unknown: 0 }
  const problems: Array<Record<string, unknown>> = []
  for (const agent of agents) {
    byStatus[agent.status] = (byStatus[agent.status] ?? 0) + 1
    // DB-only diagnosis: system.status is polled every second by clients, so
    // it never probes herdr. `agents.list` carries the probed report.
    const taskId = getTaskIdForAgent(agent.id)
    const report = describeBot(agent, undefined, taskId ? getTask(taskId) : null)
    byHealth[report.health.level] = (byHealth[report.health.level] ?? 0) + 1
    if (!isProblem(report)) continue
    problems.push({
      agentId: agent.id,
      role: agent.role,
      status: agent.status,
      level: report.health.level,
      code: report.health.code,
      message: report.health.message,
      ...(report.task ? { taskId: report.task.taskId } : {}),
    })
  }
  return {
    daemon: {
      name: DAEMON_NAME,
      version: deployedVersion(),
      pid: process.pid,
      uptimeSeconds,
      socketPath: input.socketPath,
      ...(input.secondarySocketPath ? { secondarySocketPath: input.secondarySocketPath } : {}),
    },
    projects: countProjects(),
    tasks: countTasks(),
    pendingApprovals: countPendingApprovals(),
    bots: {
      total: agents.length,
      byStatus,
      byHealth,
      problemCount: problems.length,
      problems: problems.slice(0, 10),
    },
  }
}
