/**
 * Stand-alone self-check for the maintenance orphan-selection rule. NOT
 * imported by the daemon. Must not touch the network, herdr, or the database.
 *
 * Run: bun run src/backend/orchestrator/maintenance.selfcheck.ts
 */
import assert from "node:assert/strict"

import { __maintenanceInternals } from "./maintenance.ts"

const { selectOrphanTerminals } = __maintenanceInternals

const terminal = (
  address: string,
  overrides: Partial<{ state: string; name: string | null; labels: Record<string, string> }> = {},
) => ({
  address,
  state: overrides.state ?? "live",
  name: overrides.name ?? null,
  labels: overrides.labels ?? { kbotsBotId: "AGT-1", kbotsRole: "ceo" },
})

const referenced = new Set(["t-live-kept"])

const picked = selectOrphanTerminals(
  [
    terminal("t-orphan", { name: "kbots-kbots-ceo-abcd" }),
    terminal("t-role-only", { labels: { kbotsRole: "git" } }),
    terminal("t-live-kept"),
    terminal("t-finished", { state: "finished" }),
    terminal("t-router", { labels: { owner: "codex" } }),
    terminal("t-unlabelled", { labels: {} }),
  ],
  referenced,
)

assert.deepEqual(
  picked.map((item) => item.address),
  ["t-orphan", "t-role-only"],
)
assert.equal(picked[0]?.name, "kbots-kbots-ceo-abcd")
assert.equal(picked[0]?.labels.kbotsBotId, "AGT-1")

console.log("maintenance.selfcheck: OK")
