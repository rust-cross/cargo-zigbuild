use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::process::{self, Command};

use anyhow::{Context, Result};
use clap::Parser;

use crate::Zig;

#[derive(Clone, Debug, Default, Parser)]
#[command(
    display_order = 1,
    after_help = "Run `cargo help clippy` for more detailed information."
)]
pub struct Clippy {
    #[command(flatten)]
    pub cargo: cargo_options::Clippy,

    /// Disable zig linker
    #[arg(skip)]
    pub disable_zig_linker: bool,

    /// Enable zig ar
    #[arg(skip)]
    pub enable_zig_ar: bool,
}

impl Clippy {
    /// Create a new run from manifest path
    #[allow(clippy::field_reassign_with_default)]
    pub fn new(manifest_path: Option<PathBuf>) -> Self {
        let mut build = Self::default();
        build.manifest_path = manifest_path;
        build
    }

    /// Execute `cargo clippy` command
    pub fn execute(&self) -> Result<()> {
        let mut run = self.build_command()?;

        let mut child = run.spawn().context("Failed to run cargo clippy")?;
        let status = child
            .wait()
            .expect("Failed to wait on cargo clippy process");
        if !status.success() {
            process::exit(status.code().unwrap_or(1));
        }
        Ok(())
    }

    /// Generate cargo subcommand
    pub fn build_command(&self) -> Result<Command> {
        let mut build = self.cargo.command();
        if !self.disable_zig_linker {
            Zig::apply_command_env(
                self.manifest_path.as_deref(),
                self.release,
                &self.cargo.common,
                &mut build,
                self.enable_zig_ar,
            )?;
        }

        Ok(build)
    }
}

impl Deref for Clippy {
    type Target = cargo_options::Clippy;

    fn deref(&self) -> &Self::Target {
        &self.cargo
    }
}

impl DerefMut for Clippy {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.cargo
    }
}

impl From<cargo_options::Clippy> for Clippy {
    fn from(cargo: cargo_options::Clippy) -> Self {
        Self {
            cargo,
            ..Default::default()
        }
    }
}
