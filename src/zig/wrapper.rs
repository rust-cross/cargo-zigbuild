use std::env;
use std::ffi::OsStr;
#[cfg(target_family = "unix")]
use std::fs::OpenOptions;
use std::io::Write;
#[cfg(target_family = "unix")]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use fs_err as fs;
#[cfg(not(target_family = "unix"))]
use path_slash::PathBufExt;
use target_lexicon::{Architecture, Environment, OperatingSystem, Triple};

use super::locate::cache_dir;
use super::{Zig, get_dlltool_name, has_system_dlltool};

/// zig wrapper paths
#[derive(Debug, Clone)]
pub struct ZigWrapper {
    pub cc: PathBuf,
    pub cxx: PathBuf,
    pub ar: PathBuf,
    pub ranlib: PathBuf,
    pub lib: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct TargetFlags {
    pub target_cpu: String,
    pub target_feature: String,
}

impl TargetFlags {
    pub fn parse_from_encoded(encoded: &OsStr) -> Result<Self> {
        let mut parsed = Self::default();

        let f = rustflags::from_encoded(encoded);
        for flag in f {
            if let rustflags::Flag::Codegen { opt, value } = flag {
                let key = opt.replace('-', "_");
                match key.as_str() {
                    "target_cpu" => {
                        if let Some(value) = value {
                            parsed.target_cpu = value;
                        }
                    }
                    "target_feature" => {
                        // See https://github.com/rust-lang/rust/blob/7e3ba5b8b7556073ab69822cc36b93d6e74cd8c9/compiler/rustc_session/src/options.rs#L1233
                        if let Some(value) = value {
                            if !parsed.target_feature.is_empty() {
                                parsed.target_feature.push(',');
                            }
                            parsed.target_feature.push_str(&value);
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(parsed)
    }
}

/// Prepare wrapper scripts for `zig cc` and `zig c++` and returns their paths
///
/// We want to use `zig cc` as linker and c compiler. We want to call `python -m ziglang cc`, but
/// cargo only accepts a path to an executable as linker, so we add a wrapper script. We then also
/// use the wrapper script to pass arguments and substitute an unsupported argument.
///
/// We create different files for different args because otherwise cargo might skip recompiling even
/// if the linker target changed
#[allow(clippy::blocks_in_conditions)]
pub fn prepare_zig_linker(
    target: &str,
    cargo_config: &cargo_config2::Config,
) -> Result<ZigWrapper> {
    let (rust_target, abi_suffix) = target.split_once('.').unwrap_or((target, ""));
    let abi_suffix = if abi_suffix.is_empty() {
        String::new()
    } else {
        if abi_suffix
            .split_once('.')
            .filter(|(x, y)| {
                !x.is_empty()
                    && x.chars().all(|c| c.is_ascii_digit())
                    && !y.is_empty()
                    && y.chars().all(|c| c.is_ascii_digit())
            })
            .is_none()
        {
            bail!("Malformed zig target abi suffix.")
        }
        format!(".{abi_suffix}")
    };
    let triple: Triple = rust_target
        .parse()
        .with_context(|| format!("Unsupported Rust target '{rust_target}'"))?;
    let arch = triple.architecture.to_string();
    let target_env = match (triple.architecture, triple.environment) {
        (Architecture::Mips32(..), Environment::Gnu) => Environment::Gnueabihf,
        (Architecture::Mips32(..), Environment::Musl) => Environment::Musleabi,
        (Architecture::Powerpc, Environment::Gnu) => Environment::Gnueabihf,
        (_, Environment::GnuLlvm) => Environment::Gnu,
        (_, environment) => environment,
    };
    let file_ext = if cfg!(windows) { "bat" } else { "sh" };
    let file_target = target.trim_end_matches('.');

    let mut cc_args = vec![
        // prevent stripping
        "-g".to_owned(),
        // disable sanitizers
        "-fno-sanitize=all".to_owned(),
    ];

    // TODO: Maybe better to assign mcpu according to:
    // rustc --target <target> -Z unstable-options --print target-spec-json
    let zig_mcpu_default = match triple.operating_system {
        OperatingSystem::Linux => {
            match arch.as_str() {
                // zig uses _ instead of - in cpu features
                "arm" => match target_env {
                    Environment::Gnueabi | Environment::Musleabi => "generic+v6+strict_align",
                    Environment::Gnueabihf | Environment::Musleabihf => {
                        "generic+v6+strict_align+vfp2-d32"
                    }
                    _ => "",
                },
                "armv5te" => "generic+soft_float+strict_align",
                "armv7" => "generic+v7a+vfp3-d32+thumb2-neon",
                arch_str @ ("i586" | "i686") => {
                    if arch_str == "i586" {
                        "pentium"
                    } else {
                        "pentium4"
                    }
                }
                "riscv64gc" => "generic_rv64+m+a+f+d+c",
                "s390x" => "z10-vector",
                _ => "",
            }
        }
        _ => "",
    };

    // Override mcpu from RUSTFLAGS if provided. The override happens when
    // commands like `cargo-zigbuild build` are invoked.
    // Currently we only override according to target_cpu.
    let zig_mcpu_override = {
        let rust_flags = cargo_config.rustflags(rust_target)?.unwrap_or_default();
        let encoded_rust_flags = rust_flags.encode()?;
        let target_flags = TargetFlags::parse_from_encoded(OsStr::new(&encoded_rust_flags))?;
        // Note: zig uses _ instead of - for target_cpu and target_feature
        // target_cpu may be empty string, which means target_cpu is not specified.
        target_flags.target_cpu.replace('-', "_")
    };

    if !zig_mcpu_override.is_empty() {
        cc_args.push(format!("-mcpu={zig_mcpu_override}"));
    } else if !zig_mcpu_default.is_empty() {
        cc_args.push(format!("-mcpu={zig_mcpu_default}"));
    }

    match triple.operating_system {
        OperatingSystem::Linux => {
            let zig_arch = match arch.as_str() {
                // zig uses _ instead of - in cpu features
                "arm" => "arm",
                "armv5te" => "arm",
                "armv7" => "arm",
                "i586" | "i686" => {
                    let zig_version = Zig::zig_version()?;
                    if zig_version.major == 0 && zig_version.minor >= 11 {
                        "x86"
                    } else {
                        "i386"
                    }
                }
                "riscv64gc" => "riscv64",
                "s390x" => "s390x",
                _ => arch.as_str(),
            };
            let mut zig_target_env = target_env.to_string();

            let zig_version = Zig::zig_version()?;

            // Since Zig 0.15.0, arm-linux-ohos changed to arm-linux-ohoseabi
            // We need to follow the change but target_lexicon follow the LLVM target(https://github.com/bytecodealliance/target-lexicon/pull/123).
            // So we use string directly.
            if zig_version >= semver::Version::new(0, 15, 0)
                && arch.as_str() == "armv7"
                && target_env == Environment::Ohos
            {
                zig_target_env = "ohoseabi".to_string();
            }

            cc_args.push("-target".to_string());
            cc_args.push(format!("{zig_arch}-linux-{zig_target_env}{abi_suffix}"));
        }
        OperatingSystem::MacOSX { .. } | OperatingSystem::Darwin(_) => {
            let zig_version = Zig::zig_version()?;
            // Zig 0.10.0 switched macOS ABI to none
            // see https://github.com/ziglang/zig/pull/11684
            if zig_version > semver::Version::new(0, 9, 1) {
                cc_args.push("-target".to_string());
                cc_args.push(format!("{arch}-macos-none{abi_suffix}"));
            } else {
                cc_args.push("-target".to_string());
                cc_args.push(format!("{arch}-macos-gnu{abi_suffix}"));
            }
        }
        OperatingSystem::Windows => {
            let zig_arch = match arch.as_str() {
                "i686" => {
                    let zig_version = Zig::zig_version()?;
                    if zig_version.major == 0 && zig_version.minor >= 11 {
                        "x86"
                    } else {
                        "i386"
                    }
                }
                arch => arch,
            };
            cc_args.push("-target".to_string());
            cc_args.push(format!("{zig_arch}-windows-{target_env}{abi_suffix}"));
        }
        OperatingSystem::Emscripten => {
            cc_args.push("-target".to_string());
            cc_args.push(format!("{arch}-emscripten{abi_suffix}"));
        }
        OperatingSystem::Wasi => {
            cc_args.push("-target".to_string());
            cc_args.push(format!("{arch}-wasi{abi_suffix}"));
        }
        OperatingSystem::WasiP1 => {
            cc_args.push("-target".to_string());
            cc_args.push(format!("{arch}-wasi.0.1.0{abi_suffix}"));
        }
        OperatingSystem::IOS(_) if triple.environment == Environment::Macabi => {
            // Mac Catalyst (aarch64-apple-ios-macabi / x86_64-apple-ios-macabi)
            // maps to zig's maccatalyst target
            cc_args.push("-target".to_string());
            cc_args.push(format!("{arch}-maccatalyst-none{abi_suffix}"));
        }
        OperatingSystem::Freebsd => {
            let zig_arch = match arch.as_str() {
                "i686" => {
                    let zig_version = Zig::zig_version()?;
                    if zig_version.major == 0 && zig_version.minor >= 11 {
                        "x86"
                    } else {
                        "i386"
                    }
                }
                arch => arch,
            };
            cc_args.push("-target".to_string());
            cc_args.push(format!("{zig_arch}-freebsd"));
        }
        OperatingSystem::Openbsd => {
            cc_args.push("-target".to_string());
            cc_args.push(format!("{arch}-openbsd"));
        }
        OperatingSystem::Unknown => {
            if triple.architecture == Architecture::Wasm32
                || triple.architecture == Architecture::Wasm64
            {
                cc_args.push("-target".to_string());
                cc_args.push(format!("{arch}-freestanding{abi_suffix}"));
            } else {
                bail!("unsupported target '{rust_target}'")
            }
        }
        _ => bail!(format!("unsupported target '{rust_target}'")),
    };

    let zig_linker_dir = cache_dir();
    fs::create_dir_all(&zig_linker_dir)?;

    if triple.operating_system == OperatingSystem::Linux {
        if matches!(
            triple.environment,
            Environment::Gnu
                | Environment::Gnuspe
                | Environment::Gnux32
                | Environment::Gnueabi
                | Environment::Gnuabi64
                | Environment::GnuIlp32
                | Environment::Gnueabihf
        ) {
            let glibc_version = if abi_suffix.is_empty() {
                (2, 17)
            } else {
                let mut parts = abi_suffix[1..].split('.');
                let major: usize = parts.next().unwrap().parse()?;
                let minor: usize = parts.next().unwrap().parse()?;
                (major, minor)
            };
            // See https://github.com/ziglang/zig/issues/9485
            if glibc_version < (2, 28) {
                use crate::linux::{FCNTL_H, FCNTL_MAP};

                let zig_version = Zig::zig_version()?;
                if zig_version.major == 0 && zig_version.minor < 11 {
                    let fcntl_map = zig_linker_dir.join("fcntl.map");
                    let existing_content = fs::read_to_string(&fcntl_map).unwrap_or_default();
                    if existing_content != FCNTL_MAP {
                        fs::write(&fcntl_map, FCNTL_MAP)?;
                    }
                    let fcntl_h = zig_linker_dir.join("fcntl.h");
                    let existing_content = fs::read_to_string(&fcntl_h).unwrap_or_default();
                    if existing_content != FCNTL_H {
                        fs::write(&fcntl_h, FCNTL_H)?;
                    }

                    cc_args.push(format!("-Wl,--version-script={}", fcntl_map.display()));
                    cc_args.push("-include".to_string());
                    cc_args.push(fcntl_h.display().to_string());
                }
            }
        } else if matches!(
            triple.environment,
            Environment::Musl
                | Environment::Muslabi64
                | Environment::Musleabi
                | Environment::Musleabihf
        ) {
            use crate::linux::MUSL_WEAK_SYMBOLS_MAPPING_SCRIPT;

            let zig_version = Zig::zig_version()?;
            let rustc_version = rustc_version::version_meta()?.semver;

            // as zig 0.11.0 is released, its musl has been upgraded to 1.2.4 with break changes
            // but rust is still with musl 1.2.3
            // we need this workaround before rust 1.72
            // https://github.com/ziglang/zig/pull/16098
            if (zig_version.major, zig_version.minor) >= (0, 11)
                && (rustc_version.major, rustc_version.minor) < (1, 72)
            {
                let weak_symbols_map = zig_linker_dir.join("musl_weak_symbols_map.ld");
                fs::write(&weak_symbols_map, MUSL_WEAK_SYMBOLS_MAPPING_SCRIPT)?;

                cc_args.push(format!("-Wl,-T,{}", weak_symbols_map.display()));
            }
        }
    }

    // Use platform-specific quoting: shell_words for Unix (single quotes),
    // custom quoting for Windows batch files (double quotes)
    let cc_args_str = join_args_for_script(&cc_args);

    // Put all generated wrappers and symlinks in a per-exe subdirectory so
    // that parallel builds driven by different binaries (e.g. multiple maturin
    // instances in separate temp venvs) never clobber each other.
    // See https://github.com/rust-cross/cargo-zigbuild/issues/318
    let current_exe = resolve_current_exe()?;
    let exe_hash = crc::Crc::<u16>::new(&crc::CRC_16_IBM_SDLC)
        .checksum(current_exe.as_os_str().as_encoded_bytes());
    let wrapper_dir = zig_linker_dir
        .join("wrappers")
        .join(format!("{:x}", exe_hash));
    fs::create_dir_all(&wrapper_dir)?;

    let hash = crc::Crc::<u16>::new(&crc::CRC_16_IBM_SDLC).checksum(cc_args_str.as_bytes());
    let zig_cc = wrapper_dir.join(format!("zigcc-{file_target}-{:x}.{file_ext}", hash));
    let zig_cxx = wrapper_dir.join(format!("zigcxx-{file_target}-{:x}.{file_ext}", hash));
    let zig_ranlib = wrapper_dir.join(format!("zigranlib.{file_ext}"));
    let zig_version = Zig::zig_version()?;
    let zig_command = Zig::find_zig()?;
    write_linker_wrapper(&zig_cc, "cc", &cc_args_str, &zig_version, &zig_command)?;
    write_linker_wrapper(&zig_cxx, "c++", &cc_args_str, &zig_version, &zig_command)?;
    write_linker_wrapper(&zig_ranlib, "ranlib", "", &zig_version, &zig_command)?;

    let exe_ext = if cfg!(windows) { ".exe" } else { "" };
    let zig_ar = wrapper_dir.join(format!("ar{exe_ext}"));
    symlink_wrapper(&zig_ar)?;
    let zig_lib = wrapper_dir.join(format!("lib{exe_ext}"));
    symlink_wrapper(&zig_lib)?;

    // Create dlltool symlinks for Windows GNU targets, but only if no system dlltool exists
    // On Windows hosts, rustc looks for "dlltool.exe"
    // On non-Windows hosts, rustc looks for architecture-specific names
    //
    // See https://github.com/rust-lang/rust/blob/a18e6d9d1473d9b25581dd04bef6c7577999631c/compiler/rustc_codegen_ssa/src/back/archive.rs#L275-L309
    if matches!(triple.operating_system, OperatingSystem::Windows)
        && matches!(triple.environment, Environment::Gnu)
    {
        // Only create zig dlltool wrapper if no system dlltool is found
        // System dlltool (from mingw-w64) handles raw-dylib better than zig's dlltool
        if !has_system_dlltool(&triple.architecture) {
            let dlltool_name = get_dlltool_name(&triple.architecture);
            let zig_dlltool = wrapper_dir.join(format!("{dlltool_name}{exe_ext}"));
            symlink_wrapper(&zig_dlltool)?;
        }
    }

    Ok(ZigWrapper {
        cc: zig_cc,
        cxx: zig_cxx,
        ar: zig_ar,
        ranlib: zig_ranlib,
        lib: zig_lib,
    })
}

/// Resolve the current executable path, preferring the test override env var.
fn resolve_current_exe() -> Result<PathBuf> {
    if let Ok(exe) = env::var("CARGO_BIN_EXE_cargo-zigbuild") {
        Ok(PathBuf::from(exe))
    } else {
        Ok(env::current_exe()?)
    }
}

pub(crate) fn symlink_wrapper(target: &Path) -> Result<()> {
    let current_exe = resolve_current_exe()?;
    #[cfg(windows)]
    {
        if !target.exists() {
            // symlink on Windows requires admin privileges so we use hardlink instead
            if std::fs::hard_link(&current_exe, target).is_err() {
                // hard_link doesn't support cross-device links so we fallback to copy
                std::fs::copy(&current_exe, target)?;
            }
        }
    }

    #[cfg(unix)]
    {
        if !target.exists() {
            if fs::read_link(target).is_ok() {
                // remove broken symlink
                fs::remove_file(target)?;
            }
            std::os::unix::fs::symlink(current_exe, target)?;
        }
    }
    Ok(())
}

/// Join arguments for Unix shell script using shell_words (single quotes)
#[cfg(target_family = "unix")]
fn join_args_for_script<I, S>(args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    shell_words::join(args)
}

/// Quote a string for Windows batch file (cmd.exe)
///
/// - `%` expands even inside quotes, so we escape it as `%%`.
/// - We disable delayed expansion in the wrapper script, so `!` should not expand.
/// - Internal `"` are escaped by doubling them (`""`).
#[cfg(not(target_family = "unix"))]
fn quote_for_batch(s: &str) -> String {
    let needs_quoting_or_escaping = s.is_empty()
        || s.contains(|c: char| {
            matches!(
                c,
                ' ' | '\t' | '"' | '&' | '|' | '<' | '>' | '^' | '%' | '(' | ')' | '!'
            )
        });

    if !needs_quoting_or_escaping {
        return s.to_string();
    }

    let mut out = String::with_capacity(s.len() + 8);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\"\""),
            '%' => out.push_str("%%"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Join arguments for Windows batch file using double quotes
#[cfg(not(target_family = "unix"))]
fn join_args_for_script<I, S>(args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter()
        .map(|s| quote_for_batch(s.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Write a zig cc wrapper batch script for unix
#[cfg(target_family = "unix")]
fn write_linker_wrapper(
    path: &Path,
    command: &str,
    args: &str,
    zig_version: &semver::Version,
    zig_command: &(PathBuf, Vec<String>),
) -> Result<()> {
    let mut buf = Vec::<u8>::new();
    let current_exe = resolve_current_exe()?;
    writeln!(&mut buf, "#!/bin/sh")?;

    // Export zig version to avoid spawning `zig version` subprocess
    writeln!(
        &mut buf,
        "export CARGO_ZIGBUILD_ZIG_VERSION={}",
        zig_version
    )?;
    // Export the resolved zig command to avoid re-probing for
    // `python -m ziglang` / `zig` on every compiler invocation
    writeln!(
        &mut buf,
        "export CARGO_ZIGBUILD_ZIG_COMMAND={}",
        shell_words::quote(&zig_command.0.to_string_lossy())
    )?;
    if !zig_command.1.is_empty() {
        writeln!(
            &mut buf,
            "export CARGO_ZIGBUILD_ZIG_COMMAND_ARGS={}",
            shell_words::quote(&zig_command.1.join(" "))
        )?;
    }

    // Pass through SDKROOT if it exists at runtime
    writeln!(&mut buf, "if [ -n \"$SDKROOT\" ]; then export SDKROOT; fi")?;

    writeln!(
        &mut buf,
        "exec \"{}\" zig {} -- {} \"$@\"",
        current_exe.display(),
        command,
        args
    )?;

    // Try not to write the file again if it's already the same.
    // This is more friendly for cache systems like ccache, which by default
    // uses mtime to determine if a recompilation is needed.
    let existing_content = fs::read(path).unwrap_or_default();
    if existing_content != buf {
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o700)
            .open(path)?
            .write_all(&buf)?;
    }
    Ok(())
}

/// Write a zig cc wrapper batch script for windows
#[cfg(not(target_family = "unix"))]
fn write_linker_wrapper(
    path: &Path,
    command: &str,
    args: &str,
    zig_version: &semver::Version,
    zig_command: &(PathBuf, Vec<String>),
) -> Result<()> {
    let mut buf = Vec::<u8>::new();
    let current_exe = resolve_current_exe()?;
    let current_exe = if is_mingw_shell() {
        current_exe.to_slash_lossy().to_string()
    } else {
        current_exe.display().to_string()
    };
    writeln!(&mut buf, "@echo off")?;
    // Prevent `!VAR!` expansion surprises (delayed expansion) in user-controlled args.
    writeln!(&mut buf, "setlocal DisableDelayedExpansion")?;
    // Set zig version to avoid spawning `zig version` subprocess
    writeln!(&mut buf, "set CARGO_ZIGBUILD_ZIG_VERSION={}", zig_version)?;
    // Set the resolved zig command to avoid re-probing for
    // `python -m ziglang` / `zig` on every compiler invocation
    writeln!(
        &mut buf,
        "set \"CARGO_ZIGBUILD_ZIG_COMMAND={}\"",
        zig_command.0.display()
    )?;
    if !zig_command.1.is_empty() {
        writeln!(
            &mut buf,
            "set \"CARGO_ZIGBUILD_ZIG_COMMAND_ARGS={}\"",
            zig_command.1.join(" ")
        )?;
    }
    writeln!(
        &mut buf,
        "\"{}\" zig {} -- {} %*",
        adjust_canonicalization(current_exe),
        command,
        args
    )?;

    let existing_content = fs::read(path).unwrap_or_default();
    if existing_content != buf {
        fs::write(path, buf)?;
    }
    Ok(())
}

pub(crate) fn is_mingw_shell() -> bool {
    env::var_os("MSYSTEM").is_some() && env::var_os("SHELL").is_some()
}

// https://stackoverflow.com/a/50323079/3549270
#[cfg(target_os = "windows")]
pub fn adjust_canonicalization(p: String) -> String {
    const VERBATIM_PREFIX: &str = r#"\\?\"#;
    if p.starts_with(VERBATIM_PREFIX) {
        p[VERBATIM_PREFIX.len()..].to_string()
    } else {
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_target_flags() {
        let cases = [
            // Input, TargetCPU, TargetFeature
            ("-C target-feature=-crt-static", "", "-crt-static"),
            ("-C target-cpu=native", "native", ""),
            (
                "--deny warnings --codegen target-feature=+crt-static",
                "",
                "+crt-static",
            ),
            ("-C target_cpu=skylake-avx512", "skylake-avx512", ""),
            ("-Ctarget_cpu=x86-64-v3", "x86-64-v3", ""),
            (
                "-C target-cpu=native --cfg foo -C target-feature=-avx512bf16,-avx512bitalg",
                "native",
                "-avx512bf16,-avx512bitalg",
            ),
            (
                "--target x86_64-unknown-linux-gnu --codegen=target-cpu=x --codegen=target-cpu=x86-64",
                "x86-64",
                "",
            ),
            (
                "-Ctarget-feature=+crt-static -Ctarget-feature=+avx",
                "",
                "+crt-static,+avx",
            ),
        ];

        for (input, expected_target_cpu, expected_target_feature) in cases.iter() {
            let args = cargo_config2::Flags::from_space_separated(input);
            let encoded_rust_flags = args.encode().unwrap();
            let flags = TargetFlags::parse_from_encoded(OsStr::new(&encoded_rust_flags)).unwrap();
            assert_eq!(flags.target_cpu, *expected_target_cpu, "{}", input);
            assert_eq!(flags.target_feature, *expected_target_feature, "{}", input);
        }
    }

    #[test]
    fn test_join_args_for_script() {
        // Test basic arguments without special characters
        let args = vec!["-target", "x86_64-linux-gnu"];
        let result = join_args_for_script(&args);
        assert!(result.contains("-target"));
        assert!(result.contains("x86_64-linux-gnu"));
    }

    #[test]
    #[cfg(not(target_family = "unix"))]
    fn test_quote_for_batch() {
        // Simple argument without special characters - no quoting needed
        assert_eq!(quote_for_batch("-target"), "-target");
        assert_eq!(quote_for_batch("x86_64-linux-gnu"), "x86_64-linux-gnu");

        // Arguments with spaces need quoting
        assert_eq!(
            quote_for_batch("C:\\Users\\John Doe\\path"),
            "\"C:\\Users\\John Doe\\path\""
        );

        // Empty string needs quoting
        assert_eq!(quote_for_batch(""), "\"\"");

        // Arguments with special batch characters need quoting
        assert_eq!(quote_for_batch("foo&bar"), "\"foo&bar\"");
        assert_eq!(quote_for_batch("foo|bar"), "\"foo|bar\"");
        assert_eq!(quote_for_batch("foo<bar"), "\"foo<bar\"");
        assert_eq!(quote_for_batch("foo>bar"), "\"foo>bar\"");
        assert_eq!(quote_for_batch("foo^bar"), "\"foo^bar\"");
        assert_eq!(quote_for_batch("foo%bar"), "\"foo%bar\"");

        // Internal double quotes are escaped by doubling
        assert_eq!(quote_for_batch("foo\"bar"), "\"foo\"\"bar\"");
    }

    #[test]
    #[cfg(not(target_family = "unix"))]
    fn test_join_args_for_script_windows() {
        // Test with path containing spaces
        let args = vec![
            "-target",
            "x86_64-linux-gnu",
            "-L",
            "C:\\Users\\John Doe\\path",
        ];
        let result = join_args_for_script(&args);
        // The path with space should be quoted
        assert!(result.contains("\"C:\\Users\\John Doe\\path\""));
        // Simple args should not be quoted
        assert!(result.contains("-target"));
        assert!(!result.contains("\"-target\""));
    }
}
