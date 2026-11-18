/**
 * Herdr adapter: read-only access to live terminals via `hc ls`.
 * Awaitable failures are surfaced by callers (never thrown into the daemon).
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

const execFileAsync = promisify(execFile)
const DEFAULT_TIMEOUT_MS = 5_000

function candidates(): string[] {
  if (process.env.HC_BIN) return [process.env.HC_BIN]
  return ["hc", join(homedir(), ".local/bin", "hc")]
}

async function runHc(args: string[], timeoutMs: number): Promise<string> {
  let lastError: unknown
  for (const bin of candidates()) {
    try {
      const { stdout } = await execFileAsync(bin, args, { timeout: timeoutMs, maxBuffer: 4 * 1024 * 1024 })
      return stdout
    } catch (error) {
      lastError = error
      // Only fall through to the next candidate when the binary is missing;
      // other failures (non-zero exit, timeout) are real errors.
      const code = (error as NodeJS.ErrnoException).code
      if (code !== "ENOENT") throw error
    }
  }
  throw lastError instanceof Error ? lastError : new Error("hc binary not found")
}

export async function listLiveTerminals(timeoutMs: number = DEFAULT_TIMEOUT_MS): Promise<HerdrTerminal[]> {
  const stdout = await runHc(["ls"], timeoutMs)
  const parsed: unknown = JSON.parse(stdout)
  if (!parsed || typeof parsed !== "object" || !Array.isArray((parsed as { terminals?: unknown }).terminals)) {
    throw new Error("hc ls returned an unexpected shape")
  }
  const terminals = (parsed as { terminals: unknown[] }).terminals
  const result: HerdrTerminal[] = []
  for (const entry of terminals) {
    if (!entry || typeof entry !== "object") continue
    const terminal = entry as Record<string, unknown>
    if (typeof terminal.address !== "string" || typeof terminal.cwd !== "string") continue
    result.push({
      address: terminal.address,
      cwd: terminal.cwd,
      ...(typeof terminal.agent === "string" ? { agent: terminal.agent } : {}),
      ...(typeof terminal.name === "string" ? { name: terminal.name } : {}),
      ...(typeof terminal.state === "string" ? { state: terminal.state } : {}),
      ...(typeof terminal.status === "string" ? { status: terminal.status } : {}),
      ...(typeof terminal.turn === "number" ? { turn: terminal.turn } : {}),
    })
  }
  return result
}
