# Contributing

Thanks for helping with the Echo VR Launcher.

- **Bugs and ideas:** open an issue. For a bug, include the reference that Settings →
  Upload logs gives you, or say what you did, what happened and what you expected.
- **Code:** [AGENTS.md](AGENTS.md) explains how to build, test and commit. It is written
  for people and coding agents alike. In short, `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings` and `cargo test` must pass. Every UI change
  also needs its screenshots looked at.
- **Plugins:** [docs/plugins](docs/plugins) explains how to write a launcher plugin.

By contributing you agree that your work is licensed under the GPL-3.0-or-later, as the
rest of the launcher is.
