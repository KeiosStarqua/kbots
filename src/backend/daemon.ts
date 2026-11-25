/**
 * kbots workflow daemon (v1, minimal).
 *
 * Owns workflow state through SQLite and exposes the local JSON-RPC 2.0
 * contract from docs/kbots-ipc.md over a Unix domain socket (Unix hosts) and
 * an authenticated TCP listener (Windows hosts, or `--tcp` / KBOTS_TCP on
 * Unix). One newline-delimited request per connection; one response.
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
 * TCP defaults to 127.0.0.1:7878 on Windows; `--tcp HOST:PORT` (or
 * KBOTS_TCP) enables it on Unix too for remote/Tailscale clients.
 */
import { chmod, mkdir, unlink } from "node:fs/promises"
import { homedir } from "node:os"
import { dirname, join } from "node:path"
import { connect, createServer, type Server, type Socket } from "node:net"
import { Effect } from "effect"

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
  agentsLogsHandler,
  reconcileAgents,
} from "./bots/agents.ts"
import {
  startWatcherLoop,
  watchersListHandler,
  watchersSetHandler,
} from "./orchestrator/watchers.ts"
import { startNudgeLoop } from "./orchestrator/nudger.ts"
import { startMaintenanceLoop } from "./orchestrator/maintenance.ts"
import { authorized, DEFAULT_TCP_ADDR, ensureToken, tcpAddress } from "./adapters/transport.ts"

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

function acceptConnection(socket: Socket, handlers: Record<string, Handler>, authToken: string | null): void {
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
  void handleConnection(socket, handlers, authToken)
}

type Handler = (params: Record<string, unknown>) => Promise<unknown>

const startedAt = Date.now()

function parseArgs(argv: string[]): { socket?: string; tcp?: string } {
  const result: { socket?: string; tcp?: string } = {}
  const index = argv.indexOf("--socket")
  if (index !== -1) {
    const value = argv[index + 1]
    if (!value || value.startsWith("--")) throw new Error("--socket requires a path")
    result.socket = value
  }
  const equals = argv.find((arg) => arg.startsWith("--socket="))
  if (equals) result.socket = equals.slice("--socket=".length)
  for (const flag of ["--tcp", "--host"]) {
    const at = argv.indexOf(flag)
    if (at !== -1) {
      const value = argv[at + 1]
      result.tcp = value && !value.startsWith("--") ? value : ""
      break
    }
    const withEquals = argv.find((arg) => arg.startsWith(`${flag}=`))
    if (withEquals) {
      result.tcp = withEquals.slice(flag.length + 1)
      break
    }
  }
  return result
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

function buildHandlers(socket: string, secondarySocket?: string, tcp?: string): Record<string, Handler> {
  return {
    "system.status": () => systemStatusHandler({ socketPath: socket, secondarySocketPath: secondarySocket, tcpAddress: tcp, startedAt }),
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
    "agents.logs": agentsLogsHandler,
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

async function handleConnection(socket: Socket, handlers: Record<string, Handler>, authToken: string | null): Promise<void> {
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
        if (authToken !== null && !authorized(request.token, authToken)) {
          throw new RpcError(-32001, "unauthorized: missing or invalid daemon token (set KBOTS_TOKEN)")
        }
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
  authToken: string | null,
): Promise<Server | null> {
  try {
    if (await socketInUse(path)) {
      console.error(`${DAEMON_NAME}: secondary socket ${path} already serving; not binding it`)
      return null
    }
    await mkdir(dirname(path), { recursive: true, mode: 0o700 })
    await unlink(path).catch(() => {})
    const server = createServer((socket) => {
      acceptConnection(socket, handlers, authToken)
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
  const args = parseArgs(process.argv.slice(2))
  const isWindows = process.platform === "win32"
  // Windows has no AF_UNIX daemon transport, so TCP is always on there.
  const tcp = tcpAddress(args.tcp) ?? (isWindows ? DEFAULT_TCP_ADDR : null)
  const authToken = tcp ? await Effect.runPromise(ensureToken) : null
  // Windows reports its TCP endpoint as the primary address; Unix keeps the
  // legacy AF_UNIX listeners. Windows never binds an AF_UNIX path.
  const path = isWindows ? (tcp ?? DEFAULT_TCP_ADDR) : socketPath(args.socket)
  const bindUnix = !isWindows
  if (bindUnix) {
    await mkdir(dirname(path), { recursive: true, mode: 0o700 })
    await unlink(path).catch(() => {})
  }

  const secondary = bindUnix ? fallbackSocketPath(args.socket, path) : null
  const handlers = buildHandlers(path, secondary ?? undefined, tcp ?? undefined)
  // Unix sockets stay protected by their 0600 file mode; only TCP carries the
  // token so an opt-in TCP listener does not change local socket behavior.
  const acceptUnix = (socket: Socket) => acceptConnection(socket, handlers, null)
  const acceptTcp = (socket: Socket) => acceptConnection(socket, handlers, authToken)
  let unixServer: Server | null = null
  if (bindUnix) unixServer = createServer(acceptUnix)
  let tcpServer: Server | null = null
  if (tcp) tcpServer = createServer(acceptTcp)

  let secondaryServer: Server | null = null
  const servers = [unixServer, tcpServer].filter((server): server is Server => server !== null)
  const shutdown = () => {
    stopWatcher()
    stopNudge()
    stopMaintenance()
    secondaryServer?.close()
    for (const server of servers) server.close()
    if (bindUnix) void unlink(path).catch(() => {})
    if (secondary) void unlink(secondary).catch(() => {})
    setTimeout(() => process.exit(0), 500).unref()
  }
  process.on("SIGINT", shutdown)
  process.on("SIGTERM", shutdown)

  if (secondary) secondaryServer = await bindSecondary(secondary, handlers, null)

  if (unixServer) {
    await new Promise<void>((resolvePromise, rejectPromise) => {
      unixServer!.once("error", rejectPromise)
      unixServer!.listen(path, resolvePromise)
    })
    await chmod(path, 0o600)
    console.error(`${DAEMON_NAME}: listening on ${path}`)
  }
  if (tcpServer && tcp) {
    const [host, portText] = tcp.split(/:(?=[^:]*$)/)
    const port = Number.parseInt(portText ?? "", 10)
    if (!host || !Number.isInteger(port) || port < 1 || port > 65535) {
      throw new Error(`invalid TCP address: ${tcp} (expected HOST:PORT)`)
    }
    await new Promise<void>((resolvePromise, rejectPromise) => {
      tcpServer!.once("error", rejectPromise)
      tcpServer!.listen(port, host, resolvePromise)
    })
    console.error(`${DAEMON_NAME}: listening on tcp://${tcp}`)
  }
  const stopWatcher = startWatcherLoop()
  const stopNudge = startNudgeLoop()
  // Periodic reconcile + reap of finished CEO terminals (KBOTS_REAP=off,
  // KBOTS_REAP_INTERVAL_MS, KBOTS_REAP_GRACE_MS). Sessions are never deleted.
  const stopMaintenance = startMaintenanceLoop()
  void reconcileAgents()
    .then(({ checked, changed }) => {
      if (checked > 0) console.error(`${DAEMON_NAME}: reconciled ${checked} live agent(s), ${changed} changed`)
    })
    .catch((error: unknown) => {
      console.error(`${DAEMON_NAME}: reconciliation failed: ${error instanceof Error ? error.message : String(error)}`)
    })
}

main().catch((error: unknown) => {
  const detail = error instanceof Error ? (error.stack ?? error.message) : String(error)
  console.error(`kbots-daemon: ${detail}`)
  process.exit(1)
})
