import { invalidParams } from "../errors.ts"
import { listEvents, type EventRow } from "../db/repositories.ts"

export async function listEventsHandler(params: Record<string, unknown>): Promise<{ items: EventRow[] }> {
  const raw = params.limit
  let limit = 100
  if (raw !== undefined && raw !== null) {
    if (typeof raw !== "number" || !Number.isFinite(raw)) {
      throw invalidParams("limit must be a number between 1 and 500")
    }
    limit = Math.min(500, Math.max(1, Math.trunc(raw)))
  }
  return { items: listEvents(limit) }
}
