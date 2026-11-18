import { Plugin } from "@opencode/plugin"
import { OpenCode } from "@opencode/sdk"

const model = process.env.OPENCODE_MODEL ?? "opencode/inkling-2-small-free"
const slash = model.indexOf("/")
if (slash < 1 || slash === model.length - 1) {
  throw new Error("OPENCODE_MODEL must use provider/model format")
}

const providerID = model.slice(0, slash)
const modelID = model.slice(slash + 1)
if (providerID !== "opencode" && providerID !== "opencode-go") {
  throw new Error(`Expected an OpenCode or OpenCode Go model, got ${providerID}`)
}

const databasePath = process.env.OPENCODE_DB_PATH
  ?? Bun.spawnSync(["opencode", "debug", "paths", "db"]).stdout.toString().trim()
if (!databasePath) throw new Error("Could not determine the OpenCode database path")

const plugin = Plugin.define({
  id: "customize-agent",
  async setup(ctx) {
    const providers = await ctx.provider.list()
    const selectedProvider = await ctx.provider.get({ providerID })
    console.info(`[kbots] providers: ${providers.data.map((item) => item.id).join(", ")}`)
    console.info(
      `[kbots] selected provider: ${selectedProvider.data.id} (${selectedProvider.data.name})`,
    )
    const connection = await ctx.integration.connection.active("opencode")
    console.info(`[kbots] OpenCode account connection available: ${Boolean(connection)}`)

    await ctx.agent.transform((agents) => {
      agents.update("build", (agent) => {
        agent.description = "Builds features and fixes bugs for our team"
      })
    })
  },
})

await using opencode = await OpenCode.create({
  database: { path: databasePath },
  config: { content: JSON.stringify({ model }) },
  plugins: [plugin],
})

let sessionID: string | undefined
try {
  const session = await opencode.sessions.create({
    location: { directory: process.cwd() },
    model: { providerID, id: modelID },
  })
  sessionID = session.id

  await opencode.sessions.prompt({
    sessionID,
    text: "Reply with exactly: OPENCODE_GO_SDK_OK",
  })
  await opencode.experimental.session.wait(
    { sessionID },
    { signal: AbortSignal.timeout(120_000) },
  )

  const messages = await opencode.message.list({
    sessionID,
    limit: 100,
    order: "desc",
    type: "assistant",
  })
  const assistant = messages.data.find((message) => message.type === "assistant")
  if (assistant?.type === "assistant" && assistant.error) {
    throw new Error(`${model} request failed: ${assistant.error.message}`)
  }

  const answer = assistant?.type === "assistant"
    ? assistant.content.filter((part) => part.type === "text").map((part) => part.text).join("")
    : undefined

  if (!answer) throw new Error("OpenCode returned no assistant text")

  console.log(`model: ${model}`)
  console.log(`assistant: ${answer}`)
} finally {
  if (sessionID) await opencode.sessions.remove({ sessionID })
}
