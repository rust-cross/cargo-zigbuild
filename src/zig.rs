use std::env;
#[cfg(target_family = "unix")]
use std::fs::OpenOptions;
use std::io::Write;
#[cfg(target_family = "unix")]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::str;

use anyhow::{bail, Context, Result};
use fs_err as fs;
#[cfg(not(target_family = "unix"))]
use path_slash::PathBufExt;
use serde::Deserialize;
use target_lexicon::{Architecture, Environment, OperatingSystem, Triple};

/// Zig linker wrapper
#[derive(Clone, Debug, clap::Subcommand)]
pub enum Zig {
    /// `zig cc` wrapper
    #[clap(name = "cc", trailing_var_arg = true)]
    Cc {
        /// `zig cc` arguments
        #[clap(takes_value = true, multiple_values = true)]
        args: Vec<String>,
    },
    /// `zig c++` wrapper
    #[clap(name = "c++", trailing_var_arg = true)]
    Cxx {
        /// `zig c++` arguments
        #[clap(takes_value = true, multiple_values = true)]
        args: Vec<String>,
    },
}

impl Zig {
    /// Execute the underlying zig command
    pub fn execute(&self) -> Result<()> {
        let (cmd, cmd_args) = match self {
            Zig::Cc { args } => ("cc", args),
            Zig::Cxx { args } => ("c++", args),
        };

        let target = cmd_args
            .iter()
            .position(|x| x == "-target")
            .and_then(|index| cmd_args.get(index + 1));
        let is_musl = target.map(|x| x.contains("musl")).unwrap_or_default();
        let is_windows_gnu = target
            .map(|x| x.contains("windows-gnu"))
            .unwrap_or_default();
        let is_arm = target.map(|x| x.contains("arm")).unwrap_or_default();

        let rustc_ver = rustc_version::version()?;

        let filter_link_arg = |arg: &str| {
            if arg == "-lgcc_s" {
                // Replace libgcc_s with libunwind
                return Some("-lunwind".to_string());
            }
            if is_arm && arg.ends_with(".rlib") && arg.contains("libcompiler_builtins-") {
                // compiler-builtins is duplicated with zig's compiler-rt
                return None;
            }
            if is_windows_gnu {
                if arg == "-lgcc_eh" {
                    // zig doesn't provide gcc_eh alternative
                    // We use libc++ to replace it on windows gnu targets
                    return Some("-lc++".to_string());
                } else if arg == "-lwindows" || arg == "-l:libpthread.a" || arg == "-lgcc" {
                    return None;
                }
            }
            if is_musl {
                // Avoids duplicated symbols with both zig musl libc and the libc crate
                if arg.ends_with(".o") && arg.contains("self-contained") && arg.contains("crt") {
                    return None;
                }
                if rustc_ver.major == 1
                    && rustc_ver.minor < 59
                    && arg.ends_with(".rlib")
                    && arg.contains("liblibc-")
                {
                    // Rust distributes standalone libc.a in self-contained for musl since 1.59.0
                    // See https://github.com/rust-lang/rust/pull/90527
                    return None;
                }
                if arg == "-lc" {
                    return None;
                }
            }
            Some(arg.to_string())
        };
        let has_undefined_dynamic_lookup = |args: &[String]| {
            let undefined = args
                .iter()
                .position(|x| x == "-undefined")
                .and_then(|i| args.get(i + 1));
            matches!(undefined, Some(x) if x == "dynamic_lookup")
        };

        let mut new_cmd_args = Vec::with_capacity(cmd_args.len());
        for arg in cmd_args {
            let arg = if arg.starts_with('@') && arg.ends_with("linker-arguments") {
                // rustc passes arguments to linker via an @-file when arguments are too long
                // See https://github.com/rust-lang/rust/issues/41190
                let content = fs::read(arg.trim_start_matches('@'))?;
                let mut link_args: Vec<_> = str::from_utf8(&content)?
                    .split('\n')
                    .filter_map(filter_link_arg)
                    .collect();
                if has_undefined_dynamic_lookup(&link_args) {
                    link_args.push("-Wl,-undefined=dynamic_lookup".to_string());
                }
                fs::write(arg.trim_start_matches('@'), link_args.join("\n").as_bytes())?;
                Some(arg.to_string())
            } else {
                filter_link_arg(arg)
            };
            if let Some(arg) = arg {
                new_cmd_args.push(arg);
            }
        }
        if has_undefined_dynamic_lookup(cmd_args) {
            new_cmd_args.push("-Wl,-undefined=dynamic_lookup".to_string());
        }
        let mut child = Self::command()?
            .arg(cmd)
            .args(new_cmd_args)
            .spawn()
            .with_context(|| format!("Failed to run `zig {}`", cmd))?;
        let status = child.wait().expect("Failed to wait on zig child process");
        if !status.success() {
            process::exit(status.code().unwrap_or(1));
        }
        Ok(())
    }

    /// Build the zig command line
    pub fn command() -> Result<Command> {
        let (zig, zig_args) = Self::find_zig()?;
        let mut cmd = Command::new(zig);
        cmd.args(zig_args);
        Ok(cmd)
    }

    /// Search for `python -m ziglang` first and for `zig` second.
    pub fn find_zig() -> Result<(String, Vec<String>)> {
        Self::find_zig_python()
            .or_else(|_| Self::find_zig_bin())
            .context("Failed to find zig")
    }

    /// Detect the plain zig binary
    fn find_zig_bin() -> Result<(String, Vec<String>)> {
        let output = Command::new("zig").arg("version").output()?;
        let version_str =
            str::from_utf8(&output.stdout).context("`zig version` didn't return utf8 output")?;
        Self::validate_zig_version(version_str)?;
        Ok(("zig".to_string(), Vec::new()))
    }

