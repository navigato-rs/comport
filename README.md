# ComPort

Native Mattermost and Microsoft Teams desktop client. Pure Rust, Blade + egui +
winit. Not a webview wrapper.

This is the third [navigato-rs](https://github.com/navigato-rs) app, after
[FileMan](https://github.com/navigato-rs/fileman) and
[Starcom](https://github.com/navigato-rs/starcom). Architecture and the
implementation sequence live in [`DESIGN.md`](DESIGN.md).

The tree is a single crate. Networking, cache, and UI land as modules under
`src/`, not as extra packages. If something needs to be shared with the other
apps, it will move to a dedicated repo.

## Status

Mattermost demo against recorded REST fixtures: left room list, Favorites,
avatars, emoji shortcodes, and a cache-first message page.

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
