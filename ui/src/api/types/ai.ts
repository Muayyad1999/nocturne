/**
 * The AI assistant (`nocturne-assistant`): model providers, chats, the live
 * event stream and the tool boundary.
 *
 * These types mirror the backend's parameter and result structs exactly:
 * field names are the backend's (`snake_case`), timestamps are milliseconds
 * since the epoch, and `?` marks a field the backend may leave out (or a
 * parameter it defaults). Backend source: `crates/nocturne-assistant`.
 * Specification: `docs/dev/features/assistant.md`.
 *
 * Two things to know before building against this:
 *
 * 1. **`ai.send` returns immediately.** It stores the user's message and
 *    answers with a `turn_id`; everything the model says, every tool it runs
 *    and every token it costs arrives on the `ai.event` stream. `seq` rises by
 *    one per chat, so a window that re-attaches can see it missed something
 *    and call `ai.chats.get` to catch up.
 * 2. **No provider configured is a normal state**, not an error state:
 *    `ai.status` says `configured: false` and `ai.send` fails with
 *    `unavailable` and a message that says where to add one. Nothing else in
 *    Nocturne depends on the assistant.
 */

// ----------------------------------------------------------------- providers

/** Which API a provider speaks. A chat is bound to the kind it started with. */
export type ProviderKind = 'anthropic' | 'openai' | 'ollama';

/** How hard the model should think; mapped to each provider's own control. */
export type Effort = 'low' | 'medium' | 'high' | 'xhigh' | 'max';

/**
 * How tools are offered to the model. `auto` uses the tool-call API and falls
 * back to describing the tools in the prompt if the model or server rejects
 * it; `prompted` always describes them in the prompt, which works with local
 * models that have no tool API at all.
 */
export type ToolMode = 'auto' | 'native' | 'prompted';

/** A configured model endpoint. The API key is never part of this. */
export interface ProviderEntry {
  id: string;
  name: string;
  kind: ProviderKind;
  base_url: string;
  model: string;
  /** The backend sends `null` when no effort is set, not an absent field. */
  effort?: Effort | null;
  tool_mode: ToolMode;
  /**
   * Tokens of context, as configured; `null` when nobody set one, and the
   * engine then plans with the provider's own assumption (16 000 for an
   * OpenAI-compatible endpoint on the local network). A window of 16 000 or
   * fewer is offered the ten core tools only, with tighter result caps.
   */
  context_window?: number | null;
  max_output_tokens?: number | null;
  /** Whether a key is stored in the vault for this provider. */
  has_key: boolean;
  is_default: boolean;
  enabled: boolean;
  created_at: number;
  updated_at: number;
}

/** A ready-made entry for the "add provider" dialog. */
export interface ProviderPreset {
  id: string;
  name: string;
  kind: ProviderKind;
  base_url: string;
  default_model: string;
  needs_key: boolean;
  /** Runs on the user's own machine or network: nothing leaves the site. */
  local: boolean;
  key_url: string;
  note: string;
}

/** A model an endpoint offers. */
export interface ModelInfo {
  id: string;
  name: string;
  /** `null` when the endpoint does not say. */
  context_window?: number | null;
  max_output?: number | null;
}

/** A local model runtime found on this machine. */
export interface LocalEndpoint {
  /** The preset to prefill from (`ollama`, `lmstudio`). */
  preset: string;
  base_url: string;
  models: ModelInfo[];
}

/** `ai.providers.list` (no params) → {@link ProviderEntry}[] (the default first) */

/** `ai.providers.presets` (no params) → {@link ProviderPreset}[] */

export interface AiProvidersSaveParams {
  /** Omit to add a provider; give it to change one. */
  id?: string;
  name?: string;
  kind?: ProviderKind;
  /** Defaults to the endpoint of the kind (Anthropic, OpenAI, Ollama). */
  base_url?: string;
  model?: string;
  /** Stored in the vault (`ai:provider:<id>:key`) and never returned. `""` removes it. */
  api_key?: string;
  effort?: Effort;
  tool_mode?: ToolMode;
  context_window?: number;
  max_output_tokens?: number;
  is_default?: boolean;
  enabled?: boolean;
}

/** `ai.providers.save` {@link AiProvidersSaveParams} → {@link ProviderEntry} */

