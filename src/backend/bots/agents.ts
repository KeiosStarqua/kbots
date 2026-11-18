import { createHash } from "node:crypto"
import { readdir, readFile } from "node:fs/promises"
import { homedir } from "node:os"
import { isAbsolute, join } from "node:path"
import { listProjects } from "../db/repositories.ts"
import type { ProjectRow } from "../db/repositories.ts"

type Role = "ceo" | "git" | "ops"
type Manifest = {
  version: 1
  bots: Record<string, { enabled?: boolean }>
}
type SessionState = {
  version: 1
  projectPath: string
  sessions: Partial<Record<Role, string>>
}
type AgentItem = {
  projectId?: string
  projectPath: string
  projectName?: string
  role: string
  enabled: boolean | null
  sessionID: string | null
  directory: string
  registered: boolean
  manifestError?: string
}

const roles: Role[] = ["ceo", "git", "ops"]
const maxStateFiles = 200

function stateRoot(): string {
  return join(process.env.XDG_STATE_HOME ?? join(homedir(), ".local/state"), "kbots/projects")
}

function sha256hex(value: string): string {
  return createHash("sha256").update(value).digest("hex")
}

function defaultBots(): Record<Role, { enabled: boolean }> {
  return { ceo: { enabled: true }, git: { enabled: true }, ops: { enabled: true } }
}

async function readManifestBots(
  projectPath: string,
): Promise<{ bots: Record<string, { enabled?: boolean }>; manifestError?: string }> {
  try {
    const path = join(projectPath, ".agents/bot/kbots.json")
    const parsed: unknown = JSON.parse(await readFile(path, "utf8"))
    if (!parsed || typeof parsed !== "object") {
      throw new Error("manifest must be an object")
    }
    const manifest = parsed as Manifest
    if (manifest.version !== 1) {
      throw new Error("manifest must be version 1")
    }
    if (!manifest.bots || typeof manifest.bots !== "object") {
      throw new Error("manifest must define bots")
    }
    return { bots: manifest.bots }
  } catch (error) {
    const manifestError = error instanceof Error ? error.message : String(error)
    return { bots: defaultBots(), manifestError }
  }
}

function sessionIDFromState(state: unknown, role: Role): string | null {
  if (!state || typeof state !== "object") return null
  const sessions = (state as SessionState).sessions
  if (!sessions || typeof sessions !== "object") return null
  const value = sessions[role]
  return typeof value === "string" ? value : null
}

export async function agentsListHandler(): Promise<Record<string, unknown>> {
  const registry = listProjects()
  const hashMap = new Map<string, ProjectRow>()
  for (const project of registry) {
    hashMap.set(sha256hex(project.path), project)
  }

  const items: AgentItem[] = []

  for (const project of registry) {
    const { bots, manifestError } = await readManifestBots(project.path)
    const stateFile = join(stateRoot(), `${sha256hex(project.path)}.json`)
    let state: unknown = null
    try {
      state = JSON.parse(await readFile(stateFile, "utf8"))
    } catch {
      state = null
    }
    for (const role of roles) {
      items.push({
        projectId: project.projectId,
        projectPath: project.path,
        projectName: project.name,
        role,
        enabled: bots[role]?.enabled !== false,
        sessionID: sessionIDFromState(state, role),
        directory: join(project.path, ".agents/bot", role),
        registered: true,
        ...(manifestError ? { manifestError } : {}),
      })
    }
  }

  const root = stateRoot()
  let stateFiles: string[] = []
  try {
    stateFiles = (await readdir(root)).filter((entry) => entry.endsWith(".json")).slice(0, maxStateFiles)
  } catch {
    stateFiles = []
  }
  for (const file of stateFiles) {
    const hash = file.slice(0, -".json".length)
    if (hashMap.has(hash)) continue
    let state: SessionState
    try {
      const parsed: unknown = JSON.parse(await readFile(join(root, file), "utf8"))
      if (!parsed || typeof parsed !== "object") continue
      state = parsed as SessionState
    } catch {
      continue
    }
    if (typeof state.projectPath !== "string" || !isAbsolute(state.projectPath)) continue
    if (!state.sessions || typeof state.sessions !== "object") continue
    for (const role of roles) {
      const sessionID = state.sessions[role]
      if (typeof sessionID !== "string") continue
      items.push({
        projectPath: state.projectPath,
        role,
        enabled: null,
        sessionID,
        directory: join(state.projectPath, ".agents/bot", role),
        registered: false,
      })
    }
  }

  const roleOrder = new Map<string, number>(roles.map((role, index) => [role, index]))
  items.sort((a, b) => {
    const byPath = a.projectPath.localeCompare(b.projectPath)
    if (byPath !== 0) return byPath
    return (roleOrder.get(a.role) ?? roles.length) - (roleOrder.get(b.role) ?? roles.length)
  })

  return {
    items,
    registeredProjects: registry.length,
    generatedAt: new Date().toISOString(),
  }
}
