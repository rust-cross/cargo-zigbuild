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

fn cargo_global_arg_width(arg: &str) -> Option<usize> {
    let short_flags = arg
        .strip_prefix('-')
        .is_some_and(|flags| !flags.is_empty() && flags.chars().all(|c| matches!(c, 'q' | 'v')));
    let attached_value = arg.starts_with("--color=") || arg.starts_with("--config=");
    let attached_unstable = arg
        .strip_prefix("-Z")
        .is_some_and(|value| !value.is_empty());

    if matches!(arg, "--color" | "--config" | "-Z") {
        Some(2)
    } else if matches!(
        arg,
        "--quiet" | "--verbose" | "--locked" | "--offline" | "--frozen"
    ) || short_flags
        || attached_value
        || attached_unstable
    {
        Some(1)
    } else {
        None
    }
}

/// Cargo accepts global options on either side of its subcommand, while the
/// command-specific parsers used here accept them after the subcommand.
/// Normalize the former form into the latter before handing argv to clap.
fn normalize_global_cargo_args<I>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter().collect::<Vec<_>>();
    let mut index = 1;

    while let Some(arg) = args.get(index).and_then(|arg| arg.to_str()) {
        if let Some(width) = cargo_global_arg_width(arg) {
            index += width;
            continue;
        }
        if arg.starts_with('-') || index == 1 || arg == "zig" {
            return args;
        }

        let global_args = args.drain(1..index).collect::<Vec<_>>();
        args.splice(2..2, global_args);
        return args;
    }

    args
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
    } else if program_name.to_string_lossy().ends_with("dlltool") {
        let zig = Zig::Dlltool {
            args: args.collect(),
        };
        zig.execute()?;
    } else if program_name.eq_ignore_ascii_case("install_name_tool") {
        cargo_zigbuild::macos::install_name_tool::execute(args)?;
    } else {
        let opt = Opt::parse_from(normalize_global_cargo_args(env::args_os()));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(|value| OsString::from(*value)).collect()
    }

    #[test]
    fn normalize_cargo_global_options() {
        let cases: &[(&[&str], &[&str])] = &[
            (
                &["cargo-zigbuild", "--color=auto", "test", "--no-run"],
                &["cargo-zigbuild", "test", "--color=auto", "--no-run"],
            ),
            (
                &[
                    "cargo-zigbuild",
                    "--color",
                    "auto",
                    "--config",
                    "net.offline=true",
                    "-Z",
                    "unstable-options",
                    "-qv",
                    "check",
                ],
                &[
                    "cargo-zigbuild",
                    "check",
                    "--color",
                    "auto",
                    "--config",
                    "net.offline=true",
                    "-Z",
                    "unstable-options",
                    "-qv",
                ],
            ),
            (
                &[
                    "cargo-zigbuild",
                    "--offline",
                    "metadata",
                    "--format-version",
                    "1",
                ],
                &[
                    "cargo-zigbuild",
                    "metadata",
                    "--offline",
                    "--format-version",
                    "1",
                ],
            ),
            (
                &["cargo-zigbuild", "test", "--color=always"],
                &["cargo-zigbuild", "test", "--color=always"],
            ),
            (
                &[
                    "cargo-zigbuild",
                    "--target",
                    "aarch64-unknown-linux-gnu",
                    "build",
                ],
                &[
                    "cargo-zigbuild",
                    "--target",
                    "aarch64-unknown-linux-gnu",
                    "build",
                ],
            ),
            (
                &[
                    "cargo-zigbuild",
                    "--color=auto",
                    "zig",
                    "cc",
                    "--",
                    "hello.c",
                ],
                &[
                    "cargo-zigbuild",
                    "--color=auto",
                    "zig",
                    "cc",
                    "--",
                    "hello.c",
                ],
            ),
        ];

        for (input, expected) in cases {
            assert_eq!(
                normalize_global_cargo_args(args(input)),
                args(expected),
                "input: {input:?}"
            );
        }
    }

    #[test]
    fn clap_accepts_global_options_before_cargo_subcommand() {
        let normalized = normalize_global_cargo_args(args(&[
            "cargo-zigbuild",
            "--color=auto",
            "--offline",
            "test",
            "--no-run",
        ]));
        assert!(matches!(Opt::try_parse_from(normalized), Ok(Opt::Test(_))));
    }
}
