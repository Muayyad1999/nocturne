/**
 * The `isp_access` setting: whether the assistant may read the ISP customer
 * ledger. What the words on the Settings tab say is fixed here, from what
 * the engine does (`crates/nocturne-assistant/src/tools/ledger.rs` and the
 * gate in `tools/mod.rs`), so the screen and its tests say the same thing.
 *
 * Off, which is the default, is *nothing*: the ledger's three tools are not
 * offered and the gate refuses all five of its reads, `isp.summary` among
 * them — not even a count reaches a model. On, the model may be sent the
 * ledger's totals and, per customer, an id, a name, a state, a plan and a
 * balance, and invoices by number, state and amount. What is passed on is
 * an allow-list of fields, so a contact detail is never among them, under
 * either setting.
 */

import type { AiStatus, AiToolInfo } from '@/api/types/ai';

/**
 * The switch is shown only by a build that has the ledger and the settings
 * methods to change it with. The key itself exists in the free edition too,
 * where it is `false` and changes nothing, so it is not offered there.
 */
export const LEDGER_SWITCH_NEEDS: readonly string[] = ['isp.summary', 'settings.get', 'settings.set'];

/** `undefined` while `app.methods` has not answered: not yet known is not absent. */
export function ledgerSwitchAvailable(methods: readonly string[] | undefined): boolean | undefined {
  if (methods === undefined) return undefined;
  return LEDGER_SWITCH_NEEDS.every((method) => methods.includes(method));
}

/** The tools `ai.tools` lists that read the ledger: the ones the switch decides. */
export function ledgerTools(tools: readonly AiToolInfo[]): AiToolInfo[] {
  return tools.filter((tool) => tool.methods.some((method) => method.startsWith('isp.')));
}

export const LEDGER_WORDS = {
  label: 'Let the assistant read the customer ledger',
  off: 'Off. The assistant is offered none of the ledger’s tools and reads nothing from it: no customer, no invoice and no total reaches the model.',
  on: 'On. The model a conversation uses may be sent the ledger’s totals — customers by state, invoices by state, arrears per currency — and, per customer, their id, name, state, plan and balance, and their invoices by number, state, amounts and due date.',
  never:
    'Never sent, whichever way this is set: phone numbers, e-mail and postal addresses, account codes, notes, tags, where a service is, invoice lines and individual payments. The assistant cannot change anything in the ledger.',
} as const;

/**
 * Which of the ledger's tools a turn with the default model would be offered
 * now, in a sentence — read from `ai.tools`, never inferred from the switch,
 * so the screen shows what the engine does rather than what it was asked.
 * With no model set up, `ai.tools` lists what a model would be offered.
 */
export function offeredNow(tools: readonly AiToolInfo[], on: boolean, configured = true): string {
  const offered = ledgerTools(tools);
  if (offered.length > 0) {
    const titles = offered.map((tool) => tool.title).join(', ');
    return configured
      ? `Offered to the default model now: ${titles}.`
      : `No model is set up yet; one would be offered: ${titles}.`;
  }
  if (!on) return 'None of the ledger’s tools is offered to the model now.';
  // On, and still none: the default model is given the core set, which is
  // what a context window of 16 000 tokens or fewer gets.
  return 'None of the ledger’s tools is offered to the default model now: a model with a context window of 16 000 tokens or fewer is given only the core tools. If this one has more, set its window on the Models tab.';
}

export interface Offer {
  /** What `ai.tools` last answered, if it has. */
  tools: readonly AiToolInfo[] | undefined;
  /**
   * `ai.tools` or the setting is being read again. The two are re-read side
   * by side after a change, and whichever comes back first, the pair is from
   * two different moments until the other does: "on, and none of the
   * ledger's tools" would then name a reason that is not the one.
   */
  reading: boolean;
  /**
   * Why the last reading of `ai.tools` failed, when it did. A list from an
   * earlier reading may still be held beside it; it is not what holds now.
   */
  error?: string;
  on: boolean;
  configured: boolean;
}

/** The line under the switch: what the engine offers, or that it is being asked. */
export function offerLine({ tools, reading, error, on, configured }: Offer): string {
  if (reading) return 'Reading which tools are offered…';
  if (error !== undefined) return `Which tools are offered could not be read: ${error}`;
  if (tools === undefined) return 'Reading which tools are offered…';
  return offeredNow(tools, on, configured);
}

/**
 * What to say once `settings.set` has answered, from what the engine says it
 * now holds rather than from what was asked: the section comes back whole.
 */
export function savedWords(asked: boolean, answer: unknown): { saved: boolean; text: string } {
  const held =
    typeof answer === 'object' && answer !== null && 'isp_access' in answer
      ? (answer as { isp_access: unknown }).isp_access
      : undefined;
  if (held !== asked) {
    return {
      saved: false,
      text: 'The engine did not store the change. The switch shows what it holds.',
    };
  }
  return {
    saved: true,
    text: asked
      ? 'The assistant may read the customer ledger'
      : 'The assistant no longer reads the customer ledger',
  };
}

/**
 * The sentence of the question that names where the records would go. Before
 * `ai.status` has answered nothing is known about the model, which is not
 * the same as there being none.
 */
export function modelWords(status: AiStatus | undefined): string {
  if (status === undefined) return 'Which model that is could not be read.';
  const model = status.default;
  if (!model) return 'No model is set up yet.';
  return `The default model now is ${model.name} (${model.model}, at ${model.base_url}).`;
}
