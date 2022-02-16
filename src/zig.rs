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
use target_lexicon::{OperatingSystem, Triple};

/// Zig linker wrapper
#[derive(Debug, clap::Parser)]
pub enum Zig {
    /// `zig cc` wrapper
    #[clap(name = "cc", setting = clap::AppSettings::TrailingVarArg)]
    Cc {
        /// `zig cc` arguments
        #[clap(takes_value = true, multiple_values = true)]
        args: Vec<String>,
    },
    /// `zig c++` wrapper
    #[clap(name = "c++", setting = clap::AppSettings::TrailingVarArg)]
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
        // Replace libgcc_s with libunwind
        let cmd_args: Vec<String> = cmd_args
            .iter()
            .map(|arg| {
                let arg = if arg == "-lgcc_s" {
                    "-lunwind".to_string()
                } else if arg.starts_with('@') && arg.ends_with("linker-arguments") {
                    // rustc passes arguments to linker via an @-file when arguments are too long
                    // See https://github.com/rust-lang/rust/issues/41190
                    let content = fs::read(arg.trim_start_matches('@'))?;
                    let link_args = str::from_utf8(&content)?.replace("-lgcc_s", "-lunwind");
                    fs::write(arg.trim_start_matches('@'), link_args.as_bytes())?;
                    arg.to_string()
                } else {
                    arg.to_string()
                };
                Ok(arg)
            })
            .collect::<Result<_>>()?;
        let (zig, zig_args) = Self::find_zig()?;
        let mut child = Command::new(zig)
            .args(zig_args)
            .arg(cmd)
            .args(cmd_args)
            .spawn()
            .with_context(|| format!("Failed to run `zig {}`", cmd))?;
        let status = child.wait().expect("Failed to wait on zig child process");
        if !status.success() {
            process::exit(status.code().unwrap_or(1));
        }
        Ok(())
    }

    /// Search for `python -m ziglang` first and for `zig` second.
    /// That way we use the zig from `maturin[ziglang]` first,
    /// but users or distributions can also insert their own zig
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
}

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
    let arch = triple.architecture.to_string();
    let file_ext = if cfg!(windows) { "bat" } else { "sh" };
    let zig_cc = format!("zigcc-{}.{}", target, file_ext);
    let zig_cxx = format!("zigcxx-{}.{}", target, file_ext);
    let cc_args = match triple.operating_system {
        OperatingSystem::Linux => format!(
            "-target {}-linux-{}{}",
            arch, triple.environment, abi_suffix
        ),
        OperatingSystem::MacOSX { .. } | OperatingSystem::Darwin => {
            format!("-target {}-macos-gnu{}", arch, abi_suffix)
        }
        OperatingSystem::Windows { .. } => format!("-target {}-windows-gnu{}", arch, abi_suffix),
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

#[cfg(target_family = "unix")]
fn write_linker_wrapper(path: &Path, command: &str, args: &str) -> Result<()> {
    let mut custom_linker_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o700)
        .open(path)?;
    let current_exe = if let Ok(maturin) = env::var("CARGO_BIN_EXE_maturin") {
        PathBuf::from(maturin)
    } else {
        env::current_exe()?
    };
    writeln!(&mut custom_linker_file, "#!/bin/bash")?;
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
    let current_exe = if let Ok(exe) = env::var("CARGO_BIN_EXE_cargo_zigbuild") {
        PathBuf::from(exe)
    } else {
        env::current_exe()?
    };
    writeln!(
        &mut custom_linker_file,
        "{} zig {} -- {} %*",
        current_exe.display(),
        command,
        args
    )?;
    Ok(())
}
