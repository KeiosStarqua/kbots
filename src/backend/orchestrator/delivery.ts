/**
 * Queued task-message delivery. Shared by the dispatcher, the tasks.message
 * RPC and the message-flush path. Never forces: delivery uses the same
 * prompt path as `agents.prompt` (promptAgentCore), which throws when the
 * agent is busy or blocked; on any failure the message stays queued.
 *
 * This module deliberately does NOT import bots/agents.ts for its delivery
 * logic beyond promptAgentCore, and bots/agents.ts inlines its own flush in
 * reconcileAgents() to avoid an import cycle.
 */
import {
  addEvent,
  getTaskCeoAgent,
  listQueuedMessagesForTask,
  markMessageDelivered,
  type AgentRow,
  type TaskMessageRow,
} from "../db/repositories.ts"
import { promptAgentCore } from "../bots/agents.ts"

export interface FlushResult {
  delivered: TaskMessageRow[]
  failed: boolean
  error?: string
}

/**
 * Attempt to deliver queued user/system messages for a task, oldest first.
 * Stops at the first failure (busy/blocked/unreachable) so ordering is kept.
 * Logs `task.message_delivered` per delivered message; does not log on
 * failure (busy is the normal case — the message stays queued).
 */
export async function deliverQueuedMessages(
  taskId: string,
  agentOverride?: AgentRow,
): Promise<FlushResult> {
  const agent = agentOverride ?? getTaskCeoAgent(taskId)
  if (!agent) {
    return { delivered: [], failed: true, error: "no CEO agent linked to the task" }
  }
  const queued = listQueuedMessagesForTask(taskId)
  const delivered: TaskMessageRow[] = []
  for (const message of queued) {
    try {
      await promptAgentCore(agent, message.text)
    } catch (error) {
      const reason = error instanceof Error ? error.message : String(error)
      return { delivered, failed: true, error: reason }
    }
    const updated = markMessageDelivered(message.id)
    addEvent(
      "task.message_delivered",
      `Agent ${agent.id} received queued ${message.direction} message ${message.id}`,
      { taskId, agentId: agent.id, messageId: message.id, direction: message.direction },
    )
    delivered.push(
      updated ?? { ...message, status: "delivered", deliveredAt: new Date().toISOString() },
    )
  }
  return { delivered, failed: false }
}
