use std::path::PathBuf;
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
    /// Do not print cargo log messages
    #[clap(short = 'q', long)]
    pub quiet: bool,

    /// Package to build (see `cargo help pkgid`)
    #[clap(short = 'p', long = "package", value_name = "SPEC")]
    pub packages: Vec<String>,

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
    #[clap(long, value_name = "TRIPLE", env = "CARGO_BUILD_TARGET")]
    pub target: Option<String>,

    /// Output compiler information without compiling
    #[clap(long, value_name = "INFO")]
    pub print: Option<String>,

    /// Comma separated list of types of crates for the compiler to emit (unstable)
    #[clap(
        long,
        value_name = "CRATE-TYPE",
        use_value_delimiter = true,
        multiple_values = true
    )]
    pub crate_type: Vec<String>,

    /// Directory for all generated artifacts
    #[clap(long, value_name = "DIRECTORY", parse(from_os_str))]
    pub target_dir: Option<PathBuf>,

    /// Path to Cargo.toml
    #[clap(long, value_name = "PATH", parse(from_os_str))]
    pub manifest_path: Option<PathBuf>,

    /// Ignore `rust-version` specification in packages
    #[clap(long)]
    pub ignore_rust_version: bool,

    /// Error format
    #[clap(long, value_name = "FMT", multiple_values = true)]
    pub message_format: Vec<String>,

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

    /// Rustc flags
    #[clap(takes_value = true, multiple_values = true)]
    pub args: Vec<String>,
}

impl Rustc {
    /// Execute `cargo rustc` command with zig as the linker
    pub fn execute(&self) -> Result<()> {
        let build = Build {
            quiet: self.quiet,
            packages: self.packages.clone(),
            jobs: self.jobs,
            lib: self.lib,
            bin: self.bin.clone(),
            bins: self.bins,
            example: self.example.clone(),
            examples: self.examples,
            test: self.test.clone(),
            tests: self.tests,
            bench: self.bench.clone(),
            benches: self.benches,
            all_targets: self.all_targets,
            release: self.release,
            profile: self.profile.clone(),
            features: self.features.clone(),
            all_features: self.all_features,
            no_default_features: self.no_default_features,
            target: self.target.clone(),
            target_dir: self.target_dir.clone(),
            manifest_path: self.manifest_path.clone(),
            ignore_rust_version: self.ignore_rust_version,
            message_format: self.message_format.clone(),
            unit_graph: self.unit_graph,
            future_incompat_report: self.future_incompat_report,
            verbose: self.verbose,
            color: self.color.clone(),
            frozen: self.frozen,
            locked: self.locked,
            offline: self.offline,
            config: self.config.clone(),
            unstable_flags: self.unstable_flags.clone(),
            ..Default::default()
        };

        let mut rustc = build.build_command("rustc")?;

        if let Some(print) = self.print.as_ref() {
            rustc.arg("--print").arg(print);
        }
        if !self.crate_type.is_empty() {
            rustc.arg("--crate-type").arg(self.crate_type.join(","));
        }
        if !self.args.is_empty() {
            rustc.arg("--").args(&self.args);
        }

        let mut child = rustc.spawn().context("Failed to run cargo rustc")?;
        let status = child.wait().expect("Failed to wait on cargo build process");
        if !status.success() {
            process::exit(status.code().unwrap_or(1));
        }
        Ok(())
    }
}
