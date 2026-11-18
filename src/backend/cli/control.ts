import { OpenCode } from "@opencode/client"
import { Service } from "@opencode/client/service"
import { Effect } from "effect"
import { resolve } from "node:path"

function parseModel(model: string): { providerID: string; id: string } {
  return Effect.runSync(Effect.try(() => {
    const separator = model.indexOf("/")
    if (separator < 1 || separator === model.length - 1) {
      throw new Error("--model must use provider/model format")
    }
    return {
      providerID: model.slice(0, separator),
      id: model.slice(separator + 1),
    }
  }))
}

function option(name: string): string | undefined {
  const index = process.argv.indexOf(name)
  return index === -1 ? undefined : process.argv[index + 1]
}

const kirAutoDirectory = resolve(process.env.KIR_AUTO_DIRECTORY
  ?? resolve(import.meta.dir, "../film-production/.agents/bot/kir-auto"))

if (process.argv.includes("--help") || process.argv.includes("-h")) {
  console.log(`Usage:
  bun run control                         Inspect the local OpenCode service
  bun run control --list-sessions         List sessions in kir-auto
  bun run control --session <id>          Inspect a kir-auto session
  bun run control --session <id> --prompt <text>
                                          Send a prompt to an existing session
  bun run control --session <id> --interrupt
                                          Interrupt its current run
  bun run control --prompt <text>         Create a new session and send a prompt

Options:
  --prompt <text>          Prompt text
  --model <provider/model> Model for a new session
  --session <id>           Existing session (must be in kir-auto)
  --list-sessions          List sessions in kir-auto
  --interrupt              Interrupt an existing session's current run
  --directory <path>       Project directory (defaults to kir-auto for
                           session commands; otherwise the current directory)
`)
  process.exit(0)
}

const prompt = option("--prompt")
const sessionID = option("--session")
const interrupt = process.argv.includes("--interrupt")
const listSessions = process.argv.includes("--list-sessions")
const directory = resolve(option("--directory") ?? (sessionID || listSessions
  ? kirAutoDirectory
  : process.cwd()))
const model = option("--model") ?? process.env.OPENCODE_MODEL

if (interrupt && prompt) throw new Error("Choose either --prompt or --interrupt")
if (listSessions && (sessionID || prompt || interrupt)) {
  throw new Error("--list-sessions cannot be combined with session actions")
}
if (interrupt && !sessionID) throw new Error("--interrupt requires --session <id>")

const endpoint = await Service.ensure()
const client = OpenCode.make({
  baseUrl: endpoint.url,
  headers: Service.headers(endpoint),
})

const [server, providers] = await Promise.all([
  client.server.info(),
  client.provider.list({ location: { directory } }),
])

console.log(`server: ${server.version} (pid ${server.pid})`)
console.log(`url: ${endpoint.url}`)
console.log(`providers: ${providers.data.map((provider) => provider.id).join(", ")}`)

let targetSessionID = sessionID

if (listSessions) {
  const sessions = await client.session.list({ directory, limit: 100, order: "desc" })
  console.log(`sessions in ${directory}:`)
  for (const session of sessions.data) {
    const selectedModel = session.model
      ? `${session.model.providerID}/${session.model.id}`
      : "server default"
    console.log(
      `${session.id}\t${session.outcome ?? "active"}\t${selectedModel}\t${session.title ?? "untitled"}`,
    )
  }
  process.exit(0)
}

if (sessionID) {
  const session = await client.session.get({ sessionID })
  if (resolve(session.location.directory) !== directory) {
    throw new Error(`Session ${sessionID} is not in ${directory}`)
  }

  if (interrupt) {
    await client.session.interrupt({ sessionID, resume: false })
    console.log(`interrupted: ${sessionID}`)
    process.exit(0)
  }

  if (!prompt) {
    const selectedModel = session.model
      ? `${session.model.providerID}/${session.model.id}`
      : "server default"
    console.log(`session: ${session.id}`)
    console.log(`title: ${session.title ?? "untitled"}`)
    console.log(`directory: ${session.location.directory}`)
    console.log(`model: ${selectedModel}`)
    console.log(`outcome: ${session.outcome ?? "active"}`)
    process.exit(0)
  }

  await client.session.prompt({ sessionID, text: prompt })
  await client.experimental.session.wait(
    { sessionID },
    { signal: AbortSignal.timeout(120_000) },
  )
} else {
  if (!prompt) process.exit(0)

  let modelRef: { providerID: string; id: string } | undefined
  if (model) {
    modelRef = parseModel(model)
  }

  const session = await client.session.create({
    title: "kbots local server control",
    location: { directory },
    model: modelRef,
  })
  targetSessionID = session.id
  await client.session.prompt({ sessionID: targetSessionID, text: prompt })
  await client.experimental.session.wait(
    { sessionID: targetSessionID },
    { signal: AbortSignal.timeout(120_000) },
  )
  console.log(`session: ${targetSessionID}`)
}

if (!targetSessionID) throw new Error("No session was selected or created")

const messages = await client.message.list({
  sessionID: targetSessionID,
  limit: 100,
  order: "desc",
  type: "assistant",
})
const assistant = messages.data.find((message) => message.type === "assistant")
if (assistant?.type === "assistant" && assistant.error) {
  throw new Error(`OpenCode request failed: ${assistant.error.message}`)
}

const answer = assistant?.type === "assistant"
  ? assistant.content.filter((part) => part.type === "text").map((part) => part.text).join("")
  : undefined
if (!answer) throw new Error("OpenCode returned no assistant text")

console.log(`assistant: ${answer}`)
