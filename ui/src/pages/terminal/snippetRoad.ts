/**
 * The engine's side of sending a snippet, in the one place that names it.
 *
 * `snippetSend.ts` decides the order — the policy is asked, then the person,
 * then the engine types — and this file is what it is given to do those
 * with: `terminal.classify`, the paste funnel's question
 * (`terminals.confirm`) and `terminal.snippets.send`. Nothing else in the
 * interface calls that last method, so a snippet cannot reach a device by a
 * way that skips the question; `roads.test.ts` holds that.
 */

import { rpc } from '@/api/client';
import type { SnippetSendResult, TerminalLineClass } from '@/api/types/terminal';
import { terminals } from './sessions';
import type { SnippetSendDeps } from './snippetSend';

export const snippetRoad: SnippetSendDeps = {
  classify: (deviceId, text) => rpc<TerminalLineClass[]>('terminal.classify', { device_id: deviceId, text }),
  confirm: (question) => terminals.confirm(question),
  send: (params) => rpc<SnippetSendResult>('terminal.snippets.send', params),
};
