/**
 * Interactive terminal sessions, SFTP, saved snippets, session logs and SSH
 * key generation — the `nocturne-terminal` backend crate.
 *
 * Field names are the backend's, verbatim. Timestamps are milliseconds since
 * the Unix epoch. `?` marks a parameter the backend defaults, or a result
 * field that may be `null`.
 *
 * Specification: `docs/dev/features/terminal.md`.
 */

// ---------------------------------------------------------------- sessions

/** How a session reaches the far end. */
export type TransportKind = 'ssh' | 'telnet' | 'serial' | 'local' | 'sim';

export type SessionState = 'connecting' | 'open' | 'closed';

/** What the transport negotiated, as the transport crate reports it. */
export interface ConnectionInfo {
  kind?: TransportKind | null;
  /** `10.0.0.1:22`, `COM3`, `lab:R1`. */
  peer: string;
  /** SSH identification string / pre-authentication banner. */
  server_banner?: string | null;
  kex?: string | null;
  cipher?: string | null;
  mac?: string | null;
  host_key_algorithm?: string | null;
  /** `SHA256:…` */
  host_key_fingerprint?: string | null;
  /** Negotiated only thanks to the legacy algorithm set. */
  legacy_crypto: boolean;
  /** Jump hosts traversed. */
  via: string[];
}

/** A terminal session as the UI sees it. */
export interface SessionInfo {
  id: string;
  /** `SW1`, `admin@10.0.0.1`, `COM3`, `Local shell`. */
  title: string;
  kind: TransportKind;
  /** The inventory device, for a session that is to one. */
  device_id?: string | null;
  /** The device is simulated: whatever happens here happens in a lab. */
  lab: boolean;
  opened_at: number;
  state: SessionState;
  close_reason?: string | null;
  closed_at?: number | null;
  cols: number;
  rows: number;
  info: ConnectionInfo;
  /** Bytes received from the far end. */
  bytes_in: number;
  /** Bytes sent to the far end. */
  bytes_out: number;
  /** Name of the log file the session is (or, once closed, was) written to. */
  logging?: string | null;
  /** Why the session is not logged although it was meant to be. */
  log_error?: string | null;
}

export type AdhocTransport = 'ssh' | 'telnet' | 'serial' | 'local';

export type Parity = 'none' | 'odd' | 'even';

/** `software` is XON/XOFF, `hardware` is RTS/CTS. */
export type FlowControl = 'none' | 'software' | 'hardware';

export interface SerialOptions {
  /** 9600 by default. */
  baud?: number;
  /** 5 to 8; 8 by default. */
  data_bits?: number;
  parity?: Parity;
  /** 1 or 2; 1 by default. */
  stop_bits?: number;
  flow?: FlowControl;
}

export interface LocalOptions {
  /** The user's own shell when omitted. */
  program?: string | null;
  args?: string[];
  /** The user's home directory when omitted. */
  cwd?: string | null;
}

/** A target that is not in the inventory. Every field has a default. */
export interface Adhoc {
  /** `ssh` by default. */
  transport?: AdhocTransport;
  /** Host name or address; for `serial` the port name (`COM3`, `/dev/ttyUSB0`). */
  host?: string;
  /** Omitted or `0`: the transport's default (22 / 23), or the baud rate for serial. */
  port?: number | null;
  username?: string;
  /** Used for this session only; never stored, logged or echoed back. */
  password?: string | null;
  /** Take the user name and secrets from a stored credential instead. */
  credential_id?: string | null;
  /** Reach the target through this inventory device (SSH only). */
  jump_device_id?: string | null;
  serial?: SerialOptions;
  local?: LocalOptions;
  /** Overrides the title the backend would derive from the target. */
  title?: string | null;
}

/** Exactly one of `device_id` and `adhoc`. */
export interface TerminalOpenParams {
  /** A device in the inventory, by id (its name or address also works). */
  device_id?: string;
  adhoc?: Adhoc;
  /** 20…500; 120 by default. */
  cols?: number;
  /** 5…200; 40 by default. */
  rows?: number;
  /** Write a session log; defaults to the `terminal.log_sessions` setting. */
  log?: boolean;
}

