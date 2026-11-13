# nocturne-terminal — working notes

Spec: `docs/dev/features/terminal.md`. Conventions: `docs/dev/features/README.md`.

## Status

- [ ] settings, model, ring buffer
- [ ] sessions: registry, pump, coalesced output, attach/snapshot
- [ ] open: device + ad-hoc (ssh/telnet/serial/local), host-key details
- [ ] auto-login
- [ ] session logs (writer, index, list/read/delete)
- [ ] snippets, classify, serial ports, keys
- [ ] SFTP (+ transfer jobs)
- [ ] tests (spec "Tests" section), clippy, fmt
- [ ] `ui/src/api/types/terminal.ts`

## Decisions (where the spec left a choice)

See the bottom of this file; filled in as they are made.
