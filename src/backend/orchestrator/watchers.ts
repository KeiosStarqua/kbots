/**
 * File-change watcher: a daemon-internal poller that notices git working-tree
 * changes in watched projects and prompts the managed git agent with the
 * delta once things are quiet and the agent is idle.
 *
 * Design notes:
 * - Per project; rows live in SQLite. A watcher is auto-provisioned (enabled,
 *   defaults) the first time a git agent is seen live without one; an
 *   explicit `kbots prj watch --off` is kept — auto-provision never
 *   overrides an existing row. Configure via RPC (CLI) any time.
 * - Debounce: we report only after the tree has been quiet for
 *   debounce_seconds — never mid-edit-storm.
 * - Cooldown: after delivering a prompt we suppress further deliveries for
 *   cooldown_seconds, so the agent's own edits don't re-trigger prompts
 *   (feedback loop).
 * - Busy/blocked agent: the delta is re-computed on the next tick; nothing
 *   is queued or dropped silently. We never force, never interrupt, and the
 *   prompt goes out only when the agent is idle — same guard as `agents prompt`.
 * - Not a git repo / git missing: the watcher disables itself with an event.
 */
import { createHash } from "node:crypto"
import { execFile } from "node:child_process"
import { promisify } from "node:util"

import {
  addEvent,
  getProject,
  getProjectByPath,
  getWatcher,
  listAgents,
  listEnabledWatchers,
  listWatchers,
  updateWatcherState,
  upsertWatcher,
} from "../db/repositories.ts"
import { RpcError, invalidParams } from "../errors.ts"
import { isLiveAgentStatus } from "../db/repositories.ts"
import { promptAgentCore } from "../bots/agents.ts"

const execFileAsync = promisify(execFile)
const GIT_TIMEOUT_MS = 5_000
const TICK_MS = 1_000
const DEFER_LOG_MS = 60_000
const MAX_PATHS_LIMIT = 50

type GitProbe = { fingerprint: string; paths: string[] } | { error: string }

async function probeWorkingTree(projectPath: string): Promise<GitProbe> {
  try {
    const { stdout } = await execFileAsync(
      "git",
      ["-C", projectPath, "status", "--porcelain=v1", "--untracked-files=normal"],
      { timeout: GIT_TIMEOUT_MS, maxBuffer: 8 * 1024 * 1024 },
    )
    const lines = stdout
      .split("\n")
      .map((line) => line.trimEnd())
      .filter((line) => line.length > 0)
      .sort()
    const paths = lines.map((line) => {
      const raw = line.slice(3).trim()
      const arrow = raw.indexOf(" -> ")
      return arrow === -1 ? raw : raw.slice(arrow + 4)
    })
    return { fingerprint: createHash("sha256").update(lines.join("\n")).digest("hex"), paths }
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    return { error: message }
  }
}

function resolveProject(params: Record<string, unknown>) {
  const ref = typeof params.project === "string" && params.project.trim() ? params.project.trim() : ""
  if (ref) {
    const project = getProject(ref)
    if (!project) throw invalidParams(`unknown project: ${ref}`)
    return project
  }
  const cwd = process.cwd()
  const project = getProjectByPath(cwd)
  if (!project) {
    throw invalidParams("no project in the current directory; pass project or run inside a registered project")
  }
  return project
}

function clampInt(raw: unknown, field: string, min: number, max: number): number | undefined {
  if (raw === undefined || raw === null) return undefined
  const value = typeof raw === "number" ? raw : Number(raw)
  if (!Number.isFinite(value) || value < min || value > max) {
    throw invalidParams(`${field} must be a number between ${min} and ${max}`)
  }
  return Math.floor(value)
}

function watcherPayload(row: import("../db/repositories.ts").WatcherRow): Record<string, unknown> {
  const project = getProject(row.projectId)
  return {
    projectId: row.projectId,
    ...(project ? { projectName: project.name, projectPath: project.path } : {}),
    enabled: row.enabled,
    intervalSeconds: row.intervalSeconds,
    debounceSeconds: row.debounceSeconds,
    cooldownSeconds: row.cooldownSeconds,
    maxPaths: row.maxPaths,
    lastChangeAt: row.lastChangeAt,
    lastDeliveredAt: row.lastDeliveredAt,
  }
}

export async function watchersListHandler(): Promise<Record<string, unknown>> {
  return { items: listWatchers().map(watcherPayload) }
}

export async function watchersSetHandler(params: Record<string, unknown>): Promise<Record<string, unknown>> {
  const project = resolveProject(params)
  const enabled = typeof params.enabled === "boolean" ? params.enabled : params.enabled === "false" ? false : true
  const row = upsertWatcher({
    projectId: project.projectId,
    enabled,
    intervalSeconds: clampInt(params.intervalSeconds, "intervalSeconds", 1, 3600),
    debounceSeconds: clampInt(params.debounceSeconds, "debounceSeconds", 1, 3600),
    cooldownSeconds: clampInt(params.cooldownSeconds, "cooldownSeconds", 0, 86400),
    maxPaths: clampInt(params.maxPaths, "maxPaths", 1, MAX_PATHS_LIMIT),
  })
  addEvent("watch.configured", `Watcher for ${project.name} ${enabled ? "enabled" : "disabled"}`, {
    projectId: project.projectId,
    enabled,
  })
  return watcherPayload(row)
}

/**
 * Auto-provision: a live git agent with no watcher row gets a default
 * enabled watcher. Only when the row is absent — an explicit `watchers.set`
 * (e.g. `kbots prj watch --off`) always wins and is never overridden.
 */
