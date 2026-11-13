import { create } from 'zustand';
import type { ConnectionState, TransportKind } from './transport';

interface Connection {
  /** State of the link to the engine. */
  state: ConnectionState;
  /** Which transport is in use; null until one has been chosen. */
  transport: TransportKind | null;
}

/** The connection to the engine, for anything that shows or reacts to it. */
export const useConnection = create<Connection>(() => ({
  state: 'connecting',
  transport: null,
}));
