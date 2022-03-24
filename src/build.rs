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
    /// Do not print cargo log messages
    #[clap(short = 'q', long)]
    pub quiet: bool,

    /// Package to build (see `cargo help pkgid`)
    #[clap(
        short = 'p',
        long = "package",
        value_name = "SPEC",
        multiple_values = true
    )]
    pub packages: Vec<String>,

    /// Build all packages in the workspace
    #[clap(long)]
    pub workspace: bool,

    /// Exclude packages from the build
    #[clap(long, value_name = "SPEC", multiple_values = true)]
    pub exclude: Vec<String>,

    /// Alias for workspace (deprecated)
    #[clap(long)]
    pub all: bool,

    /// Number of parallel jobs, defaults to # of CPUs
    #[clap(short = 'j', long, value_name = "N")]
    pub jobs: Option<usize>,

    /// Build only this package's library
    #[clap(long)]
    pub lib: bool,

    /// Build only the specified binary
    #[clap(long, value_name = "NAME", multiple_values = true)]
    pub bin: Vec<String>,

    /// Build all binaries
    #[clap(long)]
    pub bins: bool,

    /// Build only the specified example
    #[clap(long, value_name = "NAME", multiple_values = true)]
    pub example: Vec<String>,

    /// Build all examples
    #[clap(long)]
    pub examples: bool,

    /// Build only the specified test target
    #[clap(long, value_name = "NAME", multiple_values = true)]
    pub test: Vec<String>,

    /// Build all tests
    #[clap(long)]
    pub tests: bool,

    /// Build only the specified bench target
    #[clap(long, value_name = "NAME", multiple_values = true)]
    pub bench: Vec<String>,

    /// Build all benches
    #[clap(long)]
    pub benches: bool,

    /// Build all targets
    #[clap(long)]
    pub all_targets: bool,

    /// Build artifacts in release mode, with optimizations
    #[clap(short = 'r', long)]
    pub release: bool,

    /// Build artifacts with the specified Cargo profile
    #[clap(long, value_name = "PROFILE-NAME")]
    pub profile: Option<String>,

    /// Space or comma separated list of features to activate
    #[clap(long, multiple_values = true)]
    pub features: Vec<String>,

    /// Activate all available features
    #[clap(long)]
    pub all_features: bool,

    /// Do not activate the `default` feature
    #[clap(long)]
    pub no_default_features: bool,

    /// Build for the target triple
    #[clap(
        long,
        value_name = "TRIPLE",
        env = "CARGO_BUILD_TARGET",
        multiple_occurrences = true
    )]
    pub target: Vec<String>,

    /// Directory for all generated artifacts
    #[clap(long, value_name = "DIRECTORY", parse(from_os_str))]
    pub target_dir: Option<PathBuf>,

    /// Copy final artifacts to this directory (unstable)
    #[clap(long, value_name = "PATH", parse(from_os_str))]
    pub out_dir: Option<PathBuf>,

    /// Path to Cargo.toml
    #[clap(long, value_name = "PATH", parse(from_os_str))]
    pub manifest_path: Option<PathBuf>,

    /// Ignore `rust-version` specification in packages
    #[clap(long)]
    pub ignore_rust_version: bool,

    /// Error format
    #[clap(long, value_name = "FMT", multiple_values = true)]
    pub message_format: Vec<String>,

    /// Output the build plan in JSON (unstable)
    #[clap(long)]
    pub build_plan: bool,

    /// Output build graph in JSON (unstable)
    #[clap(long)]
    pub unit_graph: bool,

    /// Outputs a future incompatibility report at the end of the build (unstable)
    #[clap(long)]
    pub future_incompat_report: bool,

    /// Use verbose output (-vv very verbose/build.rs output)
    #[clap(short = 'v', long, parse(from_occurrences), max_occurrences = 2)]
    pub verbose: usize,

    /// Coloring: auto, always, never
    #[clap(long, value_name = "WHEN")]
    pub color: Option<String>,

    /// Require Cargo.lock and cache are up to date
    #[clap(long)]
    pub frozen: bool,

    /// Require Cargo.lock is up to date
    #[clap(long)]
    pub locked: bool,

    /// Run without accessing the network
    #[clap(long)]
    pub offline: bool,

    /// Override a configuration value (unstable)
    #[clap(long, value_name = "KEY=VALUE", multiple_values = true)]
    pub config: Vec<String>,

    /// Unstable (nightly-only) flags to Cargo, see 'cargo -Z help' for details
    #[clap(short = 'Z', value_name = "FLAG", multiple_values = true)]
    pub unstable_flags: Vec<String>,

    /// Disable zig linker
    #[clap(skip)]
    pub disable_zig_linker: bool,
}