export interface TerminalInputParams {
  session_id: string;
  /** UTF-8 keystrokes or a paste, sent as typed. */
  data: string;
}

export interface TerminalInputManyParams {
  session_ids: string[];
  data: string;
}

export interface TerminalInputNotSent {
  session_id: string;
  code: string;
  message: string;
}

export interface TerminalInputManyResult {
  /** Sessions the input reached. */
  sent: number;
  /** Sessions it did not, and why. */
  failed: TerminalInputNotSent[];
}

export interface TerminalResizeParams {
  session_id: string;
  cols: number;
  rows: number;
}

export interface TerminalSessionRef {
  session_id: string;
}

export interface TerminalAttached {
  info: SessionInfo;
  /** Base64 of the retained raw output. */
  scrollback: string;
  /**
   * The last output event the scrollback includes. Apply `terminal.output`
   * events with a higher `seq` to it, and ignore any with this one or lower.
   */
  seq: number;
}

export interface TerminalSnapshotParams {
  session_id: string;
  /** Lines from the end of the scrollback; 200 by default, 10 000 at most. */
  lines?: number;
  /** `false` returns the text as the device printed it; secrets are hidden by default. */
  redact?: boolean;
}

export interface TerminalSnapshotResult {
  /** Plain text: escape sequences, redraws and pager prompts resolved. */
  text: string;
  lines: number;
  /** Something in the output was hidden. */
  redacted: boolean;
}

export interface TerminalLoggingParams {
  session_id: string;
  /** `true` (the default) starts a log, `false` completes it. */
  enabled?: boolean;
}

/** What a platform's policy thinks a command would do. */
export type CommandClass = 'read_only' | 'change' | 'dangerous';

export interface TerminalClassifyParams {
  /** The device whose policy decides; the generic policy without it. */
  device_id?: string | null;
  /** What is about to be typed or pasted. At most 20 000 lines. */
  text: string;
}

/** One entry per line of `text`, blank lines included and in the same order. */
export interface TerminalLineClass {
  line: string;
  class: CommandClass;
  /** Why, in words to show the user. */
  reason: string;
}

/** A serial port of the machine Nocturne runs on. */
export interface SerialPortInfo {
  /** What to pass as `Adhoc.host`. */
  name: string;
  description: string;
  manufacturer?: string | null;
  usb_vid?: number | null;
  usb_pid?: number | null;
}

// ---------------------------------------------------------------- snippets

/**
 * A placeholder in a snippet's text: `{{name}}` or `{{name=default}}`.
 * `default` is absent when there is none and `""` when it is empty: the two
 * are different (a variable without one must be given a value).
 */
export interface SnippetVariable {
  name: string;
  default?: string;
}

/**
 * A snippet as `terminal.snippets.list` and `.save` return it: what is
 * stored, and what its text says.
 *
 * `text` is as it was written, placeholders and all — and for a snippet
 * saved before today's rules it can hold anything, an escape character
 * included. It is text a person typed for a device: show it as text.
 */
export interface Snippet {
  id: string;
  name: string;
  /** The text as written. What is typed is `terminal.snippets.render`'s `text`. */
  text: string;
  /** The device profile this is for; `null` means any. */
  profile?: string | null;
  tags: string[];
  created_at: number;
  updated_at: number;
  /** The variables in the text, in the order they first appear. Empty for one with a `problem`. */
  variables: SnippetVariable[];
  /**
   * Why this snippet cannot be rendered or sent as it is, in the engine's
   * words (`line 1, column 16: …`). Only on one stored before today's rules;
   * saving it again, corrected, clears it.
   */
  problem?: string;
  /**
   * Whether this is the one to use in the session (or for the device type)
   * the list was asked for. Absent when it was asked for neither.
   */
  chosen?: boolean;
  /** The id of the variant that is used instead of this one. Only with `chosen: false`, and not always then. */
  chosen_instead?: string;
}

