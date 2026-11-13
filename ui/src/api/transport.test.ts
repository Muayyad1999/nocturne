import { describe, expect, it } from 'vitest';
import { chooseTransport } from './transport';

describe('chooseTransport', () => {
  it('uses Tauri IPC inside the desktop shell', () => {
    expect(chooseTransport({ tauri: true, dev: false, mockRequested: false })).toBe('tauri');
  });

  it('uses HTTP and WebSocket in a browser', () => {
    expect(chooseTransport({ tauri: false, dev: false, mockRequested: false })).toBe('http');
  });

  it('uses the mock only when asked for, and only in a development or test build', () => {
    expect(chooseTransport({ tauri: false, dev: true, mockRequested: true })).toBe('mock');
    expect(chooseTransport({ tauri: true, dev: true, mockRequested: true })).toBe('mock');
    expect(chooseTransport({ tauri: false, dev: true, mockRequested: false })).toBe('http');
  });

  it('never uses the mock in a production build, whatever the address says', () => {
    expect(chooseTransport({ tauri: false, dev: false, mockRequested: true })).toBe('http');
    expect(chooseTransport({ tauri: true, dev: false, mockRequested: true })).toBe('tauri');
  });
});
