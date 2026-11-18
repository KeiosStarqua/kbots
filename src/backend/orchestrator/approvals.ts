import { RpcError, invalidParams } from "../errors.ts"
import {
  addEvent,
  approveApproval,
  getApproval,
  listApprovals,
  type ApprovalRow,
} from "../db/repositories.ts"

export async function listApprovalsHandler(params: Record<string, unknown>): Promise<{ items: ApprovalRow[] }> {
  const raw = params.status
  let status: "pending" | "approved" | undefined
  if (raw !== undefined && raw !== null) {
    if (typeof raw !== "string" || (raw !== "pending" && raw !== "approved")) {
      throw invalidParams("status must be 'pending' or 'approved'")
    }
    status = raw
  }
  return { items: listApprovals({ status }) }
}

export async function approveApprovalHandler(params: Record<string, unknown>): Promise<Record<string, unknown>> {
  const approvalId = typeof params.approvalId === "string" ? params.approvalId.trim() : ""
  if (!approvalId) throw invalidParams("approvalId is required")

  const existing = getApproval(approvalId)
  if (!existing) throw new RpcError(-32602, `approval not found: ${approvalId}`)

  approveApproval(approvalId)
  if (existing.status === "pending") {
    addEvent("approval.approved", `Approval ${approvalId} approved`, { approvalId, operation: existing.operation })
  }

  return { approvalId, status: "approved", alreadyApproved: existing.status === "approved" }
}
