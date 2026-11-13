import { describe, expect, it } from 'vitest';
import {
  RpcError,
  errorFromStatus,
  isEnvelope,
  isRpcError,
  toRpcError,
  unreachableError,
  unwrapEnvelope,
} from './errors';

describe('RpcError', () => {
  it('carries a code, a message and optional details', () => {
    const error = new RpcError('conflict', 'a device named core-sw-01 already exists', { field: 'name' });
    expect(error).toBeInstanceOf(Error);
    expect(error.name).toBe('RpcError');
    expect(error.code).toBe('conflict');
    expect(error.message).toBe('a device named core-sw-01 already exists');
    expect(error.details).toEqual({ field: 'name' });
    expect(isRpcError(error)).toBe(true);
    expect(isRpcError(new Error('x'))).toBe(false);
  });

  it('has no details unless the backend sent some', () => {
    expect(new RpcError('not_found', 'device 7 was not found').details).toBeUndefined();
  });
});

describe('unwrapEnvelope', () => {
  it('returns the result of a successful reply', () => {
    expect(unwrapEnvelope({ ok: true, result: [1, 2] })).toEqual([1, 2]);
    expect(unwrapEnvelope({ ok: true, result: null })).toBeNull();
    expect(unwrapEnvelope({ ok: true })).toBeUndefined();
  });

  it('throws the backend error with its code, message and details', () => {
    const reply = { ok: false, error: { code: 'not_found', message: 'device 7 was not found', details: { id: 7 } } };
    expect(() => unwrapEnvelope(reply)).toThrowError(RpcError);
    try {
      unwrapEnvelope(reply);
    } catch (error) {
      expect(error).toMatchObject({ code: 'not_found', message: 'device 7 was not found', details: { id: 7 } });
    }
  });

  it('keeps codes it has never heard of', () => {
    const reply = { ok: false, error: { code: 'device_error', message: 'the device rejected the command' } };
    expect(() => unwrapEnvelope(reply)).toThrowError(expect.objectContaining({ code: 'device_error' }));
  });

  it('reports anything else as a malformed reply', () => {
    for (const reply of [undefined, null, 'ok', 42, [], {}, { ok: 'yes' }, { ok: false }, { ok: false, error: 'boom' }]) {
      expect(() => unwrapEnvelope(reply)).toThrowError(expect.objectContaining({ code: 'internal' }));
    }
  });

  it('recognises envelopes', () => {
    expect(isEnvelope({ ok: true, result: 1 })).toBe(true);
    expect(isEnvelope({ ok: false, error: { code: 'invalid', message: 'bad' } })).toBe(true);
    expect(isEnvelope({ ok: false })).toBe(false);
    expect(isEnvelope('<html>')).toBe(false);
  });
});

describe('errorFromStatus', () => {
  it('maps HTTP statuses to stable codes', () => {
    const codes = Object.fromEntries(
      [400, 401, 403, 404, 408, 409, 413, 422, 500, 502, 503, 504].map((status) => [status, errorFromStatus(status).code]),
    );
    expect(codes).toEqual({
      400: 'invalid',
      401: 'unauthorized',
      403: 'forbidden',
      404: 'not_found',
      408: 'timeout',
      409: 'conflict',
      413: 'invalid',
      422: 'invalid',
      500: 'internal',
      502: 'unavailable',
      503: 'unavailable',
      504: 'timeout',
    });
  });

  it('says what to do about a rejected token', () => {
    expect(errorFromStatus(401).message).toMatch(/token/i);
  });
});

describe('toRpcError', () => {
  it('passes RpcErrors through untouched', () => {
    const error = new RpcError('timeout', 'no answer');
    expect(toRpcError(error)).toBe(error);
  });

  it('adopts an error payload thrown across IPC', () => {
    expect(toRpcError({ code: 'forbidden', message: 'read-only mode', details: 1 })).toMatchObject({
      code: 'forbidden',
      message: 'read-only mode',
      details: 1,
    });
  });

  it('wraps everything else as internal, keeping the message when there is one', () => {
    expect(toRpcError(new TypeError('x is not a function'))).toMatchObject({ code: 'internal', message: 'x is not a function' });
    expect(toRpcError('command rpc not found')).toMatchObject({ code: 'internal', message: 'command rpc not found' });
    expect(toRpcError(undefined)).toMatchObject({ code: 'internal' });
    expect(toRpcError(undefined).message).not.toBe('');
  });

  it('describes an unreachable backend as unavailable', () => {
    expect(unreachableError()).toMatchObject({ code: 'unavailable' });
  });
});
