use std::ops::{Deref, DerefMut};
use std::process;

use anyhow::{Context, Result};
use clap::Parser;

use crate::build::Build;

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
    /// Execute `cargo rustc` command with zig as the linker
    pub fn execute(&self) -> Result<()> {
        let build = Build {
            cargo: self.cargo.clone().into(),
            ..Default::default()
        };

        let mut rustc = build.build_command("rustc")?;

        if let Some(print) = self.cargo.print.as_ref() {
            rustc.arg("--print").arg(print);
        }
        if !self.cargo.crate_type.is_empty() {
            rustc
                .arg("--crate-type")
                .arg(self.cargo.crate_type.join(","));
        }
        if !self.cargo.args.is_empty() {
            rustc.arg("--").args(&self.cargo.args);
        }

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
