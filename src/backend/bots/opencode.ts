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
