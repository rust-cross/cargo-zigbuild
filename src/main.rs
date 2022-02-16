use anyhow::Result;
use clap::Parser;

mod cli;
mod zig;

use cli::Opt;

fn main() -> Result<()> {
    let opt = Opt::parse();
    match opt {
        Opt::Build(build) => {
            println!("{:#?}", build);
        }
        Opt::Zig(zig) => zig.execute()?,
    }
    Ok(())
}
