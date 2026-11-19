/**
 * kbots workflow daemon (v1, minimal).
 *
 * Owns workflow state through SQLite and exposes the local JSON-RPC 2.0
 * contract from docs/kbots-ipc.md over a Unix domain socket. One newline-
 * delimited request per connection; one newline-delimited response.
 *
 * v2 scope: task records with auto-dispatch to per-task OpenCode CEO
 * sessions, project registry, approvals log, event log, and managed agent
 * lifecycle (agents.* / reconcileAgents). One request/response per connection.
 *
 * Start manually: `bun run daemon` (or `bun run src/backend/daemon.ts`).
 * Socket resolution (same order as the CLI): --socket PATH, KBOTS_SOCKET,
 * $XDG_RUNTIME_DIR/kbots.sock, $HOME/.local/run/kbots.sock.
 * Unless --socket/KBOTS_SOCKET is explicit and the primary lands in
 * $XDG_RUNTIME_DIR, the daemon also binds a secondary listener on
 * $HOME/.local/run/kbots.sock so sandboxed agents (whose AF_UNIX connect to
 * /run/user/* is denied) can reach it through the fallback path. The
 * secondary listener is skipped when another daemon already serves it.
 */
import { chmod, mkdir, unlink } from "node:fs/promises"
import { homedir } from "node:os"
import { dirname, join } from "node:path"
import { connect, createServer, type Server, type Socket } from "node:net"

import { RpcError } from "./errors.ts"
import { registerProjectHandler, listProjectsHandler, getProjectHandler } from "./orchestrator/projects.ts"
import {
  createTaskHandler,
  listTasksHandler,
  getTaskHandler,
  cancelTaskHandler,
  tasksUpdateHandler,
  tasksMessageHandler,
  tasksMessagesHandler,
  tasksBriefHandler,
  tasksInboxHandler,
} from "./orchestrator/tasks.ts"
import { routeRequest } from "./orchestrator/router.ts"
import { listApprovalsHandler, approveApprovalHandler } from "./orchestrator/approvals.ts"
import { listEventsHandler } from "./orchestrator/events.ts"
import { systemStatusHandler, DAEMON_NAME } from "./orchestrator/system.ts"
import {
  agentsListHandler,
  agentsStartHandler,
  agentsStopHandler,
  agentsInterruptHandler,
  agentsPromptHandler,
  agentsShowHandler,
  reconcileAgents,
} from "./bots/agents.ts"
import {
  startWatcherLoop,
  watchersListHandler,
  watchersSetHandler,
} from "./orchestrator/watchers.ts"
import { startNudgeLoop } from "./orchestrator/nudger.ts"

const MAX_REQUEST_BYTES = 1024 * 1024
const SOCKET_TIMEOUT_MS = 10_000
const ROUTER_SOCKET_TIMEOUT_MS = 120_000
// Global guard: a single client (or a bug) can never exhaust sockets file
// descriptors or the event loop. Override with KBOTS_MAX_CONNECTIONS.
const MAX_CONCURRENT_CONNECTIONS = (() => {
  const raw = Number.parseInt(process.env.KBOTS_MAX_CONNECTIONS ?? "", 10)
  return Number.isFinite(raw) && raw > 0 ? raw : 256
})()

let activeConnections = 0
let lastBusyLogAt = 0

function acceptConnection(socket: Socket, handlers: Record<string, Handler>): void {
  if (activeConnections >= MAX_CONCURRENT_CONNECTIONS) {
    writeJson(socket, rpcError(null, -32000, "server busy: too many concurrent connections"))
    const now = Date.now()
    if (now - lastBusyLogAt > 10_000) {
      lastBusyLogAt = now
      console.error(`${DAEMON_NAME}: rejected connection: at capacity (${MAX_CONCURRENT_CONNECTIONS})`)
    }
    return
  }
  activeConnections += 1
  socket.once("close", () => {
    activeConnections -= 1
  })
  void handleConnection(socket, handlers)
}

type Handler = (params: Record<string, unknown>) => Promise<unknown>

const startedAt = Date.now()