export interface AiProviderIdParams {
  id: string;
}

/** `ai.providers.delete` {@link AiProviderIdParams} → {@link AiDeleted} */

export interface AiDeleted {
  deleted: boolean;
}

/** What one tiny real request to the endpoint found out. */
export interface ProviderTestResult {
  ok: boolean;
  model: string;
  latency_ms: number;
  /**
   * Whether the model could use a tool, and how. `none` means it can talk
   * about the network but not look at it.
   */
  tool_calling: 'native' | 'prompted' | 'none';
  /** Why it failed, or what to know about how it passed. */
  error?: string;
}

/** `ai.providers.test` {@link AiProviderIdParams} → {@link ProviderTestResult} */

export interface AiProvidersModelsParams {
  /** A saved provider, whose key comes from the vault… */
  id?: string;
  /** …or an endpoint that has not been saved yet. */
  kind?: ProviderKind;
  base_url?: string;
  api_key?: string;
}

/** `ai.providers.models` {@link AiProvidersModelsParams} → {@link ModelInfo}[] */

/** `ai.providers.detect_local` (no params) → {@link LocalEndpoint}[] */

/** How much the assistant may do by itself. Enforced by the change engine. */
export type Autonomy = 'draft' | 'lab' | 'low_risk';

export interface AiStatus {
  /** False is normal: the user has not added a model yet. */
  configured: boolean;
  default?: ProviderEntry;
  autonomy: Autonomy;
  /** Always true: there is no way to send a model unredacted device output. */
  redaction: boolean;
  /**
   * How many tools a turn with the default provider would be offered now:
   * the core ten for a small window, the ledger's only while `isp_access` is
   * on. Decided from the window a turn uses, configured or assumed.
   */
  tools: number;
}

/** `ai.status` (no params) → {@link AiStatus} */

/**
 * One tool the assistant can use here, and the RPC methods it may reach.
 * Real entry: `{"name":"qos_status","title":"QoS classes","touches_device":false,
 * "methods":["monitor.qos","devices.get"]}`.
 */
export interface AiToolInfo {
  name: string;
  title: string;
  /**
   * It opens a session to a device or sends probes to addresses (a discovery
   * scan), so `confirm_reads` asks about it.
   */
  touches_device: boolean;
  methods: string[];
}

/**
 * `ai.tools` (no params) → {@link AiToolInfo}[] — the tools a turn with the
 * default provider would be offered now: those whose methods this build
 * registers, the ISP ledger's (`isp_overview`, `isp_customers`,
 * `isp_invoices`) only while `isp_access` is on, and the core ten for a
 * small window. With no provider configured, every tool the build has.
 */

// --------------------------------------------------------------------- chats

export interface Chat {
  id: string;
  /** Taken from the user's first message; `ai.chats.rename` changes it. */
  title: string;
  created_at: number;
  updated_at: number;
  provider_id: string;
  /** A chat cannot change the kind of API it talks to; start a new one. */
  provider_kind: ProviderKind;
  model: string;
  /** Messages in the readable history. */
  messages: number;
  /** The last `ai.event` sequence number published for this chat. */
  seq: number;
  /** Bumped when the context is compacted; older generations stay stored. */
  generation: number;
  /** The turn that is running, if any; `null` when none is. */
  pending_turn?: string | null;
  prefix: string;
}

/**
 * What a tool result is about, so the chat can draw a card. The engine leaves
 * a link out — the key is absent, never `null` — when what a feature gave as
 * an id is not one (empty, longer than 128 characters, or holding white
 * space). A `job` id is a core job (`jobs.get`).
 */
export interface AiLink {
  type: 'device' | 'change' | 'lab' | 'alert' | 'job';
  id: string;
}

export type ToolStatus = 'running' | 'ok' | 'error' | 'denied';

/** One block of a message, as the chat window renders it. */
export type UiBlock =
  | { type: 'text'; text: string }
  | { type: 'thinking'; summary: string }
  | {
      type: 'tool';
      call_id: string;
      name: string;
      /** Human title of the tool ("Run commands"). */
      title: string;
      status: ToolStatus;
      /** The arguments, on one line. */
      input_summary: string;
      /** What came back, on one line. */
      result_summary: string;
      /** Absent when the tool pointed at nothing, or at something that is not an id. */
      link?: AiLink;
    }
  | { type: 'notice'; text: string };

