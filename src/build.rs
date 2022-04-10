use std::env;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::process::{self, Command};

use anyhow::{Context, Result};
use clap::Parser;
use fs_err as fs;
use path_slash::PathBufExt;

use crate::linux::ARM_FEATURES_H;
use crate::macos::LIBICONV_TBD;
use crate::zig::{is_mingw_shell, prepare_zig_linker, Zig};

/// Compile a local package and all of its dependencies
/// using zig as the linker
#[derive(Clone, Debug, Default, Parser)]
#[clap(setting = clap::AppSettings::DeriveDisplayOrder, after_help = "Run `cargo help build` for more detailed information.")]
pub struct Build {
    #[clap(flatten)]
    pub cargo: cargo_options::Build,

    /// Disable zig linker
    #[clap(skip)]
    pub disable_zig_linker: bool,
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
        let mut build = self.build_command("build")?;
        let mut child = build.spawn().context("Failed to run cargo build")?;
        let status = child.wait().expect("Failed to wait on cargo build process");
        if !status.success() {
            process::exit(status.code().unwrap_or(1));
        }
        Ok(())
    }

    /// Generate cargo subcommand
    pub fn build_command(&self, subcommand: &str) -> Result<Command> {
        let mut build = Command::new("cargo");
        build.arg(subcommand);

        let rust_targets = self
            .cargo
            .target
            .iter()
            .map(|target| target.split_once('.').map(|(t, _)| t).unwrap_or(target))
            .collect::<Vec<&str>>();

        // collect cargo build arguments
        if self.cargo.quiet {
            build.arg("--quiet");
        }
        for pkg in &self.cargo.packages {
            build.arg("--package").arg(pkg);
        }
        if self.cargo.workspace {
            build.arg("--workspace");
        }
        for item in &self.cargo.exclude {
            build.arg("--exclude").arg(item);
        }
        if self.cargo.all {
            build.arg("--all");
        }
        if let Some(jobs) = self.cargo.jobs {
            build.arg("--jobs").arg(jobs.to_string());
        }
        if self.cargo.lib {
            build.arg("--lib");
        }
        for bin in &self.cargo.bin {
            build.arg("--bin").arg(bin);
        }
        if self.cargo.bins {
            build.arg("--bins");
        }
        for example in &self.cargo.example {
            build.arg("--example").arg(example);
        }
        if self.cargo.examples {
            build.arg("--examples");
        }
        for test in &self.cargo.test {
            build.arg("--test").arg(test);
        }
        if self.cargo.tests {
            build.arg("--tests");
        }
        for bench in &self.cargo.bench {
            build.arg("--bench").arg(bench);
        }
        if self.cargo.benches {
            build.arg("--benches");
        }
        if self.cargo.all_targets {
            build.arg("--all-targets");
        }
        if self.cargo.release {
            build.arg("--release");
        }
        if let Some(profile) = self.cargo.profile.as_ref() {
            build.arg("--profile").arg(profile);
        }
        for feature in &self.cargo.features {
            build.arg("--features").arg(feature);
        }
        if self.cargo.all_features {
            build.arg("--all-features");
        }
        if self.cargo.no_default_features {
            build.arg("--no-default-features");
        }

        rust_targets.iter().for_each(|target| {
            build.arg("--target").arg(&target);
        });

        if let Some(dir) = self.cargo.target_dir.as_ref() {
            build.arg("--target-dir").arg(dir);
        }
        if let Some(dir) = self.cargo.out_dir.as_ref() {
            build.arg("--out-dir").arg(dir);
        }
        if let Some(path) = self.cargo.manifest_path.as_ref() {
            build.arg("--manifest-path").arg(path);
        }
        if self.cargo.ignore_rust_version {
            build.arg("--ignore-rust-version");
        }
        for fmt in &self.cargo.message_format {
            build.arg("--message-format").arg(fmt);
        }
        if self.cargo.build_plan {
            build.arg("--build-plan");
        }
        if self.cargo.unit_graph {
            build.arg("--unit-graph");
        }
        if self.cargo.future_incompat_report {
            build.arg("--future-incompat-report");
        }
        if self.cargo.verbose > 0 {
            build.arg(format!("-{}", "v".repeat(self.cargo.verbose)));
        }
        if let Some(color) = self.cargo.color.as_ref() {
            build.arg("--color").arg(color);
        }
        if self.cargo.frozen {
            build.arg("--frozen");
        }
        if self.cargo.locked {
            build.arg("--locked");
        }
        if self.cargo.offline {
            build.arg("--offline");
        }
        for config in &self.cargo.config {
            build.arg("--config").arg(config);
        }
        for flag in &self.cargo.unstable_flags {
            build.arg("-Z").arg(flag);
        }

        if !self.disable_zig_linker {
            // setup zig as linker
            let rustc_meta = rustc_version::version_meta()?;
            let host_target = &rustc_meta.host;
            for (parsed_target, raw_target) in rust_targets.iter().zip(&self.cargo.target) {
                // we only setup zig as linker when target isn't exactly the same as host target
                if host_target != raw_target {
                    let env_target = parsed_target.replace('-', "_");
                    let (zig_cc, zig_cxx) = prepare_zig_linker(raw_target)?;
                    if is_mingw_shell() {
                        let zig_cc = zig_cc.to_slash_lossy();
                        build.env(format!("CC_{}", env_target), &zig_cc);
                        build.env(format!("CXX_{}", env_target), &zig_cxx.to_slash_lossy());
                        build.env(
                            format!("CARGO_TARGET_{}_LINKER", env_target.to_uppercase()),
                            &zig_cc,
                        );
                    } else {
                        build.env(format!("CC_{}", env_target), &zig_cc);
                        build.env(format!("CXX_{}", env_target), &zig_cxx);
                        build.env(
                            format!("CARGO_TARGET_{}_LINKER", env_target.to_uppercase()),
                            &zig_cc,
                        );
                    }

                    self.setup_os_deps()?;

                    if raw_target.contains("windows-gnu") {
                        build.env("WINAPI_NO_BUNDLED_LIBRARIES", "1");
                    }

                    if raw_target.contains("apple-darwin") {
                        if let Some(sdkroot) = env::var_os("SDKROOT") {
                            if !sdkroot.is_empty()
                                && env::var_os("PKG_CONFIG_SYSROOT_DIR").is_none()
                            {
                                // Set PKG_CONFIG_SYSROOT_DIR for pkg-config crate
                                build.env("PKG_CONFIG_SYSROOT_DIR", sdkroot);
                            }
                        }
                    }

                    // Enable unstable `target-applies-to-host` option automatically for nightly Rust
                    // when target is the same as host but may have specified glibc version
                    if host_target == parsed_target
                        && matches!(rustc_meta.channel, rustc_version::Channel::Nightly)
                    {
                        build.env("CARGO_UNSTABLE_TARGET_APPLIES_TO_HOST", "true");
                        build.env("CARGO_TARGET_APPLIES_TO_HOST", "false");
                    }
                }
            }
        }

        Ok(build)
    }

    fn setup_os_deps(&self) -> Result<()> {
        for target in &self.cargo.target {
            if target.contains("apple") {
                let target_dir = if let Some(target_dir) = self.cargo.target_dir.clone() {
                    target_dir.join(target)
                } else {
                    let manifest_path = self
                        .cargo
                        .manifest_path
                        .as_deref()
                        .unwrap_or_else(|| Path::new("Cargo.toml"));
                    let mut metadata_cmd = cargo_metadata::MetadataCommand::new();
                    metadata_cmd.manifest_path(&manifest_path);
                    let metadata = metadata_cmd.exec()?;
                    metadata.target_directory.into_std_path_buf().join(target)
                };
                let profile = match self.cargo.profile.as_deref() {
                    Some("dev" | "test") => "debug",
                    Some("release" | "bench") => "release",
                    Some(profile) => profile,
                    None => {
                        if self.cargo.release {
                            "release"
                        } else {
                            "debug"
                        }
                    }
                };
                let deps_dir = target_dir.join(profile).join("deps");
                fs::create_dir_all(&deps_dir)?;
                fs::write(deps_dir.join("libiconv.tbd"), LIBICONV_TBD)?;
            } else if target.contains("arm") && target.contains("linux") {
                // See https://github.com/ziglang/zig/issues/3287
                if let Ok(lib_dir) = Zig::lib_dir() {
                    let arm_features_h = lib_dir
                        .join("libc")
                        .join("glibc")
                        .join("sysdeps")
                        .join("arm")
                        .join("arm-features.h");
                    if !arm_features_h.is_file() {
                        fs::write(arm_features_h, ARM_FEATURES_H)?;
                    }
                }
            }
        }
        Ok(())
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
            disable_zig_linker: false,
        }
    }
}
