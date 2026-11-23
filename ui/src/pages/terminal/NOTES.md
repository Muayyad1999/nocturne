# Terminal screen — working notes

Running log for whoever picks this up next.

## Shape

```
index.tsx           the screen: rail + toolbar + panes + overlays
sessions.ts         the terminal registry — xterm lives outside React
stream.ts           attach/seq bookkeeping, base64 → bytes      (unit tested)
quickconnect.ts     `user@host[:port]`, and a session → target   (unit tested)
paste.ts            lines, line endings, brackets, what to ask   (unit tested)
pasteFunnel.ts      the one road pasted text takes to a device   (unit tested)
snippetDraft.ts     what the snippet form holds and sends back   (unit tested)
layout.ts           zustand: panes, tabs, broadcast; persisted
appearance.ts       design tokens → an xterm ITheme
tokens.css          the sixteen ANSI colours (belong in design/tokens.css)
Pane.tsx            tabs, the quiet header, the surface, find, drop zones
SessionList.tsx     devices, open sessions, recent, quick connect, this machine
PasteSheet.tsx      the seat belt
SnippetsDrawer.tsx  saved commands (click types, never sends)
SftpPanel.tsx       files on an SSH device, transfers as jobs
TransportDetails.tsx what the transport negotiated, on hover
inventory.ts        the little of `devices.list` this screen reads
recents.ts          recently dialled targets (local, no secrets)
assistant.ts        "Explain this" → the dock
```

Tests: `ui/tests/terminal.spec.ts` (20, against a real engine and a real
console), `ui/tests/terminal.shots.ts` (`pnpm shots`, 20 PNGs into
`tests/shots/terminal/`), `ui/tests/terminal-support.ts` (the fake console,
the engine, and a private sshd for the SFTP test). Both Playwright files
start a real `nocturne-server`, so they need cargo or `NOCTURNE_SERVER_BIN`
— a terminal cannot be proved against a mock. The console records what it
ran and the bytes it was sent, because the screen cannot tell a paste that
ran once from one that ran twice, or CR from CRLF. The engine logs in to the
same console on its own to detect the device type, so assert on `said` and
`receivedWith`, not on everything the console saw. `sessions.test.tsx` runs
the registry with the real xterm under happy-dom against a scripted backend.
The support file can also lose output frames on an open socket
(`loseOutputOnRequest`) and present the page as the desktop shell does
(`openInDesktopShell`): the two conditions a plain browser never has, and
under which output went missing and Paste and links did nothing.

## Decisions

* **xterm instances live outside React.** `sessions.ts` is a module singleton
  that owns one `Terminal` per session id in a detached `<div>`. Panes adopt
  the element (`appendChild`); navigating away parks it off screen, still
  receiving output, so coming back is instant and needs no reattach.
* **Output never passes through React.** `terminal.output` is decoded and
  written straight into xterm. React renders only on list/state changes.
  Measured: 3.9 MB in 300 ms with no frame over 17 ms.
* **Attach protocol.** Subscribe first, hold events, then `terminal.attach`;
  write the scrollback, drop the held events the scrollback covers, write the
  rest, go live. On a dropped event stream the registry reattaches: if the
  backend's `seq` ran past what we hold, the screen is cleared and the whole
  scrollback rewritten — never a gap, never a duplicate. `stream.test.ts` has
  the cases; `terminal.spec.ts` proves the end-to-end result.
* **A tab is a session.** Closing a tab closes the session (asked for while
  it is alive), because `terminal.list` is reconciled into the panes on every
  change — a tab that did not close its session would come straight back.
* Keystrokes are sent per session and chained, so bytes cannot overtake each
  other; anything typed during a send rides along with the next one.
* **Pasted text has one road**: `terminals.paste` → `pasteFunnel.ts`. The
  registry takes the browser's `paste` event in the capture phase on the
  element xterm lives in, so xterm's own paste handlers never run — they
  send to the device at once, and `preventDefault` in a listener beside them
  stops nothing. Because it is in the registry, the lab's consoles and the
  map's docked session get the same seat belt (a plain confirm dialog there;
  the sheet here, via `setPasteAsker`). Never call `terminals.input` with
  pasted text, and never add a `paste` listener of your own.
* **Paste chords are the browser's.** Ctrl+Shift+V / ⌘V are not handled:
  xterm is told to ignore the key and the browser's `paste` event does the
  rest, with no clipboard permission. Reading the clipboard is only for the
  toolbar and the context menu (`api/transports/shell.ts`), and its failure
  is said out loud.
* **Line endings and brackets** are decided in the funnel: one CR per line
  break; `ESC[200~…ESC[201~` only when the far end asked and the paste goes
  to one session. The reasons are in `docs/dev/features/terminal.md`.
* **A hole in the output numbering stops the screen** (`stream.ts`) and the
  registry attaches again; `events.lagged` makes every terminal attach.
  Requests are one at a time per session and back off when answers keep
  falling short.
* The backend has no way to say "the caller is remote", so the screen probes
  `terminal.serial_ports`: an error there means the local shell and serial
  ports are out of reach, and the rail repeats the backend's own sentence.
* ANSI palette tokens (`--term-*`) are in `tokens.css` in this folder, not in
  `design/tokens.css` (another engineer owns that file). They should move.
* The toolbar's labels are hidden by a container query under 820px of content
  (1280 with the dock open) rather than wrapping onto a second row.

## Known gaps

* `sftp.upload` / `sftp.download` need local access, so from `nocturne-server`
  they are refused; the panel shows the engine's own sentence in the dialog.
  Browsing, which is what the panel is mostly for, works everywhere.
* "Explain" puts the selection in `assistant.ts` and opens the dock; the
  Assistant slice should read `useAssistantContext` when it lands.
* Reconnecting an ad-hoc session that needed a typed password cannot be
  automatic (the password is never stored): the quick-connect field is
  focused instead.

## Log

* Read the specs, the terminal types and the backend crate; probed a real
  `nocturne-server` with a fake telnet device to learn the live shapes.
* Built the registry, the panes, the rail and the overlays; drove them
  against the real engine the whole way.
* Looked at the screenshots: fixed xterm's black viewport band, a find bar
  that collided with the selection bubble, a toolbar that wrapped at 1280
  with the dock open, and an SFTP path that read backwards.
