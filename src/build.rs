use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
#[cfg(feature = "universal2")]
use std::process::Stdio;
use std::process::{self, Child, Command};

use anyhow::{Context, Result};
use clap::Parser;

use crate::zig::Zig;

/// Compile a local package and all of its dependencies
/// using zig as the linker
#[derive(Clone, Debug, Default, Parser)]
#[command(
    after_help = "Run `cargo help build` for more detailed information.",
    display_order = 1
)]
pub struct Build {
    #[command(flatten)]
    pub cargo: cargo_options::Build,

    /// Disable zig linker
    #[arg(skip)]
    pub disable_zig_linker: bool,

    /// Enable zig ar
    #[arg(skip)]
    pub enable_zig_ar: bool,
}

impl Build {
    /// Create a new build from manifest path
    #[allow(clippy::field_reassign_with_default)]
    pub fn new(manifest_path: Option<PathBuf>) -> Self {
        let mut build = Self::default();
        build.manifest_path = manifest_path;
        build
    }

    /// Execute `cargo build` command with zig as the linker
    pub fn execute(&self) -> Result<()> {
        let has_universal2 = self
            .cargo
            .target
            .contains(&"universal2-apple-darwin".to_string());
        let mut build = self.build_command()?;
        let mut child = build.spawn().context("Failed to run cargo build")?;
        if has_universal2 {
            self.handle_universal2_build(child)?;
        } else {
            let status = child.wait().expect("Failed to wait on cargo build process");
            if !status.success() {
                process::exit(status.code().unwrap_or(1));
            }
        }
        Ok(())
    }

    #[cfg(not(feature = "universal2"))]
    fn handle_universal2_build(&self, mut _child: Child) -> Result<()> {
        anyhow::bail!("Unsupported Rust target: universal2-apple-darwin")
    }

    #[cfg(feature = "universal2")]
    fn handle_universal2_build(&self, mut child: Child) -> Result<()> {
        use cargo_metadata::Message;
        use std::io::BufReader;
        use std::path::Path;

        // Find root crate package id
        let manifest_path = self
            .manifest_path
            .as_deref()
            .unwrap_or_else(|| Path::new("Cargo.toml"));
        let mut metadata_cmd = cargo_metadata::MetadataCommand::new();
        metadata_cmd.manifest_path(manifest_path);
        let metadata = metadata_cmd.exec()?;
        let root_pkg = metadata.root_package().expect("Should have a root package");

        let mut x86_64_artifacts = Vec::new();
        let mut aarch64_artifacts = Vec::new();

        let stream = child
            .stdout
            .take()
            .expect("Cargo build should have a stdout");
        for message in Message::parse_stream(BufReader::new(stream)) {
            let message = message.context("Failed to parse cargo metadata message")?;
            match message {
                Message::CompilerArtifact(artifact) => {
                    if artifact.package_id == root_pkg.id {
                        for filename in artifact.filenames {
                            if filename.as_str().contains("x86_64-apple-darwin") {
                                x86_64_artifacts.push(filename);
                            } else if filename.as_str().contains("aarch64-apple-darwin") {
                                aarch64_artifacts.push(filename);
                            }
                        }
                    }
                }
                Message::CompilerMessage(msg) => {
                    println!("{}", msg.message);
                }
                _ => {}
            }
        }
        let status = child.wait().expect("Failed to wait on cargo build process");
        if !status.success() {
            process::exit(status.code().unwrap_or(1));
        }
        // create fat binaries for artifacts
        for (x86_64_path, aarch64_path) in x86_64_artifacts
            .into_iter()
            .zip(aarch64_artifacts.into_iter())
        {
            let mut fat = fat_macho::FatWriter::new();
            match fat.add(fs_err::read(&x86_64_path)?) {
                Err(fat_macho::Error::InvalidMachO(_)) => continue,
                Err(e) => return Err(e)?,
                Ok(()) => {}
            }
            match fat.add(fs_err::read(&aarch64_path)?) {
                Err(fat_macho::Error::InvalidMachO(_)) => continue,
                Err(e) => return Err(e)?,
                Ok(()) => {}
            }
            let universal2_path = PathBuf::from(
                x86_64_path
                    .to_string()
                    .replace("x86_64-apple-darwin", "universal2-apple-darwin"),
            );
            let universal2_dir = universal2_path.parent().unwrap();
            fs_err::create_dir_all(universal2_dir)?;
            fat.write_to_file(universal2_path)?;
        }
        Ok(())
    }

    /// Generate cargo subcommand
    #[cfg(not(feature = "universal2"))]
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

    /// Generate cargo subcommand
    #[cfg(feature = "universal2")]
    pub fn build_command(&self) -> Result<Command> {
        let build = if let Some(index) = self
            .cargo
            .target
            .iter()
            .position(|t| t == "universal2-apple-darwin")
        {
            let mut cargo = self.cargo.clone();
            cargo.target.remove(index);
            if !cargo.target.contains(&"x86_64-apple-darwin".to_string()) {
                cargo.target.push("x86_64-apple-darwin".to_string());
            }
            if !cargo.target.contains(&"aarch64-apple-darwin".to_string()) {
                cargo.target.push("aarch64-apple-darwin".to_string());
            }
            if !cargo.message_format.iter().any(|f| f.starts_with("json")) {
                cargo.message_format.push("json".to_string());
            }
            let mut build = cargo.command();
            build.stdout(Stdio::piped()).stderr(Stdio::inherit());
            if !self.disable_zig_linker {
                Zig::apply_command_env(
                    self.manifest_path.as_deref(),
                    self.release,
                    &cargo.common,
                    &mut build,
                    self.enable_zig_ar,
                )?;
            }
            build
        } else {
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
            build
        };
        Ok(build)
    }
}

impl Deref for Build {
    type Target = cargo_options::Build;

    fn deref(&self) -> &Self::Target {
        &self.cargo
    }
}

impl DerefMut for Build {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.cargo
    }
}

impl From<cargo_options::Build> for Build {
    fn from(cargo: cargo_options::Build) -> Self {
        Self {
            cargo,
            ..Default::default()
        }
    }
}
