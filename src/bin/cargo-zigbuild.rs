use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

use cargo_zigbuild::{Build, Check, Clippy, Doc, Install, Run, Rustc, Test, Zig};
use clap::Parser;

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Parser)]
#[command(
    version,
    name = "cargo-zigbuild",
    display_order = 1,
    styles = cargo_options::styles(),
)]
pub enum Opt {
    #[command(name = "zigbuild", aliases = &["build", "b"] )]
    Build(Build),
    #[command(name = "clippy")]
    Clippy(Clippy),
    #[command(name = "check", aliases = &["c"])]
    Check(Check),
    #[command(name = "doc")]
    Doc(Doc),
    #[command(name = "install")]
    Install(Install),
    #[command(name = "rustc")]
    Rustc(Rustc),
    #[command(name = "run", alias = "r")]
    Run(Run),
    #[command(name = "test", alias = "t")]
    Test(Test),
    #[command(subcommand)]
    Zig(Zig),
    #[command(external_subcommand)]
    External(Vec<OsString>),
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
    } else if program_name.eq_ignore_ascii_case("lib") {
        let zig = Zig::Lib {
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
            Opt::Clippy(mut clippy) => {
                clippy.enable_zig_ar = true;
                clippy.execute()?
            }
            Opt::Check(mut check) => {
                check.enable_zig_ar = true;
                check.execute()?
            }
            Opt::Doc(mut doc) => {
                doc.enable_zig_ar = true;
                doc.execute()?
            }
            Opt::Install(mut install) => {
                install.enable_zig_ar = true;
                install.execute()?
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
            Opt::External(args) => {
                let mut child = Command::new(env::var_os("CARGO").unwrap_or("cargo".into()))
                    .args(args)
                    .env_remove("CARGO")
                    .spawn()?;
                let status = child.wait().expect("Failed to wait on cargo process");
                if !status.success() {
                    std::process::exit(status.code().unwrap_or(1));
                }
            }
        }
    }
    Ok(())
}
