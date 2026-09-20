# ComPort

Native Mattermost desktop client. Pure Rust, Blade + egui + winit. Not a webview
wrapper.

![ComPort showing a Mattermost team with Favorites, channels, avatars, and emoji](etc/screenshot.png)

This is the third [navigato-rs](https://github.com/navigato-rs) app, after
[FileMan](https://github.com/navigato-rs/fileman) and
[Starcom](https://github.com/navigato-rs/starcom). Architecture lives in
[`DESIGN.md`](DESIGN.md).

## Connect to your Mattermost

ComPort talks to a **self-hosted or Mattermost Cloud** server over HTTPS. There
is no ComPort account and no extra service to install on the server.

1. **Site URL** — the same address you open in a browser, including any
   subpath. Examples: `https://chat.company.com`,
   `https://mattermost.company.com/team`. Type it with `https://`, or a hostname
   (`chat.company.com`) and ComPort will use HTTPS.
2. **Username or email + password** — the same credentials as the web app.
   LDAP / AD usernames work if the server accepts them as `login_id`.
3. **MFA** — if the server requires a TOTP code, the form asks for it after the
   first 401.
4. **SSO / SAML / GitLab / Google / Entra** — not in this build (that flow
   belongs to the official desktop’s `mattermost://` handler). Create a
   **Personal Access Token** instead:
   - A system admin enables **Integrations → Integration Management → Enable
     Personal Access Tokens** (or `EnableUserAccessTokens` in `config.json`).
   - You: **Profile menu → Security → Personal Access Tokens → Create**.
   - Paste the token into ComPort. Password can stay empty.
5. Run:

```sh
cargo run --locked -- --server https://chat.company.com
```

or `cargo run --locked` and fill the login form. The last Site URL is remembered
under your config dir (`~/.config/comport` on Linux). The session token stays in
memory for this version (not written to disk).

TLS uses the OS trust store and the experimental rustls-rustcrypto provider
(same bet as FileMan’s evaluation client). Corporate MITM proxies that require
a custom CA must be in the OS trust store.

## Demo (no server)

```sh
cargo run --locked -- --demo
cargo run --locked -- --demo --snapshot /tmp/comport.png
```

## Build

```sh
cargo test --locked --all-features --all-targets
cargo clippy --locked --all-features --all-targets -- -D warnings
python3 scripts/check-dependencies.py
```

On Linux, `make install` puts the binary, `.desktop` entry, and icon under
`~/.local`.

## Privacy

No automatic uploads. See [`PRIVACY.md`](PRIVACY.md).
