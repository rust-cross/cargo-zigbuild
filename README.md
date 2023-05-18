# cargo-zigbuild

[![CI](https://github.com/rust-cross/cargo-zigbuild/workflows/CI/badge.svg)](https://github.com/rust-cross/cargo-zigbuild/actions?query=workflow%3ACI)
[![Crates.io](https://img.shields.io/crates/v/cargo-zigbuild.svg)](https://crates.io/crates/cargo-zigbuild)
[![docs.rs](https://docs.rs/cargo-zigbuild/badge.svg)](https://docs.rs/cargo-zigbuild/)
[![PyPI](https://img.shields.io/pypi/v/cargo-zigbuild.svg)](https://pypi.org/project/cargo-zigbuild)
[![Docker Image](https://img.shields.io/docker/pulls/messense/cargo-zigbuild.svg?maxAge=2592000)](https://hub.docker.com/r/messense/cargo-zigbuild/)

> 🚀 Help me to become a full-time open-source developer by [sponsoring me on GitHub](https://github.com/sponsors/messense)

Compile Cargo project with [zig](https://github.com/ziglang/zig) as [linker](https://andrewkelley.me/post/zig-cc-powerful-drop-in-replacement-gcc-clang.html) for
[easier cross compiling](https://actually.fyi/posts/zig-makes-rust-cross-compilation-just-work/).

## Installation

```bash
cargo install cargo-zigbuild
```

You can also install it using pip which will also install [`ziglang`](https://pypi.org/project/ziglang/) automatically:

```bash
pip install cargo-zigbuild
```

We also provide a [Docker image](https://hub.docker.com/r/messense/cargo-zigbuild) which has macOS SDK pre-installed in addition to cargo-zigbuild and Rust,
for example to build for x86_64 macOS:

```bash
docker run --rm -it -v $(pwd):/io -w /io messense/cargo-zigbuild \
  cargo zigbuild --release --target x86_64-apple-darwin
```

[![Packaging status](https://repology.org/badge/vertical-allrepos/cargo-zigbuild.svg?columns=4)](https://repology.org/project/cargo-zigbuild/versions)

## Usage

1. Install [zig](https://ziglang.org/) following the [official documentation](https://ziglang.org/learn/getting-started/#installing-zig),
on macOS, Windows and Linux you can also install zig from PyPI via `pip3 install ziglang`
2. Install Rust target via rustup, for example, `rustup target add aarch64-unknown-linux-gnu`
3. Run `cargo zigbuild`, for example, `cargo zigbuild --target aarch64-unknown-linux-gnu`

### Specify glibc version

`cargo zigbuild` supports passing glibc version in `--target` option, for example,
to compile for glibc 2.17 with the `aarch64-unknown-linux-gnu` target:

```bash
cargo zigbuild --target aarch64-unknown-linux-gnu.2.17
```

### macOS universal2 target

`cargo zigbuild` supports a special `universal2-apple-darwin` target for building macOS universal2 binaries/libraries on Rust 1.64.0 and later.

```bash
rustup target add x86_64-apple-darwin
rustup target add aarch64-apple-darwin
cargo zigbuild --target universal2-apple-darwin
```

> **Note**
>
> Note that Cargo `--message-format` option doesn't work with universal2 target currently.

## Caveats

1. Currently only Linux, macOS and Windows gnu targets are supported,
   other target platforms can be added if you can make it work,
   pull requests are welcome.
2. Only current Rust **stable** and **nightly** versions are regularly tested on CI, other versions may not work.

Known upstream zig [issues](https://github.com/ziglang/zig/labels/zig%20cc):

1. [zig cc: parse `-target` and `-mcpu`/`-march`/`-mtune` flags according to clang](https://github.com/ziglang/zig/issues/4911):
   Some Rust targets aren't recognized by `zig cc`, for example `armv7-unknown-linux-gnueabihf`, workaround by using `-mcpu=generic` and
   explicitly passing target features in [#58](https://github.com/rust-cross/cargo-zigbuild/pull/58)
2. [ability to link against darwin frameworks (such as CoreFoundation) when cross compiling](https://github.com/ziglang/zig/issues/1349):
   Set the `SDKROOT` environment variable to a macOS SDK path to workaround it
3. [zig misses some `compiler_rt` functions](https://github.com/ziglang/zig/issues/1290) that may lead to undefined symbol error for certain
   targets. See also: [zig compiler-rt status](https://github.com/ziglang/zig/blob/master/lib/compiler_rt/README.md).
4. [CPU features are not passed to clang](https://github.com/ziglang/zig/issues/10411)

## License

This work is released under the MIT license. A copy of the license is provided
in the [LICENSE](./LICENSE) file.
