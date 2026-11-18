import { countPendingApprovals, countProjects, countTasks } from "../db/repositories.ts"

export const DAEMON_NAME = "kbots-daemon"
export const DAEMON_VERSION = "0.1.0"

export async function systemStatusHandler(
  input: { socketPath: string; startedAt: number },
): Promise<Record<string, unknown>> {
  const uptimeSeconds = Math.floor((Date.now() - input.startedAt) / 1000)
  return {
    daemon: {
      name: DAEMON_NAME,
      version: DAEMON_VERSION,
      pid: process.pid,
      uptimeSeconds,
      socketPath: input.socketPath,
    },
    projects: countProjects(),
    tasks: countTasks(),
    pendingApprovals: countPendingApprovals(),
  }
}
