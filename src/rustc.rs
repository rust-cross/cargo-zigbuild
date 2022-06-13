use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::process;

use anyhow::{Context, Result};
use clap::Parser;

use crate::Zig;

/// Compile a package, and pass extra options to the compiler
/// with zig as the linker
#[derive(Clone, Debug, Default, Parser)]
#[clap(
    setting = clap::AppSettings::DeriveDisplayOrder,
    trailing_var_arg = true,
    after_help = "Run `cargo help rustc` for more detailed information."
)]
pub struct Rustc {
    #[clap(flatten)]
    pub cargo: cargo_options::Rustc,
}

impl Rustc {
    /// Create a new build from manifest path
    #[allow(clippy::field_reassign_with_default)]
    pub fn new(manifest_path: Option<PathBuf>) -> Self {
        let mut build = Self::default();
        build.manifest_path = manifest_path;
        build
    }

    /// Execute `cargo rustc` command with zig as the linker
    pub fn execute(&self) -> Result<()> {
        let mut rustc = self.cargo.command();
        Zig::apply_command_env(&self.cargo.common, &mut rustc)?;

        let mut child = rustc.spawn().context("Failed to run cargo rustc")?;
        let status = child.wait().expect("Failed to wait on cargo build process");
        if !status.success() {
            process::exit(status.code().unwrap_or(1));
        }
        Ok(())
    }
}

impl Deref for Rustc {
    type Target = cargo_options::Rustc;

    fn deref(&self) -> &Self::Target {
        &self.cargo
    }
}

impl DerefMut for Rustc {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.cargo
    }
}

impl From<cargo_options::Rustc> for Rustc {
    fn from(cargo: cargo_options::Rustc) -> Self {
        Self { cargo }
    }
}
