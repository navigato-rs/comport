# Feedback and diagnostics

**No automatic uploads.** Feedback opens a GitHub issue draft. Private reporting,
once Help/About lands, opens an email draft. Neither submits anything until you
send it. GitHub issues are public; do not paste tokens, chat contents, or
tenant names.

## What this app will store

- **Tokens** will live in the OS keychain, not in settings files.
- **Message/metadata cache** will be plaintext SQLite under the user profile
  (`0600` on Unix). ComPort does not encrypt chat history. Full-disk encryption
  is the operating system's job.
- **Settings** will be RON under the app config directory. Site URLs are not
  secrets; they still do not belong in diagnostics.

Those stores are not implemented yet. The running binary currently prints a
version and writes nothing.

## Local reports (planned)

Help/About will offer a content-free diagnostics blob: version, git revision,
OS/architecture, GPU backend, log path. Usage flags, when collected, are
ComPort-owned (`connected`, `search`, `upload`, `switcher`, `compose`, `teams`)
plus four timing histograms (`frame`, `cache_page`, `rest_roundtrip`,
`connect_attempt`). They are **not** FileMan `schema: 2` reports.

No message text, URLs, UPNs, tokens, or screenshots go into a report. Usage
collection will be off by default. Debug, demo, replay and snapshot runs will
not record.

## Storage

Planned per-user state directory: `navigato/comport`.
Linux uses absolute `XDG_STATE_HOME` or `~/.local/state`; macOS uses
`~/Library/Application Support`; Windows uses `LOCALAPPDATA`.
Settings stay under the app-named config dir (`~/.config/comport` on Linux).

## Release configuration

Set the organization/repository Actions secret `NAVIGATO_PRIVATE_REPORT_EMAIL`
to a dedicated single-recipient alias if private email reporting is enabled in
a later release. An absent address disables the private-report link. `SENTRY_DSN`
is not an application upload switch.