export interface TokenUsage {
  input_tokens: number;
  output_tokens: number;
}

export interface UiMessage {
  id: string;
  chat_id: string;
  turn_id: string;
  role: 'user' | 'assistant';
  at: number;
  blocks: UiBlock[];
  usage?: TokenUsage;
}

export interface AiChatsListParams {
  /** Default 50. */
  limit?: number;
}

/** `ai.chats.list` {@link AiChatsListParams} → {@link Chat}[] (most recent first) */

export interface AiChatsCreateParams {
  title?: string;
  /** Defaults to the default provider. */
  provider_id?: string;
  /**
   * Carry a summary of this chat over into the new one — which is how
   * switching to a model of another kind continues a conversation. The
   * summary is computed from the stored messages; no model is called.
   */
  from_chat_id?: string;
}

/** `ai.chats.create` {@link AiChatsCreateParams} → {@link Chat} */

export interface AiChatIdParams {
  id: string;
}

export interface AiChatView {
  chat: Chat;
  messages: UiMessage[];
  /** The newest event sequence number; anything above this is yet to come. */
  seq: number;
}

/** `ai.chats.get` {@link AiChatIdParams} → {@link AiChatView} */

export interface AiChatsRenameParams {
  id: string;
  title: string;
}

/** `ai.chats.rename` {@link AiChatsRenameParams} → {@link Chat} */

/** `ai.chats.delete` {@link AiChatIdParams} → {@link AiDeleted} (secrets of that chat go with it) */

// --------------------------------------------------------------------- turns

/** What the user is looking at when they ask. All optional. */
export interface AiContext {
  device_ids?: string[];
  /** Which screen they are on. */
  page?: string;
  /** Text they selected: terminal output, a configuration, log lines. */
  selection?: string;
  terminal_session_id?: string;
  alert_id?: string;
  change_id?: string;
  insight_id?: string;
  lab_id?: string;
}

export interface AiSendParams {
  chat_id: string;
  text: string;
  context?: AiContext;
}

export interface AiTurnStarted {
  turn_id: string;
}

/**
 * `ai.send` {@link AiSendParams} → {@link AiTurnStarted} — returns at once;
 * follow {@link AiEventEnvelope}. One turn per chat: sending again while one
 * is running fails with `conflict`.
 */

export interface AiCancelParams {
  chat_id: string;
}

export interface AiCancelled {
  cancelled: boolean;
  turn_id?: string;
}

/**
 * `ai.cancel` {@link AiCancelParams} → {@link AiCancelled} — stops the running
 * turn. Reads already in flight finish on their own; nothing is left half
 * done.
 */

export interface AiApproveParams {
  chat_id: string;
  /** From the `approval_needed` event. */
  call_id: string;
  allow: boolean;
}

export interface AiApproved {
  answered: boolean;
}

/**
 * `ai.approve` {@link AiApproveParams} → {@link AiApproved} — answers an
 * `approval_needed` event (see `confirm_reads`). `not_found` means it timed
 * out.
 */

/** A quick action the chat window offers. */
export interface AiPreset {
  id: string;
  title: string;
  description: string;
  /** What has to be in {@link AiContext} for the action to make sense. */
  needs: ('device' | 'selection' | 'alert' | 'change' | 'none')[];
  /** The message to send, which the user may edit first. */
  prompt: string;
}

/** `ai.presets` (no params) → {@link AiPreset}[] */

export interface AiUsageParams {
  /** Milliseconds since the epoch; everything from then on. Default: all. */
  since?: number;
}

export interface AiUsageTotals {
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  turns: number;
}

export interface AiUsageByModel extends AiUsageTotals {
  provider_id: string;
  model: string;
}

/** One hour of the ledger. */
export interface AiUsageHour extends AiUsageByModel {
  /** Start of the hour, in milliseconds since the epoch. */
  hour: number;
}

export interface AiUsage {
  since: number;
  totals: AiUsageTotals;
  by: AiUsageByModel[];
  hours: AiUsageHour[];
}

/** `ai.usage` {@link AiUsageParams} → {@link AiUsage} */

// -------------------------------------------------------------------- events

/** Why a turn ended. */
export type AiStop = 'end' | 'cancelled' | 'max_steps' | 'refused';

