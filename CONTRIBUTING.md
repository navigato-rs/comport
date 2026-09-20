# Contributing

ComPort follows [FileMan](https://github.com/navigato-rs/fileman) and
[Starcom](https://github.com/navigato-rs/starcom). Architecture is in
[`DESIGN.md`](DESIGN.md).

## Code style

- Keep dependencies and code volume low. Simple is good.
- **One package.** Split modules; do not add workspace crates. If something
  needs to be shared with FileMan or Starcom, extract a dedicated repo.
- One `use` per crate. Prefer modules over long lists of imported members.
- Do not rely on implicit references in `match`; use explicit `ref` bindings.
- Use enums instead of boolean arguments that obscure meaning.
- Use `anyhow` for application-level context.
- Never log tokens, passwords, message bodies, or Authorization headers.

## Organization

Create modules when they are implemented, not as empty placeholders.
Use `tests/data/` for protocol fixtures, `scripts/` for development tools,
and `etc/` for desktop metadata.

Use FileMan's Blade/egui/winit versions together when introducing the GUI.
Do not add `eframe`, a webview, `tokio`, `reqwest`, `native-tls`, or OpenSSL.
Do not depend on FileMan's `navigato-support` or `navigato-http` crates.

Commit `Cargo.lock` once dependency resolution has actually been performed.

## Workflow

Landing on `main` is fine while the project is this small. Keep commits
reviewable. Run these before pushing:

```sh
cargo fmt --check
cargo clippy --locked --all-features --all-targets -- -D warnings
cargo test --locked --all-features --all-targets
python3 scripts/check-dependencies.py
cargo deny --all-features check
```

CI runs on pull requests and pushes to `main` (Linux, macOS, Windows, MSRV,
cargo-deny).

Keep `DESIGN.md` honest: implemented vs still a plan.

## Native dependency policy

Use Rust TLS and cryptography implementations. Do not add OpenSSL, ring,
AWS-LC, native-tls, or libssh2 through transitive features. The CI policy
script checks the resolved graph. `rusqlite` (bundled SQLite) is C, not
crypto; app CI has a real C compiler. Crypto policy is `deny.toml` plus
`scripts/check-dependencies.py`, not a fake `CC`.