/** `session_id` or `profile`, not both. */
export interface SnippetListParams {
  /** Only snippets for this profile, and those for any profile; adds `chosen`. */
  profile?: string | null;
  /** The snippets for this session: its device type decides, and every row gets `chosen`. */
  session_id?: string | null;
  /** Words that must all appear in the name, tags or text. Applied after the choice among variants. */
  query?: string | null;
}

export interface SnippetSaveParams {
  /** Omitted: a new snippet. Given: the snippet to replace. */
  id?: string | null;
  name: string;
  text: string;
  profile?: string | null;
  tags?: string[];
}

/** `details` of a `terminal.snippets.save` refused for its text: where the fault is, both from 1. */
export interface SnippetTextFault {
  line: number;
  column: number;
}

/** `id`, `ids`, or both. */
export interface SnippetDeleteParams {
  id?: string | null;
  ids?: string[];
}

export interface SnippetDeleteResult {
  deleted: number;
}

export interface SnippetRenderParams {
  id: string;
  /** The text for each variable, by its name. Left out for a variable that has a default. */
  values?: Record<string, string> | null;
}

export interface SnippetRendered {
  /** What will be typed: every line ending is one `\n`, and the one that ends the snippet is dropped. */
  text: string;
  /** Lines in `text`; 0 for an empty one. */
  lines: number;
  /** SHA-256 of `text` as UTF-8, lower-case hex. What `terminal.snippets.send` is held to. */
  sha256: string;
  variables: SnippetVariable[];
}

/** What is wrong with one variable's value. */
export type SnippetValueProblem =
  | 'missing'
  | 'unknown'
  | 'not_text'
  | 'line_break'
  | 'control_character'
  | 'too_long';

/**
 * `details` of a `terminal.snippets.render` or `.send` refused (`invalid`)
 * for its values. The message says the same in words, one clause per entry
 * and in this order, joined with `; `, and never repeats a value.
 */
export interface SnippetValuesRefused {
  variables: { name: string; problem: SnippetValueProblem }[];
}

export interface SnippetSendParams {
  id: string;
  values?: Record<string, string> | null;
  /** 1 to 64 sessions, none twice. */
  session_ids: string[];
  /** The `sha256` of the render the person was shown. */
  expect_sha256: string;
}

export interface SnippetSent {
  session_id: string;
  lines: number;
}

/** A session a snippet did not reach, and why. */
export interface SnippetNotSent {
  session_id: string;
  code: string;
  message: string;
}

export interface SnippetSendResult {
  /** The sessions that took the text. */
  sent: SnippetSent[];
  /**
   * Sessions whose connection stalled or dropped between the check and the
   * typing. What the others received cannot be taken back.
   */
  failed: SnippetNotSent[];
}

/** `details` of a send refused before anything was typed: nothing reached any session. */
export interface SnippetSendRefused {
  refused: SnippetNotSent[];
}

/** `details` of a send refused as `conflict`: the hash the snippet renders to now. */
export interface SnippetSendChanged {
  sha256: string;
}

/** A snippet as a person wrote it: no id, no timestamps. */
export interface SnippetDocumentEntry {
  name: string;
  text: string;
  profile?: string | null;
  tags?: string[];
}

/** What `terminal.snippets.export` returns and `.import` reads. */
export interface SnippetsDocument {
  format: 'nocturne.snippets/1';
  snippets: SnippetDocumentEntry[];
}

/** What happens to an entry whose name and device type a snippet here already has. */
export type SnippetImportConflict = 'skip' | 'replace' | 'rename';

export interface SnippetImportParams {
  /** The document, or its JSON text as read from a file. */
  document: SnippetsDocument | string;
  /** `skip` by default: it loses nothing. */
  on_conflict?: SnippetImportConflict;
  /** Answer exactly what the import would, and write nothing. */
  dry_run?: boolean;
}

export interface SnippetImportProblem {
  /** The entry's place in the document, from 0. */
  index: number;
  /** Only when the entry has a name; cut to 80 characters. */
  name?: string;
  problem: string;
}

