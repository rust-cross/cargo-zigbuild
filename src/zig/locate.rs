use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::str;
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::Zig;

impl Zig {
    /// Build the zig command line
    pub fn command() -> Result<Command> {
        let (zig, zig_args) = Self::find_zig()?;
        let mut cmd = Command::new(zig);
        cmd.args(zig_args);
        Ok(cmd)
    }

    pub(crate) fn zig_version() -> Result<semver::Version> {
        static ZIG_VERSION: OnceLock<semver::Version> = OnceLock::new();

        if let Some(version) = ZIG_VERSION.get() {
            return Ok(version.clone());
        }
        // Check for cached version from environment variable first
        if let Ok(version_str) = env::var("CARGO_ZIGBUILD_ZIG_VERSION")
            && let Ok(version) = semver::Version::parse(&version_str)
        {
            return Ok(ZIG_VERSION.get_or_init(|| version).clone());
        }
        let output = Self::command()?.arg("version").output()?;
        let version_str =
            str::from_utf8(&output.stdout).context("`zig version` didn't return utf8 output")?;
        let version = semver::Version::parse(version_str.trim())?;
        Ok(ZIG_VERSION.get_or_init(|| version).clone())
    }

    /// Search for `python -m ziglang` first and for `zig` second.
    pub fn find_zig() -> Result<(PathBuf, Vec<String>)> {
        static ZIG_PATH: OnceLock<(PathBuf, Vec<String>)> = OnceLock::new();

        if let Some(cached) = ZIG_PATH.get() {
            return Ok(cached.clone());
        }
        // Trust the zig command resolved when the linker wrapper was generated;
        // this avoids spawning `python -m ziglang version` and `zig version`
        // probes on every compiler invocation.
        if let Ok(path) = env::var("CARGO_ZIGBUILD_ZIG_COMMAND")
            && !path.is_empty()
        {
            let path = PathBuf::from(path);
            if path.exists() {
                let args = env::var("CARGO_ZIGBUILD_ZIG_COMMAND_ARGS")
                    .map(|s| s.split_whitespace().map(ToString::to_string).collect())
                    .unwrap_or_default();
                return Ok(ZIG_PATH.get_or_init(|| (path, args)).clone());
            }
        }
        let result = Self::find_zig_python()
            .or_else(|_| Self::find_zig_bin())
            .context("Failed to find zig")?;
        Ok(ZIG_PATH.get_or_init(|| result).clone())
    }

    /// Detect the plain zig binary
    fn find_zig_bin() -> Result<(PathBuf, Vec<String>)> {
        let zig_path = zig_path()?;
        let output = Command::new(&zig_path).arg("version").output()?;

        let version_str = str::from_utf8(&output.stdout).with_context(|| {
            format!("`{} version` didn't return utf8 output", zig_path.display())
        })?;
        Self::validate_zig_version(version_str)?;
        Ok((zig_path, Vec::new()))
    }

    /// Detect the Python ziglang package
    fn find_zig_python() -> Result<(PathBuf, Vec<String>)> {
        let python_path = python_path()?;
        let output = Command::new(&python_path)
            .args(["-m", "ziglang", "version"])
            .output()?;

        let version_str = str::from_utf8(&output.stdout).with_context(|| {
            format!(
                "`{} -m ziglang version` didn't return utf8 output",
                python_path.display()
            )
        })?;
        Self::validate_zig_version(version_str)?;
        Ok((python_path, vec!["-m".to_string(), "ziglang".to_string()]))
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
        static LIB_DIR: OnceLock<PathBuf> = OnceLock::new();

        if let Some(cached) = LIB_DIR.get() {
            return Ok(cached.clone());
        }
        let (zig, zig_args) = Self::find_zig()?;
        let zig_version = Self::zig_version()?;
        let output = Command::new(zig).args(zig_args).arg("env").output()?;
        let parse_zon_lib_dir = || -> Result<PathBuf> {
            let output_str =
                str::from_utf8(&output.stdout).context("`zig env` didn't return utf8 output")?;
            let lib_dir = output_str
                .find(".lib_dir")
                .and_then(|idx| {
                    let bytes = output_str.as_bytes();
                    let mut start = idx;
                    while start < bytes.len() && bytes[start] != b'"' {
                        start += 1;
                    }
                    if start >= bytes.len() {
                        return None;
                    }
                    let mut end = start + 1;
                    while end < bytes.len() && bytes[end] != b'"' {
                        end += 1;
                    }
                    if end >= bytes.len() {
                        return None;
                    }
                    Some(&output_str[start + 1..end])
                })
                .context("Failed to parse lib_dir from `zig env` ZON output")?;
            Ok(PathBuf::from(lib_dir))
        };
        let lib_dir = if zig_version >= semver::Version::new(0, 15, 0) {
            parse_zon_lib_dir()?
        } else {
            serde_json::from_slice::<ZigEnv>(&output.stdout)
                .map(|zig_env| PathBuf::from(zig_env.lib_dir))
                .or_else(|_| parse_zon_lib_dir())?
        };
        Ok(LIB_DIR.get_or_init(|| lib_dir).clone())
    }
}

#[derive(Debug, Deserialize)]
struct ZigEnv {
    lib_dir: String,
}

fn python_path() -> Result<PathBuf> {
    let python = env::var("CARGO_ZIGBUILD_PYTHON_PATH").unwrap_or_else(|_| "python3".to_string());
    Ok(which::which(python)?)
}

fn zig_path() -> Result<PathBuf> {
    let zig = env::var("CARGO_ZIGBUILD_ZIG_PATH").unwrap_or_else(|_| "zig".to_string());
    Ok(which::which(zig)?)
}

pub(crate) fn cache_dir() -> PathBuf {
    env::var("CARGO_ZIGBUILD_CACHE_DIR")
        .ok()
        .map(|s| s.into())
        .or_else(dirs::cache_dir)
        // If the really is no cache dir, cwd will also do
        .unwrap_or_else(|| env::current_dir().expect("Failed to get current dir"))
        .join(env!("CARGO_PKG_NAME"))
        .join(env!("CARGO_PKG_VERSION"))
}
