# cargo-zigbuild

[![CI](https://github.com/messense/cargo-zigbuild/workflows/CI/badge.svg)](https://github.com/messense/cargo-zigbuild/actions?query=workflow%3ACI)
[![Crates.io](https://img.shields.io/crates/v/cargo-zigbuild.svg)](https://crates.io/crates/cargo-zigbuild)

Compile Cargo project with zig as linker.

## Installation

```bash
cargo install cargo-zigbuild
```

## Usage

1. Install [zig](https://ziglang.org/) following the [official documentation](https://ziglang.org/download/),
on macOS, Windows and Linux you can also install zig from PyPI via `pip3 install ziglang`
2. Install Rust target via rustup, for example, `rustup target add aarch64-unknown-linux-gnu`
3. Run `cargo zigbuild`, for example, `cargo zigbuild --target aarch64-unknown-linux-gnu`

### Specify libc version

`cargo zigbuild` supports passing libc version in `--target` option, for example,
to compile for glibc 2.17 with the `aarch64-unknown-linux-gnu` target:

```bash
cargo zigbuild --target aarch64-unknown-linux-gnu.2.17
```

## Limitations

Currently only Linux targets are supported, other target platforms can be added if you can make it work,
pull requests are welcome.

## License

This work is released under the MIT license. A copy of the license is provided
in the [LICENSE](./LICENSE) file.
