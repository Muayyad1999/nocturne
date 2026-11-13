/**
 * The one error type the UI sees from the backend, and the mapping of every
 * other failure (HTTP status, network, malformed reply) onto it.
 */

/**
 * Stable machine codes. The backend may add more; treat unknown codes as
 * `internal` when deciding what to do, and always show `message`.
 */
export type RpcErrorCode =
  | 'not_found'
  | 'invalid'
  | 'conflict'
  | 'unauthorized'
  | 'forbidden'
  | 'unavailable'
  | 'timeout'
  | 'cancelled'
  | 'device_error'
  | 'unknown_method'
  | 'internal'
  | (string & {});

export class RpcError extends Error {
  /** Stable machine string: branch on this, never on the message. */
  readonly code: RpcErrorCode;
  /** Optional structured context from the backend. */
  readonly details?: unknown;

  constructor(code: RpcErrorCode, message: string, details?: unknown) {
    super(message);
    this.name = 'RpcError';
    this.code = code;
    if (details !== undefined) this.details = details;
  }
}

export function isRpcError(value: unknown): value is RpcError {
  return value instanceof RpcError;
}

/** Turns anything thrown into an `RpcError`, keeping existing ones as they are. */
export function toRpcError(value: unknown): RpcError {
  if (isRpcError(value)) return value;
  if (isErrorPayload(value)) return new RpcError(value.code, value.message, value.details);
  if (value instanceof Error) return new RpcError('internal', value.message);
  if (typeof value === 'string' && value) return new RpcError('internal', value);
  return new RpcError('internal', 'Something went wrong inside Nocturne.');
}

interface ErrorPayload {
  code: string;
  message: string;
  details?: unknown;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function isErrorPayload(value: unknown): value is ErrorPayload {
  return isRecord(value) && typeof value.code === 'string' && typeof value.message === 'string';
}

/**
 * Unwraps a backend reply: `{ok: true, result}` returns the result,
 * `{ok: false, error}` throws that error, anything else is a malformed reply.
 */
export function unwrapEnvelope(reply: unknown): unknown {
  if (isRecord(reply)) {
    if (reply.ok === true) return reply.result;
    if (reply.ok === false && isErrorPayload(reply.error)) {
      throw new RpcError(reply.error.code, reply.error.message, reply.error.details);
    }
  }
  throw new RpcError('internal', 'Nocturne sent a reply the interface could not read.');
}

/** Whether a value has the shape of a backend reply envelope. */
export function isEnvelope(value: unknown): boolean {
  return (
    isRecord(value) && (value.ok === true || (value.ok === false && isErrorPayload(value.error)))
  );
}

/** The error for an HTTP response that carried no envelope of its own. */
export function errorFromStatus(status: number): RpcError {
  switch (status) {
    case 400:
    case 413:
    case 422:
      return new RpcError('invalid', `The server rejected the request (HTTP ${status}).`);
    case 401:
      return new RpcError(
        'unauthorized',
        'The access token is missing or no longer valid. Open the link printed by the server again.',
      );
    case 403:
      return new RpcError('forbidden', 'The server refused this request (HTTP 403).');
    case 404:
      return new RpcError('not_found', 'The server has no RPC endpoint at this address.');
    case 408:
    case 504:
      return new RpcError('timeout', `The server took too long to answer (HTTP ${status}).`);
    case 409:
      return new RpcError('conflict', 'The request conflicts with the current state (HTTP 409).');
    case 502:
    case 503:
      return new RpcError('unavailable', `The server is not available right now (HTTP ${status}).`);
    default:
      return new RpcError('internal', `The server answered with HTTP ${status}.`);
  }
}

/** The error for a request that never reached the backend. */
export function unreachableError(): RpcError {
  return new RpcError(
    'unavailable',
    'Nocturne is not reachable. Check that it is running and try again.',
  );
}