export interface SnippetImportResult {
  /** Snippets added, those brought in under a new name included. */
  created: number;
  replaced: number;
  /** Entries that were left out: one here already says the same, or `skip` kept the one here. */
  skipped: number;
  /** Of `created`, those that came in as `name (2)`. */
  renamed: number;
  /** While this is not empty nothing is imported and every count is 0. */
  problems: SnippetImportProblem[];
  /** The snippets now are as this answer says. Always false for a dry run. */
  applied: boolean;
  dry_run: boolean;
}

// ------------------------------------------------------------ session logs

export interface SessionLogInfo {
  /** `YYYY-MM/<title>-<YYYYmmdd-HHMMSS>.log`, relative to the sessions directory. */
  name: string;
  device_id?: string | null;
  title: string;
  kind: TransportKind;
  started_at: number;
  /** `null` while the session runs, and for one Nocturne did not see end. */
  ended_at?: number | null;
  /** Size of the file in bytes. */
  size: number;
  /** A live session is still writing to it. */
  active: boolean;
}

export interface TerminalLogsListParams {
  device_id?: string | null;
  /** Words that must all appear in the title or the file name. */
  query?: string | null;
  /** 100 by default, 1000 at most. */
  limit?: number;
  /** Continue after this log: the last `name` of the previous page. */
  before?: string | null;
}

export interface TerminalLogsReadParams {
  name: string;
  /** Byte offset into the file; 0 by default. */
  offset?: number;
  /** Most bytes to read; 256 KiB by default, 1 MiB at most. */
  limit?: number;
  /** `false` returns the text as written; device secrets are hidden by default. */
  redact?: boolean;
}

export interface TerminalLogsReadResult {
  text: string;
  /**
   * Where this page starts in the file. Not always the `offset` that was
   * asked for: a page is whole lines, so it starts with the line that offset
   * falls in, which can be earlier.
   */
  offset: number;
  /** Size of the whole file in bytes. */
  size: number;
  eof: boolean;
  /**
   * Where to continue reading. Not `offset + text.length`: redaction changes
   * lengths, and a read ends on a line boundary.
   */
  next_offset: number;
  redacted: boolean;
}

export interface TerminalLogsDeleteParams {
  names: string[];
}

export interface TerminalLogNotDeleted {
  name: string;
  code: string;
  message: string;
}

export interface TerminalLogsDeleteResult {
  deleted: number;
  failed: TerminalLogNotDeleted[];
}

// -------------------------------------------------------------------- SFTP

export interface SftpSessionInfo {
  sftp_id: string;
  device_id: string;
  device_name: string;
  peer: string;
  /** The directory the device puts the user in. */
  home: string;
  opened_at: number;
}

export interface SftpEntry {
  name: string;
  path: string;
  /** For a symbolic link: whether it points to a directory. */
  is_dir: boolean;
  is_link: boolean;
  size: number;
  /** Seconds since the Unix epoch. */
  modified?: number | null;
  /** The permission bits (`0o7777` mask), e.g. `0o644`. */
  permissions?: number | null;
}

export interface SftpOpenParams {
  /** An inventory device reached over SSH. */
  device_id: string;
}

export interface SftpRef {
  sftp_id: string;
}

export interface SftpPathParams {
  sftp_id: string;
  /** Empty means the session's home directory (for `sftp.list`). */
  path: string;
}

export interface SftpListing {
  /** The directory actually listed. */
  path: string;
  /** Directories first, then files, each group by name. */
  entries: SftpEntry[];
}

export interface SftpRenameParams {
  sftp_id: string;
  from: string;
  to: string;
}

export interface SftpTextResult {
  text: string;
  /** Size of the file in bytes. */
  size: number;
}

export interface SftpWriteTextParams {
  sftp_id: string;
  path: string;
  text: string;
}

export interface SftpWriteResult {
  /** Bytes written. */
  size: number;
}

export interface SftpTransferParams {
  sftp_id: string;
  /** Path on the device. */
  remote: string;
  /** Absolute path on the machine Nocturne runs on. */
  local: string;
}

