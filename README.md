# cargo-zigbuild

[![CI](https://github.com/messense/cargo-zigbuild/workflows/CI/badge.svg)](https://github.com/messense/cargo-zigbuild/actions?query=workflow%3ACI)
[![Crates.io](https://img.shields.io/crates/v/cargo-zigbuild.svg)](https://crates.io/crates/cargo-zigbuild)

Compile Cargo project with [zig as linker](https://andrewkelley.me/post/zig-cc-powerful-drop-in-replacement-gcc-clang.html) for [easier cross compiling](https://actually.fyi/posts/zig-makes-rust-cross-compilation-just-work/).

## Installation

```bash
cargo install cargo-zigbuild
```

## Usage

1. Install [zig](https://ziglang.org/) following the [official documentation](https://ziglang.org/learn/getting-started/#installing-zig),
on macOS, Windows and Linux you can also install zig from PyPI via `pip3 install ziglang`
2. Install Rust target via rustup, for example, `rustup target add aarch64-unknown-linux-gnu`
3. Run `cargo zigbuild`, for example, `cargo zigbuild --target aarch64-unknown-linux-gnu`

### Specify libc version

`cargo zigbuild` supports passing libc version in `--target` option, for example,
to compile for glibc 2.17 with the `aarch64-unknown-linux-gnu` target:

```bash
cargo zigbuild --target aarch64-unknown-linux-gnu.2.17
```

## Caveats

1. Currently only Linux, macOS and Windows gnu targets are supported,
   other target platforms can be added if you can make it work,
   pull requests are welcome.
2. If the `--target` argument is the same as the host target,
   for example when compiling from Linux x86\_64 to Linux x86\_64,
   Cargo by default also uses zig as linker for build dependencies like build scripts and proc-macros
   which might not work (See [#4](https://github.com/messense/cargo-zigbuild/issues/4)).
   You need to use the nightly Rust compiler and enable the unstable [`target-applies-to-host`](https://doc.rust-lang.org/nightly/cargo/reference/unstable.html#target-applies-to-host) option
   and set `target-applies-to-host = false` in cargo configuration file, for example `.cargo/config.toml`, to make it work.

Known upstream zig issues:

1. [zig cc: parse `-target` and `-mcpu`/`-march`/`-mtune` flags according to clang](https://github.com/ziglang/zig/issues/4911):
   Some Rust targets aren't recognized by `zig cc`, for example `armv7-unknown-linux-gnueabihf`

## License

This work is released under the MIT license. A copy of the license is provided
in the [LICENSE](./LICENSE) file.