/**
 * One item of the live stream. `text` and `thinking` are increments to
 * append; everything else is complete in itself.
 */
export type AiEvent =
  | { type: 'text'; delta: string }
  /** Reasoning summaries only, never a provider's signed thinking. */
  | { type: 'thinking'; delta: string }
  | {
      type: 'tool_start';
      call_id: string;
      name: string;
      title: string;
      input_summary: string;
    }
  | {
      type: 'tool_end';
      call_id: string;
      status: ToolStatus;
      result_summary: string;
      /** Absent when the tool pointed at nothing, or at something that is not an id. */
      link?: AiLink;
    }
  /**
   * A tool is waiting for `ai.approve`. Published after the `tool_start` of
   * the call it is about and before that call's `tool_end`.
   */
  | {
      type: 'approval_needed';
      call_id: string;
      name: string;
      title: string;
      /**
       * Plain text, never markup. For `confirm_reads` it is one line of the
       * call's arguments; for a tool that writes a record
       * (`acknowledge_alert`, `reserve_address`, `start_discovery`) it is the
       * question itself, several lines showing exactly what would be
       * written, with an alert's title, a prefix's name, the note or a
       * scan's ranges quoted in it — text that devices, other people and the
       * model wrote. Render it as text with its line breaks kept. Real one:
       * `"Acknowledge this alert?\n  [high] GigabitEthernet0/3 is down — SW1\n
       *   alert yi3yo246t6q665jk, firing since 2026-11-23 09:44:07Z\nwith the
       * note:\n  \"Cable reseated at the patch panel; watching for an
       * hour.\"\nThe alert stays open until it resolves; nothing is sent to
       * any device."`.
       */
      detail: string;
    }
  /** A fallback model answered, the context was compacted, the turn was cut short. */
  | { type: 'notice'; text: string }
  | { type: 'usage'; input_tokens: number; output_tokens: number }
  | { type: 'done'; stop: AiStop }
  /** The model endpoint failed. A `done` follows; the chat stays usable. */
  | { type: 'error'; code: string; message: string };

/**
 * `ai.event` — everything a turn does, as it happens. `seq` rises by one per
 * chat: a gap means the window missed an event and should call
 * `ai.chats.get`.
 */
export interface AiEventEnvelope {
  chat_id: string;
  turn_id: string;
  seq: number;
  event: AiEvent;
}

/** `ai.chats.changed` — a chat was created, retitled, extended or deleted. */
export interface AiChatsChangedEvent {
  id: string;
  title?: string;
  updated_at?: number;
  deleted?: boolean;
}

/** `ai.providers.changed` — a provider was added, edited or removed. */
export interface AiProvidersChangedEvent {
  id: string;
}

// ------------------------------------------------------------------ settings

/** The `assistant` settings section (`settings.get` / `settings.set`). */
export interface AssistantSettings {
  /** Shared with the `changes` feature, which enforces it. Default `draft`. */
  autonomy: Autonomy;
  /** Ask before every tool call that touches a device. Default false. */
  confirm_reads: boolean;
  /** Tool calls allowed in one turn. Default 40. */
  max_steps: number;
  /** Seconds one turn may take. Default 900. */
  turn_timeout_s: number;
  /** Rows of a table kept in a tool result. Default 50. */
  result_rows: number;
  /** Characters of one tool result kept, head and tail. Default 8000. */
  result_chars: number;
  /**
   * May the model read the ISP customer ledger? Default false, and false
   * means the ledger's tools are not offered and the gate refuses its five
   * reads: nothing of the ledger reaches a model. True lets it see the
   * ledger's totals and, per customer, the id, name, state, plan and
   * balance, and invoices by number, state and amount — never a phone
   * number, an address, an e-mail, an account code or a note. Read when the
   * tools are offered and again on every call, so switching it off closes
   * the ledger mid-turn too. The key exists in the free edition as well,
   * where it is false and changes nothing.
   */
  isp_access: boolean;
  /** The team's own standing instructions, added to every turn. */
  system_prompt_extra: string;
  /** Seconds to wait for an answer to an approval request. Default 300. */
  approval_timeout_s: number;
  /** Turns that may run at once, across all chats. Default 3. */
  max_concurrent_turns: number;
}