/** What a method that starts a job returns; follow it with `jobs.*`. */
export interface JobStarted {
  job_id: string;
}

/** The job's `result` once a transfer has finished. */
export interface SftpTransferResult {
  bytes: number;
  remote: string;
  local: string;
}

// -------------------------------------------------------------------- keys

export type KeyKind = 'ed25519' | 'rsa3072' | 'rsa4096';

export interface KeysGenerateParams {
  /** `ed25519` by default. */
  kind?: KeyKind;
  /** Appended to the public key line, to tell keys apart on a device. */
  comment?: string;
}

/** Returned once and kept nowhere: store it in a credential or a file. */
export interface GeneratedKey {
  /** OpenSSH format, unencrypted. */
  private_key: string;
  /** The `ssh-ed25519 AAAA… comment` line for `authorized_keys`. */
  public_key: string;
  kind: KeyKind;
  /** `SHA256:…`, as `ssh-keygen -l` prints it. */
  fingerprint?: string | null;
}

// ---------------------------------------------------------------- settings

/** Settings section `terminal`. */
export interface TerminalSettings {
  /** Raw output kept per session for re-attaching, in KiB (64…16384). Default 512. */
  scrollback_kib: number;
  /** Log every session to disk unless `terminal.open` says otherwise. Default false. */
  log_sessions: boolean;
  /** Answer the login prompts of Telnet, serial and simulated consoles. Default true. */
  auto_login: boolean;
  /** Close sessions nobody typed into for this many minutes; 0 never does. Default 0. */
  idle_timeout_min: number;
  /** Send SSH keep-alives (at the interval of the `connections` settings). Default true. */
  keepalive: boolean;
}

// ------------------------------------------------------------------ events

/**
 * `terminal.output` — a piece of the session's byte stream. Bursts are
 * coalesced: at most one event every ~8 ms, and at most 32 KiB per event.
 * `seq` counts from 1 without gaps, so a client that attached applies only
 * the events above the `seq` `terminal.attach` returned.
 */
export interface TerminalOutputEvent {
  session_id: string;
  seq: number;
  /** Base64 of the raw bytes. A character may straddle two events. */
  data: string;
}

/** `terminal.closed` — the session has ended; it is still listed until closed. */
export interface TerminalClosedEvent {
  session_id: string;
  reason: string;
}

/** `terminal.removed` — the backend has forgotten the session entirely. */
export interface TerminalRemovedEvent {
  session_id: string;
}

/** `terminal.snippets.changed` — snippets were saved or deleted. */
export interface TerminalSnippetsChangedEvent {
  saved?: string[];
  deleted?: string[];
}

/** `terminal.logs.changed` — session logs were deleted. */
export interface TerminalLogsChangedEvent {
  deleted: string[];
}

/** `sftp.closed` — an SFTP session has gone (closed, or idle too long). */
export interface SftpClosedEvent {
  sftp_id: string;
  reason?: string;
}

/**
 * The event topics this feature publishes, and the payload of each.
 *
 *   terminal.opened            SessionInfo
 *   terminal.output            TerminalOutputEvent
 *   terminal.changed           SessionInfo            (resized, logging toggled)
 *   terminal.closed            TerminalClosedEvent
 *   terminal.removed           TerminalRemovedEvent
 *   terminal.snippets.changed  TerminalSnippetsChangedEvent
 *   terminal.logs.changed      TerminalLogsChangedEvent
 *   sftp.opened                SftpSessionInfo
 *   sftp.closed                SftpClosedEvent
 */
export interface TerminalEvents {
  'terminal.opened': SessionInfo;
  'terminal.output': TerminalOutputEvent;
  'terminal.changed': SessionInfo;
  'terminal.closed': TerminalClosedEvent;
  'terminal.removed': TerminalRemovedEvent;
  'terminal.snippets.changed': TerminalSnippetsChangedEvent;
  'terminal.logs.changed': TerminalLogsChangedEvent;
  'sftp.opened': SftpSessionInfo;
  'sftp.closed': SftpClosedEvent;
}

