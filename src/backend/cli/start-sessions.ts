import { isSessionNotFoundError, OpenCode } from "@opencode/client"
import { Service } from "@opencode/client/service"
import { Effect } from "effect"
import { createHash } from "node:crypto"
import { mkdir, readFile, realpath, rename, writeFile } from "node:fs/promises"
import { homedir } from "node:os"
import { dirname, resolve } from "node:path"

type Role = "ceo" | "git" | "ops"
type Manifest = {
  version: 1
  bots: Record<Role, { enabled?: boolean; opencode?: Record<string, unknown> }>
}
type SessionState = { version: 1; projectPath: string; sessions: Partial<Record<Role, string>> }

const roles: Role[] = ["ceo", "git", "ops"]

function option(name: string): string | undefined {
  const index = process.argv.indexOf(name)
  return index === -1 ? undefined : process.argv[index + 1]
}

function statePath(projectPath: string): string {
  const stateRoot = process.env.XDG_STATE_HOME ?? resolve(homedir(), ".local/state")
  const key = createHash("sha256").update(projectPath).digest("hex")
  return resolve(stateRoot, "kbots/projects", `${key}.json`)
}

async function readManifest(projectPath: string): Promise<Manifest> {
  const path = resolve(projectPath, ".agents/bot/kbots.json")
  const parsed: unknown = JSON.parse(await readFile(path, "utf8"))
  if (!parsed || typeof parsed !== "object" || (parsed as Manifest).version !== 1) {
    throw new Error(`${path} must contain a version 1 kbots manifest`)
  }
  const manifest = parsed as Manifest
  if (!manifest.bots || roles.some((role) => !manifest.bots[role])) {
    throw new Error(`${path} must define ceo, git, and ops bot entries`)
  }
  return manifest
}

async function readState(path: string, projectPath: string): Promise<SessionState> {
  try {
    const state = JSON.parse(await readFile(path, "utf8")) as SessionState
    if (state.version !== 1 || state.projectPath !== projectPath || !state.sessions) {
      throw new Error(`Invalid session registry at ${path}`)
    }
    return state
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") {
      return { version: 1, projectPath, sessions: {} }
    }
    throw error
  }
}

async function persistState(path: string, state: SessionState): Promise<void> {
  await mkdir(dirname(path), { recursive: true })
  const temporaryPath = `${path}.${process.pid}.tmp`
  await writeFile(temporaryPath, `${JSON.stringify(state, null, 2)}\n`, { mode: 0o600 })
  await rename(temporaryPath, path)
}

async function startAll(): Promise<void> {
  const projectArg = option("--directory") ?? process.cwd()
  const projectPath = resolve(await realpath(projectArg))
  const manifest = await readManifest(projectPath)
  const enabledRoles = roles.filter((role) => manifest.bots[role].enabled !== false)
  if (enabledRoles.length === 0) {
    console.log(JSON.stringify({ projectPath, started: [], reused: [], disabled: roles }, null, 2))
    return
  }

  const path = statePath(projectPath)
  const state = await readState(path, projectPath)
  const endpoint = await Service.ensure()
  const client = OpenCode.make({ baseUrl: endpoint.url, headers: Service.headers(endpoint) })
  const started: { role: Role; sessionID: string; directory: string }[] = []
  const reused: { role: Role; sessionID: string; directory: string }[] = []

  for (const role of enabledRoles) {
    const directory = resolve(projectPath, ".agents/bot", role)
    let sessionID = state.sessions[role]
    if (sessionID) {
      let existing
      try {
        existing = await client.session.get({ sessionID })
      } catch (error) {
        if (isSessionNotFoundError(error)) sessionID = undefined
        else throw error
      }
      if (existing && resolve(existing.location.directory) !== directory) {
        throw new Error(`Saved ${role} session ${sessionID} belongs to ${existing.location.directory}`)
      }
    }

    if (sessionID) {
      reused.push({ role, sessionID, directory })
      continue
    }

    const session = await client.session.create({
      title: `kbots ${role.toUpperCase()} · ${projectPath.split("/").at(-1) ?? projectPath}`,
      location: { directory },
      metadata: { kbotsProjectPath: projectPath, kbotsRole: role },
    })
    state.sessions[role] = session.id
    started.push({ role, sessionID: session.id, directory })
    await persistState(path, state)
  }

  console.log(JSON.stringify({ projectPath, stateFile: path, started, reused, disabled: roles.filter((role) => !enabledRoles.includes(role)) }, null, 2))
}

Effect.runPromise(Effect.tryPromise({
  try: startAll,
  catch: (cause) => cause instanceof Error ? cause : new Error(String(cause)),
})).catch((error: unknown) => {
  console.error(`kbots sessions start: ${error instanceof Error ? error.message : String(error)}`)
  process.exitCode = 1
})
