# Changelog

## Unreleased

- Sign in with the system browser when the server uses SSO. Mattermost returns
  a one-time server token on `mattermost://` (or paste that page address). No
  webview and no personal access token. Password, MFA, and tokens stay available.
- Live updates come from the Mattermost websocket. History is cache-first, then
  one refresh when the socket connects or a room was saved by a previous run.
  The lower-left indicator is Live, Updated, Connecting, or Reconnecting. The
  UI sleeps between those events.
- Send, edit, and delete posts. Unread and mention marks. Switch team when the
  account is on more than one.
- Chat icon and `make install` register the binary, icon, and `comport://`.
  `mattermost://` is claimed only when nothing else already handles it.

- Mattermost demo: recorded REST fixtures, left room list, Favorites star/unstar,
  avatars, emoji shortcodes, and a cache-first message page on a worker thread.
  `comport --demo` and `--demo --snapshot PNG`.
- Login form for a self-hosted Mattermost Site URL (password, MFA, or Personal
  Access Token) over HTTPS. README screenshot and onboarding notes.
- Destroy Blade surface, encoder, and egui painter on window close (Starcom
  Drop path). `comport --demo --exit-after-frames N` plus a GPU lifecycle test.
- Repository scaffold: single crate, CI (Linux/macOS/Windows, MSRV, cargo-deny),
  desktop metadata, privacy policy, and the design document.
