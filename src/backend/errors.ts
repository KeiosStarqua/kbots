/**
 * Shared error with a JSON-RPC 2.0 error code. The daemon maps these to the
 * `error` object of a JSON-RPC response; unexpected throws become -32603
 * with a generic message so internals never leak to clients.
 */
export class RpcError extends Error {
  constructor(
    public readonly code: number,
    message: string,
  ) {
    super(message)
    this.name = "RpcError"
  }
}

export const invalidParams = (message: string): RpcError => new RpcError(-32602, message)