// ----------------------------------------------------------------- methods

/** The result of a method that only reports that it worked. */
export type Empty = Record<string, never>;

/**
 * Every RPC method of this feature: its name, its parameters and its result.
 *
 *   terminal.open              TerminalOpenParams        -> SessionInfo
 *   terminal.input             TerminalInputParams       -> Empty
 *   terminal.input_many        TerminalInputManyParams   -> TerminalInputManyResult
 *   terminal.resize            TerminalResizeParams      -> Empty
 *   terminal.send_break        TerminalSessionRef        -> Empty
 *   terminal.attach            TerminalSessionRef        -> TerminalAttached
 *   terminal.snapshot          TerminalSnapshotParams    -> TerminalSnapshotResult
 *   terminal.close             TerminalSessionRef        -> Empty
 *   terminal.list              void                      -> SessionInfo[]
 *   terminal.logging           TerminalLoggingParams     -> SessionInfo
 *   terminal.classify          TerminalClassifyParams    -> TerminalLineClass[]
 *   terminal.serial_ports      void                      -> SerialPortInfo[]
 *   terminal.snippets.list     SnippetListParams         -> Snippet[]
 *   terminal.snippets.save     SnippetSaveParams         -> Snippet
 *   terminal.snippets.delete   SnippetDeleteParams       -> SnippetDeleteResult
 *   terminal.snippets.render   SnippetRenderParams       -> SnippetRendered
 *   terminal.snippets.send     SnippetSendParams         -> SnippetSendResult
 *   terminal.snippets.export   void                      -> SnippetsDocument
 *   terminal.snippets.import   SnippetImportParams       -> SnippetImportResult
 *   terminal.logs.list         TerminalLogsListParams    -> SessionLogInfo[]
 *   terminal.logs.read         TerminalLogsReadParams    -> TerminalLogsReadResult
 *   terminal.logs.delete       TerminalLogsDeleteParams  -> TerminalLogsDeleteResult
 *   sftp.open                  SftpOpenParams            -> SftpSessionInfo
 *   sftp.sessions              void                      -> SftpSessionInfo[]
 *   sftp.list                  SftpPathParams            -> SftpListing
 *   sftp.stat                  SftpPathParams            -> SftpEntry
 *   sftp.mkdir                 SftpPathParams            -> Empty
 *   sftp.remove                SftpPathParams            -> Empty
 *   sftp.rename                SftpRenameParams          -> Empty
 *   sftp.close                 SftpRef                   -> Empty
 *   sftp.read_text             SftpPathParams            -> SftpTextResult
 *   sftp.write_text            SftpWriteTextParams       -> SftpWriteResult
 *   sftp.download              SftpTransferParams        -> JobStarted
 *   sftp.upload                SftpTransferParams        -> JobStarted
 *   keys.generate              KeysGenerateParams        -> GeneratedKey
 */