function parseArgs(argv: string[]): { socket?: string } {
  const index = argv.indexOf("--socket")
  if (index === -1) return {}
  const value = argv[index + 1]
  if (!value) throw new Error("--socket requires a path")
  return { socket: value }
}

function socketPath(override?: string): string {
  if (override) return override
  if (process.env.KBOTS_SOCKET) return process.env.KBOTS_SOCKET
  if (process.env.XDG_RUNTIME_DIR) return join(process.env.XDG_RUNTIME_DIR, "kbots.sock")
  return join(process.env.HOME ?? homedir(), ".local", "run", "kbots.sock")
}

// The fallback socket sandboxed agents can reach when /run/user/* is denied.
// Not bound when the user picked the socket explicitly (--socket / KBOTS_SOCKET)
// or the primary already is the fallback.
function fallbackSocketPath(override: string | undefined, primary: string): string | null {
  if (override || process.env.KBOTS_SOCKET) return null
  const home = join(process.env.HOME ?? homedir(), ".local", "run", "kbots.sock")
  return home === primary ? null : home
}

// True when something is already serving this socket (do not steal it).
function socketInUse(path: string): Promise<boolean> {
  return new Promise((resolvePromise) => {
    const probe = connect(path)
    probe.once("connect", () => {
      probe.end()
      resolvePromise(true)
    })
    probe.once("error", () => resolvePromise(false))
  })
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function buildHandlers(socket: string, secondarySocket?: string): Record<string, Handler> {
  return {
    "system.status": () => systemStatusHandler({ socketPath: socket, secondarySocketPath: secondarySocket, startedAt }),
    "tasks.create": createTaskHandler,
    "tasks.list": listTasksHandler,
    "tasks.get": getTaskHandler,
    "tasks.cancel": cancelTaskHandler,
    "tasks.update": tasksUpdateHandler,
    "tasks.message": tasksMessageHandler,
    "tasks.messages": tasksMessagesHandler,
    "tasks.brief": tasksBriefHandler,
    "tasks.inbox": tasksInboxHandler,
    "router.run": routeRequest,
    "projects.register": registerProjectHandler,
    "projects.list": () => listProjectsHandler(),
    "projects.get": getProjectHandler,
    "agents.list": agentsListHandler,
    "agents.start": agentsStartHandler,
    "agents.show": agentsShowHandler,
    "agents.stop": agentsStopHandler,
    "agents.prompt": agentsPromptHandler,
    "agents.interrupt": agentsInterruptHandler,
    "watchers.list": watchersListHandler,
    "watchers.set": watchersSetHandler,
    "approvals.list": listApprovalsHandler,
    "approvals.approve": approveApprovalHandler,
    "events.list": listEventsHandler,
  }
}

function writeJson(socket: Socket, payload: Record<string, unknown>): void {
  socket.end(`${JSON.stringify(payload)}\n`)
}

function rpcError(id: unknown, code: number, message: string): Record<string, unknown> {
  return { jsonrpc: "2.0", id: id ?? null, error: { code, message } }
}

async function handleConnection(socket: Socket, handlers: Record<string, Handler>): Promise<void> {
  socket.setTimeout(SOCKET_TIMEOUT_MS, () => socket.destroy())
  const chunks: Buffer[] = []
  let received = 0

  socket.on("data", (chunk: Buffer) => {
    received += chunk.length
    if (received > MAX_REQUEST_BYTES) {
      writeJson(socket, rpcError(null, -32600, "request too large"))
      socket.destroy()
      return
    }
    chunks.push(chunk)
    const text = Buffer.concat(chunks).toString("utf8")
    const newline = text.indexOf("\n")
    if (newline === -1) return

    socket.removeAllListeners("data")
    const line = text.slice(0, newline).trim()

    void (async () => {
      let id: unknown = null
      try {
        const request: unknown = JSON.parse(line)
        if (!isRecord(request) || request.jsonrpc !== "2.0") {
          throw new RpcError(-32600, "invalid JSON-RPC 2.0 request")
        }
        id = request.id ?? null
        const method = request.method
        if (typeof method !== "string" || method.length === 0) {
          throw new RpcError(-32600, "invalid JSON-RPC method")
        }
        // The router makes a bounded Codex turn (~90 s timeout), so it gets
        // a longer idle window; everything else keeps the default.
        socket.setTimeout(
          method === "router.run" ? ROUTER_SOCKET_TIMEOUT_MS : SOCKET_TIMEOUT_MS,
          () => socket.destroy(),
        )
        const params = request.params ?? {}
        if (!isRecord(params)) {
          throw new RpcError(-32602, "params must be an object")
        }
        const handler = handlers[method]
        if (!handler) {
          throw new RpcError(-32601, `method not found: ${method}`)
        }
        const result = await handler(params)
        writeJson(socket, { jsonrpc: "2.0", id, result })
      } catch (error) {
        if (error instanceof RpcError) {
          writeJson(socket, rpcError(id, error.code, error.message))
        } else if (error instanceof SyntaxError) {
          writeJson(socket, rpcError(id, -32700, "parse error"))
        } else {
          console.error(`kbots-daemon: handler failed: ${error instanceof Error ? error.message : String(error)}`)
          writeJson(socket, rpcError(id, -32603, "internal error"))
        }
      }
    })()
  })

  socket.on("error", () => socket.destroy())
}

async function bindSecondary(
  path: string,
  handlers: Record<string, Handler>,
): Promise<Server | null> {
  try {
    if (await socketInUse(path)) {
      console.error(`${DAEMON_NAME}: secondary socket ${path} already serving; not binding it`)
      return null
    }
    await mkdir(dirname(path), { recursive: true, mode: 0o700 })
    await unlink(path).catch(() => {})
    const server = createServer((socket) => {
      acceptConnection(socket, handlers)
    })
    await new Promise<void>((resolvePromise, rejectPromise) => {
      server.once("error", rejectPromise)
      server.listen(path, resolvePromise)
    })
    await chmod(path, 0o600)
    console.error(`${DAEMON_NAME}: listening on ${path} (secondary)`)
    return server
  } catch (error) {
    // Never take the daemon down because the optional fallback failed.
    console.error(`${DAEMON_NAME}: could not bind secondary socket ${path}: ${error instanceof Error ? error.message : String(error)}`)
    return null
  }
}

async function main(): Promise<void> {
  const { socket: override } = parseArgs(process.argv.slice(2))
  const path = socketPath(override)
  await mkdir(dirname(path), { recursive: true, mode: 0o700 })
  await unlink(path).catch(() => {})

  const secondary = fallbackSocketPath(override, path)
  const handlers = buildHandlers(path, secondary ?? undefined)
  const server = createServer((socket) => {
    acceptConnection(socket, handlers)
  })

  let secondaryServer: Server | null = null
  const shutdown = () => {
    stopWatcher()
    secondaryServer?.close()
    server.close(() => {
      const removals = [unlink(path).catch(() => {})]
      if (secondaryServer) removals.push(unlink(secondary!).catch(() => {}))
      void Promise.all(removals).finally(() => process.exit(0))
    })
    setTimeout(() => process.exit(0), 2_000).unref()
  }
  process.on("SIGINT", shutdown)
  process.on("SIGTERM", shutdown)

  secondaryServer = secondary ? await bindSecondary(secondary, handlers) : null

  await new Promise<void>((resolvePromise, rejectPromise) => {
    server.once("error", rejectPromise)
    server.listen(path, resolvePromise)
  })
  await chmod(path, 0o600)
  console.error(`${DAEMON_NAME}: listening on ${path}`)
  const stopWatcher = startWatcherLoop()
  const stopNudge = startNudgeLoop()
  void reconcileAgents()
    .then(({ checked, changed }) => {
      if (checked > 0) console.error(`${DAEMON_NAME}: reconciled ${checked} live agent(s), ${changed} changed`)
    })
    .catch((error: unknown) => {
      console.error(`${DAEMON_NAME}: reconciliation failed: ${error instanceof Error ? error.message : String(error)}`)
    })
}

main().catch((error: unknown) => {
  console.error(`kbots-daemon: ${error instanceof Error ? error.message : String(error)}`)
  process.exit(1)
})
