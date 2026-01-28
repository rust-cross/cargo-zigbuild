# Copilot Agent Instructions for cargo-zigbuild

## Project Overview

cargo-zigbuild is a Rust tool that enables easier cross-compilation of Cargo projects by using [zig](https://ziglang.org/) as the linker. It wraps Cargo commands and automatically configures the zig compiler toolchain for cross-platform builds, particularly targeting Linux, macOS, and Windows platforms with various architectures.

The project is available as both a Rust crate (`cargo install cargo-zigbuild`) and a Python package (`pip install cargo-zigbuild`) that bundles the tool with zig.

## Repository Structure

- `/src` - Main Rust source code
  - `bin/` - Binary entry points
  - `lib.rs` - Core library functionality
  - `zig.rs` - Zig compiler integration
  - `linux/` - Linux-specific cross-compilation logic
  - `macos/` - macOS-specific cross-compilation logic
  - `build.rs`, `check.rs`, `clippy.rs`, `doc.rs`, `install.rs`, `run.rs`, `rustc.rs`, `test.rs` - Command implementations
- `/tests` - Integration test projects (used for testing cross-compilation scenarios)
- `/.github/workflows` - GitHub Actions CI/CD workflows
  - `CI.yml` - Main CI pipeline testing multiple platforms, Rust versions, and zig versions
  - `Release.yml` - Release automation
- `Cargo.toml` - Rust package manifest
- `pyproject.toml` - Python package configuration
- `Dockerfile` - Docker image with cargo-zigbuild, Rust, and macOS SDK

## Tech Stack and Coding Standards

### Language and Tools
- **Language:** Rust (MSRV: 1.85.0)
- **Supported Rust versions:** Stable and nightly
- **Supported zig versions:** 0.11.0, 0.15.2, and master
- **Edition:** 2021

### Key Dependencies
- `clap` - Command-line argument parsing
- `cargo_metadata` - Cargo workspace introspection
- `cargo-options` - Cargo command options handling
- `target-lexicon` - Target triple parsing
- `rustc_version` - Rust compiler version detection
- `fat-macho` - macOS universal binary support (optional, for `universal2` feature)

### Coding Practices
- Follow Rust standard formatting: Use `cargo fmt --all -- --check` to verify formatting
- Run clippy for linting: `cargo clippy --all-features`
- The project uses standard Rust idioms and error handling with the `anyhow` crate
- Use `fs-err` instead of `std::fs` for better error messages

## Build, Test, and Validation

### Building
```bash
cargo build              # Debug build
cargo build --release    # Release build
```

### Testing
The test suite runs cross-compilation scenarios for various targets. Tests are in the `/tests` directory and use real-world projects to validate functionality.

```bash
cargo test                                    # Unit tests
cargo run zigbuild --target <target>          # Integration test (builds for specific target)
```

**Important test requirements:**
- Install zig before running integration tests
- Install target toolchains: `rustup target add <target>`
- Some tests require LLVM/Clang installed
- macOS cross-compilation tests may require `SDKROOT` environment variable
- Linux cross-compilation tests may use qemu for runtime testing

### Linting
```bash
cargo fmt --all -- --check    # Check formatting
cargo clippy --all-features   # Run clippy lints
```

### Code Checking
```bash
cargo check --all
```

## Contribution Guidelines

### Pull Request Requirements
- All code must pass `cargo fmt --all -- --check`
- All code must pass `cargo clippy --all-features`
- CI must pass (tests on Ubuntu, macOS, and Windows with multiple Rust and zig versions)
- Update documentation in README.md if adding new features or changing behavior

### Testing Strategy
- The CI tests on multiple platforms (Ubuntu, macOS 15, Windows)
- Tests run against Rust 1.85.0 (MSRV), stable, and nightly
- Tests run against zig 0.11.0, 0.15.2, and master
- Integration tests compile real projects for various targets:
  - Linux: `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-gnu`, `i686-unknown-linux-gnu`, `armv7-unknown-linux-gnueabihf`, `arm-unknown-linux-gnueabihf`
  - macOS: `aarch64-apple-darwin`, `x86_64-apple-darwin`, `universal2-apple-darwin`
  - Windows: `x86_64-pc-windows-gnu`, `i686-pc-windows-gnu`
  - musl: `aarch64-unknown-linux-musl`

### Known Test Characteristics
- Windows builds with zig 0.13+ are currently problematic
- Some tests are allowed to fail (`continue-on-error`) for master zig, nightly Rust, and Windows
- macOS SDK tests only run with zig 0.11.0 due to framework linking issues in zig 0.14+

## Project-Specific Context

### Key Features
1. **Cross-compilation made easy:** Simplifies building Rust projects for different architectures and platforms
2. **glibc version targeting:** Supports specifying minimum glibc version (e.g., `--target aarch64-unknown-linux-gnu.2.17`)
3. **macOS universal2 binaries:** Special `universal2-apple-darwin` target for creating universal macOS binaries
4. **Environment variable configuration:** Supports various environment variables for customization (see README.md)

### Important Technical Details
- The tool wraps zig's C/C++ compiler (`zig cc`) to act as the linker for rustc
- Uses `-nostdinc` flag with zig cc, which requires careful handling of system headers
- Sets up compiler wrappers and environment variables for cargo build to use zig
- Handles platform-specific SDK paths and framework linking (especially for macOS)
- Creates cache directories for zig tools and wrappers

### Caveats and Limitations
1. Currently only Linux and macOS targets are fully supported
2. Only current stable and nightly Rust versions are regularly tested
3. zig 0.15+ requires clang 18+ when using bindgen
4. Some upstream zig cc issues affect certain target configurations (documented in README.md)
5. Cannot use `--message-format` option with `universal2` target

### Common Pitfalls
- Missing headers/libraries: May need `CFLAGS='-isystem /usr/include'` and `RUSTFLAGS='-L /usr/lib64'`
- Framework linking on macOS cross-compilation requires `SDKROOT` environment variable
- System glibc headers can conflict with zig-provided headers if not using `-isystem` correctly

## Principles

- **Minimal dependencies:** Keep dependencies focused on what's needed for cross-compilation
- **Compatibility:** Maintain compatibility with multiple zig versions and Rust toolchains
- **User experience:** Make cross-compilation "just work" with minimal configuration
- **Documentation:** Keep README.md up-to-date with all features, caveats, and workarounds

## Development Workflow

1. Make changes to Rust source code
2. Format code: `cargo fmt --all`
3. Check for issues: `cargo clippy --all-features`
4. Build: `cargo build`
5. Test manually with a target: `cargo run zigbuild --target <target>`
6. Ensure CI will pass by testing locally when possible
7. Update README.md if changing user-facing behavior or adding features