export interface TerminalMethods {
  /** Open a session to an inventory device or an ad-hoc target. Returns once connected; errors carry the transport's code (`auth`, `host_key`, `refused`, …), and a changed host key carries both fingerprints in `details`. */
  'terminal.open': { params: TerminalOpenParams; result: SessionInfo };
  /** Send keystrokes or a paste to one session. */
  'terminal.input': { params: TerminalInputParams; result: Empty };
  /** Send the same keystrokes to several sessions; says which took them. */
  'terminal.input_many': { params: TerminalInputManyParams; result: TerminalInputManyResult };
  /** Tell the far end the terminal's new size. */
  'terminal.resize': { params: TerminalResizeParams; result: Empty };
  /** Send a break condition (serial consoles and Telnet console servers). */
  'terminal.send_break': { params: TerminalSessionRef; result: Empty };
  /** Get the scrollback and the number of the last event it covers. */
  'terminal.attach': { params: TerminalSessionRef; result: TerminalAttached };
  /** The last N lines as plain text, for "explain this", copying and saving. `redact: false` is audited. */
  'terminal.snapshot': { params: TerminalSnapshotParams; result: TerminalSnapshotResult };
  /** End the session and forget it. */
  'terminal.close': { params: TerminalSessionRef; result: Empty };
  /** Every session the backend holds, oldest first, closed ones included. */
  'terminal.list': { params: void; result: SessionInfo[] };
  /** Start or stop writing a session log while the session runs. */
  'terminal.logging': { params: TerminalLoggingParams; result: SessionInfo };
  /** What the device's policy thinks of each line about to be sent. */
  'terminal.classify': { params: TerminalClassifyParams; result: TerminalLineClass[] };
  /** The serial ports of this machine (desktop app only). */
  'terminal.serial_ports': { params: void; result: SerialPortInfo[] };
  /** Saved commands, by name. */
  'terminal.snippets.list': { params: SnippetListParams; result: Snippet[] };
  /** Create a snippet, or replace the one named by `id`. */
  'terminal.snippets.save': { params: SnippetSaveParams; result: Snippet };
  /** Delete snippets. */
  'terminal.snippets.delete': { params: SnippetDeleteParams; result: SnippetDeleteResult };
  /** A snippet with its values put in: the exact text that would be typed, and its hash. Types nothing. */
  'terminal.snippets.render': { params: SnippetRenderParams; result: SnippetRendered };
  /** Type a snippet into sessions, held to the hash of the text the person confirmed; every session or none. Audited. */
  'terminal.snippets.send': { params: SnippetSendParams; result: SnippetSendResult };
  /** Every snippet as a person wrote it, by name. */
  'terminal.snippets.export': { params: void; result: SnippetsDocument };
  /** Bring a document of snippets in: the whole document or none of it, and `dry_run` to see first. */
  'terminal.snippets.import': { params: SnippetImportParams; result: SnippetImportResult };
  /** Session logs on disk, newest first. */
  'terminal.logs.list': { params: TerminalLogsListParams; result: SessionLogInfo[] };
  /** Read a page of a session log: whole lines, starting with the one `offset` falls in. `redact: false` is audited. */
  'terminal.logs.read': { params: TerminalLogsReadParams; result: TerminalLogsReadResult };
  /** Delete session logs; one that a live session is writing is refused. */
  'terminal.logs.delete': { params: TerminalLogsDeleteParams; result: TerminalLogsDeleteResult };
  /** Open an SFTP session on an SSH device. */
  'sftp.open': { params: SftpOpenParams; result: SftpSessionInfo };
  /** The SFTP sessions the backend holds. */
  'sftp.sessions': { params: void; result: SftpSessionInfo[] };
  /** List a remote directory. */
  'sftp.list': { params: SftpPathParams; result: SftpListing };
  /** One remote file or directory. */
  'sftp.stat': { params: SftpPathParams; result: SftpEntry };
  /** Create a remote directory. */
  'sftp.mkdir': { params: SftpPathParams; result: Empty };
  /** Delete a remote file, link or empty directory. */
  'sftp.remove': { params: SftpPathParams; result: Empty };
  /** Rename or move a remote file. */
  'sftp.rename': { params: SftpRenameParams; result: Empty };
  /** Close an SFTP session. */
  'sftp.close': { params: SftpRef; result: Empty };
  /** Read a remote file as text (at most 1 MiB, UTF-8 only). */
  'sftp.read_text': { params: SftpPathParams; result: SftpTextResult };
  /** Create or replace a remote file from text (at most 1 MiB). */
  'sftp.write_text': { params: SftpWriteTextParams; result: SftpWriteResult };
  /** Copy a remote file to this machine, as a job with byte progress. */
  'sftp.download': { params: SftpTransferParams; result: JobStarted };
  /** Copy a file from this machine to the device, as a job with byte progress. */
  'sftp.upload': { params: SftpTransferParams; result: JobStarted };
  /** Generate an SSH key pair. It is returned once and stored nowhere. */
  'keys.generate': { params: KeysGenerateParams; result: GeneratedKey };
}
