use std::env;
use std::path::PathBuf;
use std::process::{self, Command};
use std::str;

use anyhow::{Context, Result};
use clap::Parser;
use fs_err as fs;

use crate::zig::{prepare_zig_linker, Zig};

/// Compile a local package and all of its dependencies
/// using zig as linker
#[derive(Debug, Parser)]
#[clap(setting = clap::AppSettings::DeriveDisplayOrder, after_help = "Run `cargo help build` for more detailed information.")]
pub struct Build {
    /// Do not print cargo log messages
    #[clap(short = 'q', long)]
    quiet: bool,

    /// Package to build (see `cargo help pkgid`)
    #[clap(
        short = 'p',
        long = "package",
        value_name = "SPEC",
        multiple_values = true
    )]
    packages: Vec<String>,

    /// Build all packages in the workspace
    #[clap(long)]
    workspace: bool,

    /// Exclude packages from the build
    #[clap(long, value_name = "SPEC", multiple_values = true)]
    exclude: Vec<String>,

    /// Alias for workspace (deprecated)
    #[clap(long)]
    all: bool,

    /// Number of parallel jobs, defaults to # of CPUs
    #[clap(short = 'j', long, value_name = "N")]
    jobs: Option<usize>,

    /// Build only this package's library
    #[clap(long)]
    lib: bool,

    /// Build only the specified binary
    #[clap(long, value_name = "NAME", multiple_values = true)]
    bin: Vec<String>,

    /// Build all binaries
    #[clap(long)]
    bins: bool,

    /// Build only the specified example
    #[clap(long, value_name = "NAME", multiple_values = true)]
    example: Vec<String>,

    /// Build all examples
    #[clap(long)]
    examples: bool,

    /// Build only the specified test target
    #[clap(long, value_name = "NAME", multiple_values = true)]
    test: Vec<String>,

    /// Build all tests
    #[clap(long)]
    tests: bool,

    /// Build only the specified bench target
    #[clap(long, value_name = "NAME", multiple_values = true)]
    bench: Vec<String>,

    /// Build all benches
    #[clap(long)]
    benches: bool,

    /// Build all targets
    #[clap(long)]
    all_targets: bool,

    /// Build artifacts in release mode, with optimizations
    #[clap(long)]
    release: bool,

    /// Build artifacts with the specified Cargo profile
    #[clap(long, value_name = "PROFILE-NAME")]
    profile: Option<String>,

    /// Space or comma separated list of features to activate
    #[clap(long, multiple_values = true)]
    features: Vec<String>,

    /// Activate all available features
    #[clap(long)]
    all_features: bool,

    /// Do not activate the `default` feature
    #[clap(long)]
    no_default_features: bool,

    /// Build for the target triple
    #[clap(long, value_name = "TRIPLE", env = "CARGO_BUILD_TARGET")]
    target: Option<String>,

    /// Directory for all generated artifacts
    #[clap(long, value_name = "DIRECTORY", parse(from_os_str))]
    target_dir: Option<PathBuf>,

    /// Copy final artifacts to this directory (unstable)
    #[clap(long, value_name = "PATH", parse(from_os_str))]
    out_dir: Option<PathBuf>,

    /// Path to Cargo.toml
    #[clap(long, value_name = "PATH", parse(from_os_str))]
    manifest_path: Option<PathBuf>,

    /// Ignore `rust-version` specification in packages
    #[clap(long)]
    ignore_rust_version: bool,

    /// Error format
    #[clap(long, value_name = "FMT", multiple_values = true)]
    message_format: Vec<String>,

    /// Output the build plan in JSON (unstable)
    #[clap(long)]
    build_plan: bool,

    /// Output build graph in JSON (unstable)
    #[clap(long)]
    unit_graph: bool,

    /// Outputs a future incompatibility report at the end of the build (unstable)
    #[clap(long)]
    future_incompat_report: bool,

    /// Use verbose output (-vv very verbose/build.rs output)
    #[clap(short = 'v', long, parse(from_occurrences), max_occurrences = 2)]
    verbose: usize,

    /// Coloring: auto, always, never
    #[clap(long, value_name = "WHEN")]
    color: Option<String>,

    /// Require Cargo.lock and cache are up to date
    #[clap(long)]
    frozen: bool,

    /// Require Cargo.lock is up to date
    #[clap(long)]
    locked: bool,

    /// Run without accessing the network
    #[clap(long)]
    offline: bool,

    /// Override a configuration value (unstable)
    #[clap(long, value_name = "KEY=VALUE", multiple_values = true)]
    config: Vec<String>,

    /// Unstable (nightly-only) flags to Cargo, see 'cargo -Z help' for details
    #[clap(short = 'Z', value_name = "FLAG", multiple_values = true)]
    unstable_flags: Vec<String>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Parser)]
#[clap(
    version,
    name = "cargo",
    global_setting(clap::AppSettings::DeriveDisplayOrder)
)]
pub enum Opt {
    #[clap(name = "zigbuild")]
    Build(Build),
    #[clap(subcommand)]
    Zig(Zig),
}

impl Build {
    pub fn execute(&self) -> Result<()> {
        let mut build = Command::new("cargo");
        build.arg("build");

        let rust_target = self
            .target
            .as_ref()
            .map(|target| target.split_once('.').map(|(t, _)| t).unwrap_or(target));

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
            build.arg("--excude").arg(item);
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
        if let Some(rust_target) = rust_target {
            build.arg("--target").arg(&rust_target);
        }
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

        // setup zig as linker
        if let Some(target) = self.target.as_ref() {
            if let Some(rust_target) = rust_target {
                let (zig_cc, zig_cxx) = prepare_zig_linker(target)?;
                let env_target = rust_target.to_uppercase().replace("-", "_");
                build.env("TARGET_CC", &zig_cc);
                build.env("TARGET_CXX", &zig_cxx);
                build.env(format!("CARGO_TARGET_{}_LINKER", env_target), &zig_cc);

                self.setup_os_deps()?;

                if rust_target.contains("windows-gnu") {
                    build.env("WINAPI_NO_BUNDLED_LIBRARIES", "1");
                }
            }
        }

        let mut child = build.spawn().context("Failed to run cargo build")?;
        let status = child.wait().expect("Failed to wait on cargo build process");
        if !status.success() {
            process::exit(status.code().unwrap_or(1));
        }
        Ok(())
    }

    fn setup_os_deps(&self) -> Result<()> {
        if let Some(target) = self.target.as_ref() {
            if target.contains("apple") {
                let target_dir = self
                    .target_dir
                    .clone()
                    .or_else(|| {
                        self.manifest_path
                            .as_ref()
                            .map(|m| m.parent().unwrap().join("target"))
                    })
                    .unwrap_or_else(|| {
                        env::current_dir()
                            .expect("Failed to get current working directory")
                            .join("target")
                    })
                    .join(target);
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
                fs::write(
                    deps_dir.join("libiconv.tbd"),
                    include_str!("macos/libiconv.tbd"),
                )?;
            }
        }
        Ok(())
    }
}
