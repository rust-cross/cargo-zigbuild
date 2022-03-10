use cargo_zigbuild::rustc::Rustc;
use cargo_zigbuild::{Build, Zig};
use clap::Parser;

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Parser)]
#[clap(
    version,
    name = "cargo",
    global_setting(clap::AppSettings::DeriveDisplayOrder)
)]
pub enum Opt {
    #[clap(name = "zigbuild", alias = "build")]
    Build(Build),
    #[clap(name = "rustc")]
    Rustc(Rustc),
    #[clap(subcommand)]
    Zig(Zig),
}

fn main() -> anyhow::Result<()> {
    let opt = Opt::parse();
    match opt {
        Opt::Build(build) => build.execute()?,
        Opt::Rustc(rustc) => rustc.execute()?,
        Opt::Zig(zig) => zig.execute()?,
    }
    Ok(())
}
