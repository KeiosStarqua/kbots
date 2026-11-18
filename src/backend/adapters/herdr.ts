/**
 * Herdr adapter: read-only plus lifecycle access to the local hc core via
 * `hc` subprocesses. The interface is intentionally narrow so a later phase
 * can swap subprocesses for one persistent framed JSON-RPC connection
 * (events.subscribe) without touching call sites.
 *
 * Exit codes (see `hc --skill`): 3 not_found, 9 blocked, 10 status_mismatch.
 */
import { execFile } from "node:child_process"
import { homedir } from "node:os"
import { join } from "node:path"
import { promisify } from "node:util"

export interface HerdrTerminal {
  address: string
  agent?: string
  name?: string
  cwd: string
  state?: string
  status?: string
  turn?: number
}

export interface HerdrFullTerminal {
  address: string
  name: string | null
  argv: string[]
  cwd: string
  state: string
  status: { app?: string; state?: string; kind?: string | null } | null
  turn: number
  labels: Record<string, string>
  pid: number | null
  exitCode: number | null
  reason: string | null
}

export class HerdrError extends Error {
  constructor(
    public readonly code: number | null,
    message: string,
  ) {
    super(message)
    this.name = "HerdrError"
  }
}

export class HerdrBusyError extends Error {
  constructor(message: string) {
    super(message)
    this.name = "HerdrBusyError"
  }
}

const execFileAsync = promisify(execFile)
const DEFAULT_TIMEOUT_MS = 5_000
const SPAWN_TIMEOUT_MS = 15_000

function candidates(): string[] {
  if (process.env.HC_BIN) return [process.env.HC_BIN]
  return ["hc", join(homedir(), ".local/bin", "hc")]
}

function stderrOf(error: unknown): string {
  const anyError = error as { stderr?: unknown; message?: unknown }
  if (typeof anyError?.stderr === "string" && anyError.stderr.trim().length > 0) return anyError.stderr.trim()
  if (error instanceof Error) return error.message
  return String(error)
}

async function execHc(args: string[], timeoutMs: number): Promise<string> {
  let lastError: HerdrError | undefined
  for (const bin of candidates()) {
    try {
      const { stdout } = await execFileAsync(bin, args, { timeout: timeoutMs, maxBuffer: 8 * 1024 * 1024 })
      return stdout
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code
      const asNumber = typeof code === "number" ? code : null
      const asString = typeof code === "string" ? code : null
      if (asString === "ENOENT" || code === undefined) {
        lastError = new HerdrError(null, `hc binary not found: ${bin}`)
        continue
      }
      throw new HerdrError(asNumber, stderrOf(error))
    }
  }
  throw lastError ?? new HerdrError(null, "hc binary not found")
}

function parseJson(stdout: string, what: string): unknown {
  try {
    return JSON.parse(stdout)
  } catch {
    throw new HerdrError(null, `hc returned non-JSON output for ${what}`)
  }
}

function toFullTerminal(raw: unknown): HerdrFullTerminal | null {
  if (!raw || typeof raw !== "object") return null
  const terminal = raw as Record<string, unknown>
  if (typeof terminal.address !== "string" || typeof terminal.cwd !== "string") return null
  const status =
    terminal.status && typeof terminal.status === "object"
      ? (terminal.status as { app?: string; state?: string; kind?: string | null })
      : null
  const labels =
    terminal.labels && typeof terminal.labels === "object" && !Array.isArray(terminal.labels)
      ? (terminal.labels as Record<string, string>)
      : {}
  return {
    address: terminal.address,
    name: typeof terminal.name === "string" ? terminal.name : null,
    argv: Array.isArray(terminal.argv) ? terminal.argv.filter((item): item is string => typeof item === "string") : [],
    cwd: terminal.cwd,
    state: typeof terminal.state === "string" ? terminal.state : "unknown",
    status,
    turn: typeof terminal.turn === "number" ? terminal.turn : 0,
    labels,
    pid: typeof terminal.pid === "number" ? terminal.pid : null,
    exitCode: typeof terminal.exit_code === "number" ? terminal.exit_code : null,
    reason: typeof terminal.reason === "string" ? terminal.reason : null,
  }
}

