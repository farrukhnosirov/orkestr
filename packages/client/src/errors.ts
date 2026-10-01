import type { RpcError, RpcErrorCode } from "@orkestr/protocol";

/** Error codes produced by the client itself (never sent by the server). */
export type ClientErrorCode = "timeout" | "disconnected" | "closed" | "aborted" | "protocol";

/** Error thrown by `OrkestrClient.call` and `connect`. */
export class OrkestrError extends Error {
  readonly code: RpcErrorCode | ClientErrorCode;
  readonly data: unknown;

  constructor(code: RpcErrorCode | ClientErrorCode, message: string, data: unknown = null) {
    super(message);
    this.name = "OrkestrError";
    this.code = code;
    this.data = data;
  }

  static fromRpc(error: RpcError): OrkestrError {
    return new OrkestrError(error.code, error.message, error.data ?? null);
  }

  /** True for errors that a retry after reconnecting may fix. */
  get isTransient(): boolean {
    return this.code === "timeout" || this.code === "disconnected" || this.code === "rate_limited";
  }
}
