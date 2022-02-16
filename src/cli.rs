use std::path::PathBuf;

use clap::Parser;

use crate::zig::Zig;

/// Compile a local package and all of its dependencies
/// using zig as linker
#[derive(Debug, Parser)]
#[clap(setting = clap::AppSettings::DeriveDisplayOrder, after_help = "Run `cargo help build` for more detailed information.")]
pub struct Build {
    /// Do not print cargo log messages
    #[clap(short = 'q', long)]
    quiet: bool,

    /// Package to build (see `cargo help pkgid`)
    #[clap(short = 'p', long = "package")]
    packages: Vec<String>,

    /// Build all packages in the workspace
    #[clap(long)]
    workspace: bool,

    /// Exclude packages from the build
    #[clap(long)]
    exclude: Vec<String>,

    /// Alias for workspace (deprecated)
    #[clap(long)]
    all: bool,

    /// Number of parallel jobs, defaults to # of CPUs
    #[clap(short = 'j', long)]
    jobs: Option<usize>,

    /// Build only this package's library
    #[clap(long)]
    lib: bool,

    /// Build only the specified binary
    #[clap(long)]
    bin: Vec<String>,

    /// Build all binaries
    #[clap(long)]
    bins: bool,

    /// Build only the specified example
    #[clap(long)]
    example: Vec<String>,

    /// Build all examples
    #[clap(long)]
    examples: bool,

    /// Build only the specified test target
    #[clap(long)]
    test: Vec<String>,

    /// Build all tests
    #[clap(long)]
    tests: bool,

    /// Build only the specified bench target
    #[clap(long)]
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
    #[clap(long)]
    features: Vec<String>,

    /// Activate all available features
    #[clap(long)]
    all_features: bool,

    /// Do not activate the `default` feature
    #[clap(long)]
    no_default_features: bool,

    /// Build for the target triple
    #[clap(long, value_name = "TRIPLE")]
    pub target: Option<String>,

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
    #[clap(long, value_name = "FMT")]
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
    verbose: u64,

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
    #[clap(long, value_name = "KEY=VALUE")]
    config: Vec<String>,

    /// Unstable (nightly-only) flags to Cargo, see 'cargo -Z help' for details
    #[clap(short = 'Z', value_name = "FLAG")]
    unstable_flags: Vec<String>,
}

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