/** Read-only live terminals (used by agents.list). */
export async function listLiveTerminals(timeoutMs: number = DEFAULT_TIMEOUT_MS): Promise<HerdrTerminal[]> {
  const stdout = await execHc(["ls"], timeoutMs)
  const parsed = parseJson(stdout, "ls") as { terminals?: unknown }
  if (!Array.isArray(parsed.terminals)) {
    throw new HerdrError(null, "hc ls returned an unexpected shape")
  }
  const result: HerdrTerminal[] = []
  for (const entry of parsed.terminals) {
    if (!entry || typeof entry !== "object") continue
    const terminal = entry as Record<string, unknown>
    if (typeof terminal.address !== "string" || typeof terminal.cwd !== "string") continue
    result.push({
      address: terminal.address,
      cwd: terminal.cwd,
      ...(typeof terminal.agent === "string" ? { agent: terminal.agent } : {}),
      ...(typeof terminal.name === "string" ? { name: terminal.name } : {}),
      ...(typeof terminal.state === "string" ? { state: terminal.state } : {}),
      ...(typeof terminal.status === "string"
        ? { status: terminal.status }
        : terminal.status && typeof terminal.status === "object" && typeof (terminal.status as { state?: unknown }).state === "string"
          ? { status: (terminal.status as { state: string }).state }
          : {}),
      ...(typeof terminal.turn === "number" ? { turn: terminal.turn } : {}),
    })
  }
  return result
}

/** All terminals (live and finished) carrying all these labels. */
export async function listTerminalsByLabels(
  labels: Record<string, string>,
  timeoutMs: number = DEFAULT_TIMEOUT_MS,
): Promise<HerdrFullTerminal[]> {
  const args = ["ls", "--all"]
  for (const [key, value] of Object.entries(labels)) {
    args.push("--label", `${key}=${value}`)
  }
  const stdout = await execHc(args, timeoutMs)
  const parsed = parseJson(stdout, "ls --all --label") as { terminals?: unknown }
  if (!Array.isArray(parsed.terminals)) {
    throw new HerdrError(null, "hc ls --label returned an unexpected shape")
  }
  return parsed.terminals
    .map(toFullTerminal)
    .filter((terminal): terminal is HerdrFullTerminal => terminal !== null)
}

/** Spawn a program in a new background terminal. Throws HerdrError on failure. */
export async function runTerminal(
  input: {
    argv: string[]
    cwd: string
    name?: string
    labels: Record<string, string>
    keep?: boolean
  },
  timeoutMs: number = SPAWN_TIMEOUT_MS,
): Promise<HerdrFullTerminal> {
  const args = ["run", "--bg", "--cwd", input.cwd, "--no-parent", "--no-notify"]
  for (const [key, value] of Object.entries(input.labels)) {
    args.push("--label", `${key}=${value}`)
  }
  if (input.name) args.push("--name", input.name)
  if (input.keep) args.push("--keep")
  args.push(...input.argv)
  const stdout = await execHc(args, timeoutMs)
  const terminal = toFullTerminal(parseJson(stdout, "run"))
  if (!terminal) throw new HerdrError(null, "hc run returned an unexpected shape")
  return terminal
}

/** One terminal's facts, or null when it no longer exists. */
export async function getTerminal(
  address: string,
  timeoutMs: number = DEFAULT_TIMEOUT_MS,
): Promise<HerdrFullTerminal | null> {
  try {
    const stdout = await execHc(["get", address], timeoutMs)
    return toFullTerminal(parseJson(stdout, "get"))
  } catch (error) {
    if (error instanceof HerdrError && error.code === 3) return null
    throw error
  }
}

/** End the terminal's program (hangup, then escalate). */
export async function killTerminal(
  address: string,
  timeoutMs: number = DEFAULT_TIMEOUT_MS,
): Promise<void> {
  await execHc(["kill", address], timeoutMs)
}

export interface HerdrInterruptResult {
  key: string
  outcome: string
}

export async function interruptTerminal(
  address: string,
  timeoutSeconds = 10,
  timeoutMs: number = DEFAULT_TIMEOUT_MS + timeoutSeconds * 1000,
): Promise<HerdrInterruptResult> {
  const stdout = await execHc(["interrupt", "--timeout", String(timeoutSeconds), address], timeoutMs)
  const parsed = parseJson(stdout, "interrupt") as Record<string, unknown>
  return {
    key: typeof parsed.key === "string" ? parsed.key : "unknown",
    outcome: typeof parsed.outcome === "string" ? parsed.outcome : "unknown",
  }
}

/**
 * Paste a prompt and submit it, only if the program is idle. Atomic
 * check-and-send: hc refuses with status_mismatch (exit 10) when the program
 * is not in the expected status, and blocked (exit 9) for blocked programs.
 */
export async function sendPromptIfIdle(
  address: string,
  text: string,
  timeoutMs: number = DEFAULT_TIMEOUT_MS,
): Promise<{ turn: number | null; status: string | null }> {
  try {
    const stdout = await execHc(["send", "--if", "idle", address, text], timeoutMs)
    const parsed = parseJson(stdout, "send") as Record<string, unknown>
    return {
      turn: typeof parsed.turn === "number" ? parsed.turn : null,
      status: typeof parsed.status === "string" ? parsed.status : null,
    }
  } catch (error) {
    if (error instanceof HerdrError && (error.code === 10 || error.code === 9)) {
      throw new HerdrBusyError(error.message)
    }
    throw error
  }
}
