/**
 * Queued task-message delivery. Shared by the dispatcher, the tasks.message
 * RPC and the message-flush path. Never forces: delivery uses the same
 * prompt path as `agents.prompt` (promptAgentCore), which throws when the
 * agent is busy or blocked; on any failure the message stays queued.
 * Every delivery try increments `task_messages.attempts`; messages that
 * reach MESSAGE_ATTEMPT_LIMIT (5) are canceled instead of retried, logged
 * as `message.abandoned`, and may move an active task to `blocked`.
 *
 * This module deliberately does NOT import bots/agents.ts for its delivery
 * logic beyond promptAgentCore, and bots/agents.ts inlines its own flush in
 * reconcileAgents() to avoid an import cycle.
 */
import {
  abandonQueuedMessage,
  addEvent,
  getTaskCeoAgent,
  incrementMessageAttempts,
  listQueuedMessagesForTask,
  markMessageDelivered,
  MESSAGE_ATTEMPT_LIMIT,
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
 * Every attempt increments `task_messages.attempts`; a message at the
 * attempt limit is canceled, logged as `message.abandoned`, and — when the
 * task is actively researching/planning/delegating/running — moves the task
 * to `blocked` with a note in task_briefs.blockers.
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
    if (message.attempts >= MESSAGE_ATTEMPT_LIMIT) {
      abandonQueuedMessage(message)
      continue
    }
    const attempted =
      incrementMessageAttempts(message.id) ?? { ...message, attempts: message.attempts + 1 }
    try {
      await promptAgentCore(agent, message.text)
    } catch (error) {
      const reason = error instanceof Error ? error.message : String(error)
      if (attempted.attempts >= MESSAGE_ATTEMPT_LIMIT) abandonQueuedMessage(attempted)
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