impl Build {
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
            .target
            .iter()
            .map(|target| target.split_once('.').map(|(t, _)| t).unwrap_or(target))
            .collect::<Vec<&str>>();

        // collect cargo build arguments
        if self.quiet {
            build.arg("--quiet");
        }
        for pkg in &self.packages {
            build.arg("--package").arg(pkg);
        }
        if self.workspace {
            build.arg("--workspace");
        }
        for item in &self.exclude {
            build.arg("--exclude").arg(item);
        }
        if self.all {
            build.arg("--all");
        }
        if let Some(jobs) = self.jobs {
            build.arg("--jobs").arg(jobs.to_string());
        }
        if self.lib {
            build.arg("--lib");
        }
        for bin in &self.bin {
            build.arg("--bin").arg(bin);
        }
        if self.bins {
            build.arg("--bins");
        }
        for example in &self.example {
            build.arg("--example").arg(example);
        }
        if self.examples {
            build.arg("--examples");
        }
        for test in &self.test {
            build.arg("--test").arg(test);
        }
        if self.tests {
            build.arg("--tests");
        }
        for bench in &self.bench {
            build.arg("--bench").arg(bench);
        }
        if self.benches {
            build.arg("--benches");
        }
        if self.all_targets {
            build.arg("--all-targets");
        }
        if self.release {
            build.arg("--release");
        }
        if let Some(profile) = self.profile.as_ref() {
            build.arg("--profile").arg(profile);
        }
        for feature in &self.features {
            build.arg("--features").arg(feature);
        }
        if self.all_features {
            build.arg("--all-features");
        }
        if self.no_default_features {
            build.arg("--no-default-features");
        }

        rust_targets.iter().for_each(|target| {
            build.arg("--target").arg(&target);
        });

        if let Some(dir) = self.target_dir.as_ref() {
            build.arg("--target-dir").arg(dir);
        }
        if let Some(dir) = self.out_dir.as_ref() {
            build.arg("--out-dir").arg(dir);
        }
        if let Some(path) = self.manifest_path.as_ref() {
            build.arg("--manifest-path").arg(path);
        }
        if self.ignore_rust_version {
            build.arg("--ignore-rust-version");
        }
        for fmt in &self.message_format {
            build.arg("--message-format").arg(fmt);
        }
        if self.build_plan {
            build.arg("--build-plan");
        }
        if self.unit_graph {
            build.arg("--unit-graph");
        }
        if self.future_incompat_report {
            build.arg("--future-incompat-report");
        }
        if self.verbose > 0 {
            build.arg(format!("-{}", "v".repeat(self.verbose)));
        }
        if let Some(color) = self.color.as_ref() {
            build.arg("--color").arg(color);
        }
        if self.frozen {
            build.arg("--frozen");
        }
        if self.locked {
            build.arg("--locked");
        }
        if self.offline {
            build.arg("--offline");
        }
        for config in &self.config {
            build.arg("--config").arg(config);
        }
        for flag in &self.unstable_flags {
            build.arg("-Z").arg(flag);
        }

        if !self.disable_zig_linker {
            // setup zig as linker
            let rustc_meta = rustc_version::version_meta()?;
            let host_target = &rustc_meta.host;
            for (parsed_target, raw_target) in rust_targets.iter().zip(&self.target) {
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

                    // Enable unstable `target-applies-to-host` option automatically for nightly Rust
                    // when target is the same as host but may have specified glibc version
                    if host_target == raw_target
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
        for target in &self.target {
            if target.contains("apple") {
                let target_dir = if let Some(target_dir) = self.target_dir.clone() {
                    target_dir.join(target)
                } else {
                    let manifest_path = self
                        .manifest_path
                        .as_deref()
                        .unwrap_or_else(|| Path::new("Cargo.toml"));
                    let mut metadata_cmd = cargo_metadata::MetadataCommand::new();
                    metadata_cmd.manifest_path(&manifest_path);
                    let metadata = metadata_cmd.exec()?;
                    metadata.target_directory.into_std_path_buf().join(target)
                };
                let profile = match self.profile.as_deref() {
                    Some("dev" | "test") => "debug",
                    Some("release" | "bench") => "release",
                    Some(profile) => profile,
                    None => {
                        if self.release {
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