    /// Detect the Python ziglang package
    fn find_zig_python() -> Result<(String, Vec<String>)> {
        let output = Command::new("python3")
            .args(&["-m", "ziglang", "version"])
            .output()?;
        let version_str = str::from_utf8(&output.stdout)
            .context("`python3 -m ziglang version` didn't return utf8 output")?;
        Self::validate_zig_version(version_str)?;
        Ok((
            "python3".to_string(),
            vec!["-m".to_string(), "ziglang".to_string()],
        ))
    }

    fn validate_zig_version(version: &str) -> Result<()> {
        let min_ver = semver::Version::new(0, 9, 0);
        let version = semver::Version::parse(version.trim())?;
        if version >= min_ver {
            Ok(())
        } else {
            bail!(
                "zig version {} is too old, need at least {}",
                version,
                min_ver
            )
        }
    }

    /// Find zig lib directory
    pub fn lib_dir() -> Result<PathBuf> {
        let (zig, zig_args) = Self::find_zig()?;
        let output = Command::new(zig).args(zig_args).arg("env").output()?;
        let zig_env: ZigEnv = serde_json::from_slice(&output.stdout)?;
        Ok(PathBuf::from(zig_env.lib_dir))
    }
}

#[derive(Debug, Deserialize)]
struct ZigEnv {
    lib_dir: String,
}

/// Prepare wrapper scripts for `zig cc` and `zig c++` and returns their paths
///
/// We want to use `zig cc` as linker and c compiler. We want to call `python -m ziglang cc`, but
/// cargo only accepts a path to an executable as linker, so we add a wrapper script. We then also
/// use the wrapper script to pass arguments and substitute an unsupported argument.
///
/// We create different files for different args because otherwise cargo might skip recompiling even
/// if the linker target changed
#[allow(clippy::blocks_in_if_conditions)]
pub fn prepare_zig_linker(target: &str) -> Result<(PathBuf, PathBuf)> {
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
        format!(".{}", abi_suffix)
    };
    let triple: Triple = rust_target.parse().unwrap();
    let arch = match triple.architecture {
        // zig target only has i386, no i586/i686
        Architecture::X86_32(..) => "i386".to_string(),
        architecture => architecture.to_string(),
    };
    let target_env = match (triple.architecture, triple.environment) {
        (Architecture::Mips32(..), Environment::Gnu) => Environment::Gnueabihf,
        (_, environment) => environment,
    };
    let file_ext = if cfg!(windows) { "bat" } else { "sh" };
    let zig_cc = format!("zigcc-{}.{}", target, file_ext);
    let zig_cxx = format!("zigcxx-{}.{}", target, file_ext);
    let cc_args = "-g"; // prevent stripping
    let cc_args = match triple.operating_system {
        OperatingSystem::Linux => format!(
            "-target {}-linux-{}{} {}",
            arch, target_env, abi_suffix, cc_args,
        ),
        OperatingSystem::MacOSX { .. } | OperatingSystem::Darwin => {
            format!("-target {}-macos-gnu{} {}", arch, abi_suffix, cc_args)
        }
        OperatingSystem::Windows { .. } => format!(
            "-target {}-windows-{}{} {}",
            arch, target_env, abi_suffix, cc_args,
        ),
        _ => bail!("unsupported target"),
    };

    let zig_linker_dir = dirs::cache_dir()
        // If the really is no cache dir, cwd will also do
        .unwrap_or_else(|| env::current_dir().expect("Failed to get current dir"))
        .join(env!("CARGO_PKG_NAME"))
        .join(env!("CARGO_PKG_VERSION"));
    fs::create_dir_all(&zig_linker_dir)?;

    let zig_cc = zig_linker_dir.join(zig_cc);
    let zig_cxx = zig_linker_dir.join(zig_cxx);
    write_linker_wrapper(&zig_cc, "cc", &cc_args)?;
    write_linker_wrapper(&zig_cxx, "c++", &cc_args)?;

    Ok((zig_cc, zig_cxx))
}

/// Write a zig cc wrapper batch script for unix
#[cfg(target_family = "unix")]
fn write_linker_wrapper(path: &Path, command: &str, args: &str) -> Result<()> {
    let mut custom_linker_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o700)
        .open(path)?;
    let current_exe = if let Ok(exe) = env::var("CARGO_BIN_EXE_cargo-zigbuild") {
        PathBuf::from(exe)
    } else {
        env::current_exe()?
    };
    writeln!(&mut custom_linker_file, "#!/usr/bin/env bash")?;
    writeln!(
        &mut custom_linker_file,
        "{} zig {} -- {} $@",
        current_exe.display(),
        command,
        args
    )?;
    Ok(())
}

/// Write a zig cc wrapper batch script for windows
#[cfg(not(target_family = "unix"))]
fn write_linker_wrapper(path: &Path, command: &str, args: &str) -> Result<()> {
    let mut custom_linker_file = fs::File::create(path)?;
    let current_exe = if let Ok(exe) = env::var("CARGO_BIN_EXE_cargo-zigbuild") {
        PathBuf::from(exe)
    } else {
        env::current_exe()?
    };
    let current_exe = if is_mingw_shell() {
        current_exe.to_slash_lossy()
    } else {
        current_exe.display().to_string()
    };
    writeln!(
        &mut custom_linker_file,
        "{} zig {} -- {} %*",
        current_exe, command, args
    )?;
    Ok(())
}

pub(crate) fn is_mingw_shell() -> bool {
    env::var_os("MSYSTEM").is_some() && env::var_os("SHELL").is_some()
}
