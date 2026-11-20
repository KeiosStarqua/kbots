/**
 * Shared OpenCode client factory for agent lifecycle work. Talks to the
 * registered local laptop service (same as start-sessions.ts / control.ts);
 * never mutates sessions directly outside this module's callers' intent.
 */
import { OpenCode } from "@opencode/client"
import { Service } from "@opencode/client/service"

export async function makeOpencodeClient() {
  const endpoint = await Service.ensure()
  const client = OpenCode.make({
    baseUrl: endpoint.url,
    headers: Service.headers(endpoint),
  })
  return { client, endpoint }
}

// ---------------------------------------------------------------------------
// Session model health: is the model a session is pinned to still available?
// OpenCode's model list is the source of truth (`status: "active"`); the list
// is cached briefly because it is read on every reconcile pass.
// ---------------------------------------------------------------------------

export interface ModelRef {
  providerID: string
  id: string
}

/** The active models of one location, split by whether they cost anything. */
export interface ActiveModels {
  /** Every active `providerID/modelID` key. */
  keys: Set<string>
  /** Active models that cost nothing (every cost tier is zero input and output). */
  free: Set<string>
}

const MODEL_CACHE_TTL_MS = 60_000
// Keyed by directory: providers and models are resolved per project location,
// so one project's list must never answer for another's agent.
const availableModelsCache = new Map<string, { at: number; models: ActiveModels }>()

/** A model list entry costs nothing when every tier bills nothing. */
function isFreeModel(model: Record<string, unknown>): boolean {
  const tiers = Array.isArray(model.cost) ? model.cost : []
  return (
    tiers.length > 0 &&
    tiers.every((tier) => {
      const cost = tier as Record<string, unknown>
      return cost.input === 0 && cost.output === 0
    })
  )
}

/** Active models for a location, or null when the list is unreadable. */
export async function availableModels(directory: string): Promise<ActiveModels | null> {
  const cached = availableModelsCache.get(directory)
  if (cached && Date.now() - cached.at < MODEL_CACHE_TTL_MS) {
    return cached.models
  }
  try {
    const { client } = await makeOpencodeClient()
    const response = (await client.model.list({ location: { directory } })) as unknown as {
      data?: unknown
    }
    const list = Array.isArray(response.data) ? response.data : []
    const keys = new Set<string>()
    const free = new Set<string>()
    for (const raw of list) {
      const model = raw as Record<string, unknown>
      if (model?.status !== "active") continue
      const providerID = typeof model.providerID === "string" ? model.providerID : null
      const modelID =
        typeof model.modelID === "string"
          ? model.modelID
          : typeof model.id === "string"
            ? model.id
            : null
      if (!providerID || !modelID) continue
      const key = `${providerID}/${modelID}`
      keys.add(key)
      if (isFreeModel(model)) free.add(key)
    }
    const models = { keys, free }
    availableModelsCache.set(directory, { at: Date.now(), models })
    return models
  } catch {
    return null
  }
}

/** The model a session is currently pinned to, or null when unreadable. */
export async function sessionModelRef(sessionID: string): Promise<ModelRef | null> {
  try {
    const { client } = await makeOpencodeClient()
    const session = (await client.session.get({ sessionID })) as unknown as Record<string, unknown>
    const model = session?.model as Record<string, unknown> | undefined
    if (model && typeof model.providerID === "string" && typeof model.id === "string") {
      return { providerID: model.providerID, id: model.id }
    }
    return null
  } catch {
    return null
  }
}

/**
 * Deterministic fallback: `KBOTS_MODEL_FALLBACK` (comma-separated
 * `provider/model`) first, then free models before paid ones (from the model
 * list's cost tiers, not their names), same provider before others, then the
 * lexicographically first active model. Null when nothing is left.
 */
export function pickFallbackModel(models: ActiveModels, dead: ModelRef | null): ModelRef | null {
  const preferred = (process.env.KBOTS_MODEL_FALLBACK ?? "")
    .split(",")
    .map((entry) => entry.trim())
    .filter(Boolean)
  const sorted = [...models.keys].sort()
  const free = [...models.free].sort()
  const sameProvider = (keys: string[]): string[] =>
    dead ? keys.filter((key) => key.startsWith(`${dead.providerID}/`)) : []
  const candidates = [
    ...preferred,
    // Free models first: an automatic switch must not silently pick a paid model.
    ...sameProvider(free),
    ...free,
    ...sameProvider(sorted),
    ...sorted,
  ]
  for (const ref of candidates) {
    if (!models.keys.has(ref)) continue
    const slash = ref.indexOf("/")
    if (slash > 0) return { providerID: ref.slice(0, slash), id: ref.slice(slash + 1) }
  }
  return null
}

/** Point a session at another model (the next turn uses it). */
export async function switchSessionModel(sessionID: string, model: ModelRef): Promise<boolean> {
  try {
    const { client } = await makeOpencodeClient()
    await client.session.switchModel({ sessionID, model })
    return true
  } catch {
    return false
  }
}
