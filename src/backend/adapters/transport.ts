/**
 * Shared daemon transport helpers: TCP listener address and the token that
 * authorizes TCP requests. Unix sockets keep their existing 0600 file
 * permissions and need no token; TCP (loopback or Tailscale) carries the same
 * secret in a top-level `token` field on every JSON-RPC request.
 */
import { randomBytes } from "node:crypto"
import { mkdir, readFile, writeFile } from "node:fs/promises"
import { homedir } from "node:os"
import { dirname, join } from "node:path"
import { Effect, Option } from "effect"

export const DEFAULT_TCP_ADDR = "127.0.0.1:7878"

/** Same path as the Rust clients' `state_dir()` (HOME, else USERPROFILE). */
export function tokenPath(): string {
  return process.env.KBOTS_TOKEN_FILE?.trim() || join(homedir(), ".local", "state", "kbots", "token")
}

export function stateDir(): string {
  return dirname(tokenPath())
}

/** `--tcp`/`--host` beat env vars; `KBOTS_TCP=1` means the default address. */
export function tcpAddress(override?: string): string | null {
  const raw = override?.trim() || process.env.KBOTS_HOST?.trim() || process.env.KBOTS_TCP?.trim() || process.env.KBOTS_ADDR?.trim()
  if (!raw) return null
  if (/^(1|true|on|yes)$/i.test(raw)) return DEFAULT_TCP_ADDR
  return raw
}

/** Generate the token on first run, reuse it afterwards. */
export const ensureToken = Effect.gen(function* () {
  const path = tokenPath()
  // Missing file is normal (first run); any read error means "generate one".
  const existing = Option.getOrNull(
    yield* Effect.option(
      Effect.tryPromise({
        try: () => readFile(path, "utf8"),
        catch: (error) => error,
      }),
    ),
  )
  if (existing?.trim()) return existing.trim()
  const token = randomBytes(32).toString("hex")
  yield* Effect.tryPromise({
    try: async () => {
      await mkdir(dirname(path), { recursive: true, mode: 0o700 })
      await writeFile(path, `${token}\n`, { mode: 0o600 })
    },
    catch: (error) => error,
  })
  return token
})

/** TCP requests must carry the generated (or explicitly configured) token. */
export function authorized(token: unknown, expected: string): boolean {
  const override = process.env.KBOTS_TOKEN?.trim()
  if (override) return typeof token === "string" && token === override
  return typeof token === "string" && token === expected
}
