use std::env;
use std::path::PathBuf;

use anyhow::Context;
use cargo_options::Metadata;
use cargo_zigbuild::{Build, Run, Rustc, Test, Zig};
use clap::Parser;

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Parser)]
#[clap(
    version,
    name = "cargo-zigbuild",
    global_setting(clap::AppSettings::DeriveDisplayOrder)
)]
pub enum Opt {
    #[clap(name = "zigbuild", aliases = &["build", "b"] )]
    Build(Build),
    #[clap(name = "metadata")]
    Metadata(Metadata),
    #[clap(name = "rustc")]
    Rustc(Rustc),
    #[clap(name = "run", alias = "r")]
    Run(Run),
    #[clap(name = "test", alias = "t")]
    Test(Test),
    #[clap(subcommand)]
    Zig(Zig),
}

fn main() -> anyhow::Result<()> {
    let mut args = env::args();
    let program_path = PathBuf::from(args.next().expect("no program path"));
    let program_name = program_path.file_stem().expect("no program name");
    if program_name.eq_ignore_ascii_case("ar") {
        let zig = Zig::Ar {
            args: args.collect(),
        };
        zig.execute()?;
    } else {
        let opt = Opt::parse();
        match opt {
            Opt::Build(mut build) => {
                build.enable_zig_ar = true;
                build.execute()?
            }
            Opt::Metadata(metadata) => {
                let mut cmd = metadata.command();
                let mut child = cmd.spawn().context("Failed to run cargo metadata")?;
                let status = child
                    .wait()
                    .expect("Failed to wait on cargo metadata process");
                if !status.success() {
                    std::process::exit(status.code().unwrap_or(1));
                }
            }
            Opt::Rustc(mut rustc) => {
                rustc.enable_zig_ar = true;
                rustc.execute()?
            }
            Opt::Run(mut run) => {
                run.enable_zig_ar = true;
                run.execute()?
            }
            Opt::Test(mut test) => {
                test.enable_zig_ar = true;
                test.execute()?
            }
            Opt::Zig(zig) => zig.execute()?,
        }
    }
    Ok(())
}
