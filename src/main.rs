use anyhow::{bail, format_err, Context, Result};
use clap::Parser;
use std::process::{self, Command};
use std::str;

mod cli;
mod zig;

use cli::{Build, Opt};

impl Build {
    fn execute(&self) -> Result<()> {
        let mut build = Command::new("cargo");
        build.arg("build");

        let target = if let Some(target) = self.target.as_ref() {
            build.arg("--target").arg(target);
            target.clone()
        } else {
            get_host_target()?
        };
        let (zig_cc, zig_cxx) = zig::prepare_zig_linker(&target)?;

        let env_target = target.to_uppercase().replace("-", "_");
        build.env("TARGET_CC", &zig_cc);
        build.env("TARGET_CXX", &zig_cxx);
        build.env(format!("CARGO_TARGET_{}_LINKER", env_target), &zig_cc);

        let mut child = build.spawn().context("Failed to run cargo build")?;
        let status = child.wait().expect("Failed to wait on cargo build process");
        if !status.success() {
            process::exit(status.code().unwrap_or(1));
        }
        Ok(())
    }
}

fn main() -> Result<()> {
    let opt = Opt::parse();
    match opt {
        Opt::Build(build) => {
            build.execute()?;
        }
        Opt::Zig(zig) => zig.execute()?,
    }
    Ok(())
}

pub(crate) fn get_host_target() -> Result<String> {
    let output = Command::new("rustc").arg("-vV").output();
    let output = match output {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            bail!(
                "rustc, the rust compiler, is not installed or not in PATH. \
                This package requires Rust and Cargo to compile extensions. \
                Install it through the system's package manager or via https://rustup.rs/.",
            );
        }
        Err(err) => {
            return Err(err).context("Failed to run rustc to get the host target");
        }
        Ok(output) => output,
    };

    let output = str::from_utf8(&output.stdout).context("`rustc -vV` didn't return utf8 output")?;

    let field = "host: ";
    let host = output
        .lines()
        .find(|l| l.starts_with(field))
        .map(|l| &l[field.len()..])
        .ok_or_else(|| {
            format_err!(
                "`rustc -vV` didn't have a line for `{}`, got:\n{}",
                field.trim(),
                output
            )
        })?
        .to_string();
    Ok(host)
}
