import { readFileSync } from "node:fs"
import { join } from "node:path"

import { countPendingApprovals, countProjects, countTasks } from "../db/repositories.ts"

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
  }
}