function autoProvisionWatchers(): void {
  for (const agent of listAgents()) {
    if (agent.role !== "git" || !isLiveAgentStatus(agent.status)) continue
    if (getWatcher(agent.projectId)) continue
    const project = getProject(agent.projectId)
    if (!project) continue
    upsertWatcher({ projectId: agent.projectId, enabled: true })
    addEvent("watch.auto_enabled", `Watcher for ${project.name} auto-enabled (git agent ${agent.id} live)`, {
      projectId: agent.projectId,
      agentId: agent.id,
    })
  }
}

const lastPollAt = new Map<string, number>()
const lastDeferLogAt = new Map<string, number>()
const lastAttemptAt = new Map<string, number>()
const RETRY_ATTEMPT_MS = 15_000

function buildPrompt(projectName: string, projectId: string, paths: string[], total: number, maxPaths: number): string {
  const shown = paths.slice(0, maxPaths)
  const remainder = total - shown.length
  const lines = [
    `Working tree changed in ${projectName} (${projectId}): ${total} file(s):`,
    ...shown,
  ]
  if (remainder > 0) lines.push(`… and ${remainder} more`)
  lines.push("Review the changes and follow this project's git bot instructions.")
  return lines.join("\n")
}

async function tick(): Promise<void> {
  const now = Date.now()
  try {
    autoProvisionWatchers()
  } catch (error) {
    console.error(`kbots-daemon: watcher auto-provision failed: ${error instanceof Error ? error.message : String(error)}`)
  }
  for (const watcher of listEnabledWatchers()) {
    try {
      const lastPoll = lastPollAt.get(watcher.projectId) ?? 0
      if (now - lastPoll < watcher.intervalSeconds * 1_000) continue
      lastPollAt.set(watcher.projectId, now)

      const project = getProject(watcher.projectId)
      if (!project) {
        updateWatcherState(watcher.projectId, { enabled: false })
        addEvent("watch.disabled", `Watcher for missing project ${watcher.projectId} disabled`, {
          projectId: watcher.projectId,
        })
        continue
      }

      const probe = await probeWorkingTree(project.path)
      if ("error" in probe) {
        updateWatcherState(watcher.projectId, { enabled: false })
        addEvent("watch.disabled", `Watcher for ${project.name} disabled: ${probe.error}`, {
          projectId: project.projectId,
          error: probe.error,
        })
        continue
      }

      const changedAt = watcher.lastChangeAt
      if (probe.fingerprint !== watcher.observedFingerprint) {
        updateWatcherState(watcher.projectId, { observedFingerprint: probe.fingerprint, lastChangeAt: new Date(now).toISOString() })
        continue
      }

      const quietFor = changedAt ? now - Date.parse(changedAt) : Number.POSITIVE_INFINITY
      if (quietFor < watcher.debounceSeconds * 1_000) continue
      if (probe.fingerprint === watcher.reportedFingerprint) continue

      if (watcher.lastDeliveredAt && now - Date.parse(watcher.lastDeliveredAt) < watcher.cooldownSeconds * 1_000) {
        continue // cooldown after our own delivery — avoid prompt ping-pong
      }

      const lastAttempt = lastAttemptAt.get(watcher.projectId) ?? 0
      if (now - lastAttempt < RETRY_ATTEMPT_MS) continue
      lastAttemptAt.set(watcher.projectId, now)

      const agent = listAgents().find(
        (candidate) => candidate.projectId === watcher.projectId && candidate.role === "git" && isLiveAgentStatus(candidate.status),
      )
      if (!agent) {
        updateWatcherState(watcher.projectId, { reportedFingerprint: probe.fingerprint })
        addEvent("watch.changed", `Working tree changed in ${project.name}; no running git agent`, {
          projectId: project.projectId,
          delivered: false,
          filesChanged: probe.paths.length,
        })
        continue
      }

      const text = buildPrompt(project.name, project.projectId, probe.paths, probe.paths.length, watcher.maxPaths)
      try {
        await promptAgentCore(agent, text)
        updateWatcherState(watcher.projectId, {
          reportedFingerprint: probe.fingerprint,
          lastDeliveredAt: new Date().toISOString(),
        })
        addEvent("watch.changed", `Working tree changed in ${project.name}; prompted ${agent.id}`, {
          projectId: project.projectId,
          agentId: agent.id,
          delivered: true,
          filesChanged: probe.paths.length,
          samplePaths: probe.paths.slice(0, watcher.maxPaths),
        })
      } catch (error) {
        // Busy/blocked (or another transient refusal): keep the fingerprint
        // unreported so we retry once the agent is idle — do not drop it.
        const lastDefer = lastDeferLogAt.get(watcher.projectId) ?? 0
        if (now - lastDefer > DEFER_LOG_MS) {
          lastDeferLogAt.set(watcher.projectId, now)
          addEvent("watch.deferred", `Working tree changed in ${project.name}; git agent busy or blocked`, {
            projectId: project.projectId,
            agentId: agent.id,
            reason: error instanceof Error ? error.message : String(error),
          })
        }
      }
    } catch (error) {
      console.error(`kbots-daemon: watcher tick failed: ${error instanceof Error ? error.message : String(error)}`)
    }
  }
}

export function startWatcherLoop(): () => void {
  const timer = setInterval(() => {
    void tick().catch((error: unknown) => {
      console.error(`kbots-daemon: watcher tick failed: ${error instanceof Error ? error.message : String(error)}`)
    })
  }, TICK_MS)
  timer.unref()
  // Prime one pass at boot; errors are logged inside tick.
  void tick().catch(() => {})
  return () => clearInterval(timer)
}
