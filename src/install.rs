use std::ops::{Deref, DerefMut};
use std::process::{self, Command};

use anyhow::{Context, Result};
use clap::Parser;

use crate::zig::Zig;

/// Install a Rust binary using zig as the linker. Default location is $HOME/.cargo/bin
#[derive(Clone, Debug, Default, Parser)]
#[command(
    after_help = "Run `cargo help install` for more detailed information.",
    display_order = 1
)]
pub struct Install {
    #[command(flatten)]
    pub cargo: cargo_options::Install,

    /// Disable zig linker
    #[arg(skip)]
    pub disable_zig_linker: bool,

    /// Enable zig ar
    #[arg(skip)]
    pub enable_zig_ar: bool,
}

impl Install {
    /// Create a new install
    pub fn new() -> Self {
        Self::default()
    }

    /// Execute `cargo install` command with zig as the linker
    pub fn execute(&self) -> Result<()> {
        let mut build = self.build_command()?;
        let mut child = build.spawn().context("Failed to run cargo install")?;
        let status = child
            .wait()
            .expect("Failed to wait on cargo install process");
        if !status.success() {
            process::exit(status.code().unwrap_or(1));
        }
        Ok(())
    }

    /// Generate cargo subcommand
    pub fn build_command(&self) -> Result<Command> {
        let mut install = self.cargo.command();
        if !self.disable_zig_linker {
            Zig::apply_command_env(
                None,
                !self.debug,
                &self.cargo.common,
                &mut install,
                self.enable_zig_ar,
            )?;
        }

        Ok(install)
    }
}

impl Deref for Install {
    type Target = cargo_options::Install;

    fn deref(&self) -> &Self::Target {
        &self.cargo
    }
}

impl DerefMut for Install {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.cargo
    }
}

impl From<cargo_options::Install> for Install {
    fn from(cargo: cargo_options::Install) -> Self {
        Self {
            cargo,
            ..Default::default()
        }
    }
}
