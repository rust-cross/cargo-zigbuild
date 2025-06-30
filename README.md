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

[![Packaging status](https://repology.org/badge/vertical-allrepos/cargo-zigbuild.svg?columns=4)](https://repology.org/project/cargo-zigbuild/versions)

## Usage

1. Install [zig](https://ziglang.org/) following the [official documentation](https://ziglang.org/learn/getting-started/#installing-zig),
on macOS, Windows and Linux you can also install zig from PyPI via `pip3 install ziglang`
2. Install Rust target via rustup, for example, `rustup target add aarch64-unknown-linux-gnu`
3. Run `cargo zigbuild`, for example, `cargo zigbuild --target aarch64-unknown-linux-gnu`

### Specify glibc version

By default `--target` for `*-gnu` will have Zig implicitly build for a default version of glibc that varies based on the release of Zig ([v12 to v14 releases default to glibc 2.28](https://github.com/ziglang/zig/blob/0.14.1/lib/std/Target.zig#L473)).

To build for a specific minimum glibc version, add that version as a suffix to the `--target` value. For example, to compile with `--target aarch64-unknown-linux-gnu` for glibc 2.17:

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

#### Tip - `cargo zigbuild` cannot find headers (`*.h` files) or libraries that exist

You may need to prepend the following ENV to your `cargo zigbuild` command with the following system paths or similar:
- `CFLAGS='-isystem /usr/include'`
- `RUSTFLAGS='-L /usr/lib64'`

---

`cargo zigbuild` always uses the `zig cc` option `-nostdinc` which excludes standard header locations like `/usr/include`. This is also a default behaviour for Zig whenever it is configured with a `--target`, which additionally opts out of standard system search paths.

This can lead to a common difference between `cargo build` being successful, while `cargo zigbuild` fails without extra configuration:

```console
# Cannot find a header file to build:
fatal error: 'libelf.h' file not found

# Cannot find a shared library to link:
error: unable to find dynamic system library 'elf' using strategy 'no_fallback'. searched paths
```

There is a variety of ways to resolve this, but for system paths like `/usr/include` you must be careful to avoid getting the system glibc headers mixed with the glibc headers Zig provides itself, otherwise this will produce errors like from `CPATH=/usr/include`:

```rust
In file included from /usr/local/lib64/python3.13/site-packages/ziglang/lib/libunwind/src/gcc_personality_v0.c:21:
In file included from /usr/local/lib64/python3.13/site-packages/ziglang/lib/libunwind/include/unwind.h:18:
In file included from /usr/include/stdint.h:26:
In file included from /usr/include/bits/libc-header-start.h:33:
/usr/include/features.h:516:9: warning: '__GLIBC_MINOR__' macro redefined [-Wmacro-redefined]
  516 | #define __GLIBC_MINOR__ 41
      |         ^
<command line>:2:9: note: previous definition is here
    2 | #define __GLIBC_MINOR__ 37
      |
```

When you have installed system packages that added headers to `/usr/include` that your project needs to build, you will want Zig to fallback to `/usr/include` just for those headers while using it's own for glibc. This can be done with `zig cc -isystem /usr/include`, which for `cargo zigbuild` can be configured through the common ENV `CFLAGS='-isystem /usr/include'`.

For the similar issue with shared libraries, if your packages are installing system libraries at `/usr/lib64` you would normally use `LDFLAGS='-L /usr/lib64'`, but `rustc` and `cargo` do not read this ENV but they must be configured with the search path for crates with a `build.rs` that searches for a library to link dynamically/statically. Instead you will need to use `RUSTFLAGS='-L /usr/lib64'`.

#### Tip - Verify minimum GLIBC version required

Provided you have no stripped the symbols from your binary built, on Linux you can run the following script to scan for glibc versioned symbols and find the highest version (the minimum required to run)

1. Create a file **`/usr/local/bin/get-min-glibc`:**

   ```bash
   #!/bin/bash
   
   FILE_NAME=$1
   readelf -W --version-info --dyn-syms ${FILE_NAME} \
     | grep 'Name: GLIBC' \
     | sed -re 's/.*GLIBC_(.+) Flags.*/\1/g' \
     | sort -t . -k1,1n -k2,2n \
     | tail -n 1
   ```

2. Make the script command executable:

   ```bash
   chmod +x /usr/local/bin/get-min-glibc
   ```

3. Run the command with the path to your executable / library to check:

   ```console
   $ get-min-glibc target/x86_64-unknown-linux-gnu/release/hello-world
   2.28
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
