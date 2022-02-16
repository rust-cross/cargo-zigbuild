use clap::Parser;

mod cli;
mod zig;

use cli::Opt;

fn main() -> anyhow::Result<()> {
    let opt = Opt::parse();
    match opt {
        Opt::Build(build) => {
            build.execute()?;
        }
        Opt::Zig(zig) => zig.execute()?,
    }
    Ok(())
}
