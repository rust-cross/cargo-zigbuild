# AGENTS.md

This file guides automated contributors working in this repository.

## Project overview
- `cargo-zigbuild` is a Rust CLI that builds Rust projects using Zig as the linker.
- The main Rust sources live under `src/`.
- Docs and usage details are in `README.md`.

## Setup
- Install Rust (stable) via rustup.
- Install Zig (or the `ziglang` Python package) if you need to run `cargo zigbuild`.

## Common commands
- Build: `cargo build`
- Integration tests (see `./.github/workflows/CI.yml`): `cargo run zigbuild --manifest-path tests/<fixture>/Cargo.toml --target <target>`
- Lint (if installed): `cargo clippy --all-targets --all-features`
- Format (if installed): `cargo fmt --all`

## Expectations for changes
- Keep changes focused and avoid unrelated formatting.
- Prefer updating `README.md` when user-facing behavior changes.
- Add or update tests when fixing bugs or adding features.

## Notes
- Some functionality depends on external tooling (Zig, SDKs). Call out any required environment setup in your change summary.
