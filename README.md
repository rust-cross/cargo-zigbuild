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
cargo install --locked cargo-zigbuild
```

You can also install it using pip which will also install [`ziglang`](https://pypi.org/project/ziglang/) automatically:

```bash
pip install cargo-zigbuild
```

We also provide Docker images which has macOS SDK pre-installed in addition to cargo-zigbuild and Rust, for example to build for x86_64 macOS:

- Linux docker image ([ghcr.io](https://github.com/rust-cross/cargo-zigbuild/pkgs/container/cargo-zigbuild), [Docker Hub](https://hub.docker.com/r/messense/cargo-zigbuild)):
```bash
docker run --rm -it -v $(pwd):/io -w /io ghcr.io/rust-cross/cargo-zigbuild \
  cargo zigbuild --release --target x86_64-apple-darwin
```

- Windows docker image ([ghcr.io](https://github.com/rust-cross/cargo-zigbuild/pkgs/container/cargo-zigbuild.windows), [Docker Hub](https://hub.docker.com/r/messense/cargo-zigbuild.windows)):
```powershell
docker run --rm -it -v ${pwd}:c:\io -w c:\io ghcr.io/rust-cross/cargo-zigbuild.windows `
  cargo zigbuild --target x86_64-apple-darwin
```
> [!NOTE]  
> Windows docker image can compile debug builds, but does NOT support `cargo build --release` for *-apple-darwin targets.
> You will get ```error: unable to run `strip`: program not found```. If you know a solution to this, please open an issue and/or PR.

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

> [!NOTE]
> There are [various caveats](https://github.com/rust-cross/cargo-zigbuild/issues/231#issuecomment-1983434802) with the glibc version targeting feature:
> - If you do not provide a `--target`, Zig is not used and the command effectively runs a regular `cargo build`.
> - If you specify an invalid glibc version, `cargo zigbuild` will not relay the warning emitted from `zig cc` about the fallback version selected.
> - This feature does not necessarily match the behaviour of dynamically linking to a specific version of glibc on the build host.
>   - Version 2.32 can be specified, but runs on a host with only 2.31 available when it should instead abort with an error.
>   - Meanwhile specifying 2.33 will correctly be detected as incompatible when run on a host with glibc 2.31.
> - Certain `RUSTFLAGS` like `-C linker` opt-out of using Zig, while `-L path/to/files` will have Zig ignore `-C target-feature=+crt-static`.
> - `-C target-feature=+crt-static` for statically linking to a glibc version is **not supported** (_upstream `zig cc` lacks support_)

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

1. Currently only Linux and macOS targets are supported,
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
