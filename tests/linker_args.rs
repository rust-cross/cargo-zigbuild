//! Exercise both production argument paths without requiring an installed Zig.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

fn filter_args(
    args: &[String],
    zig_version: &str,
    response_range: Option<std::ops::Range<usize>>,
    target: &str,
) -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    let zig = dir.path().join("zig");
    let captured = dir.path().join("captured");
    std::fs::write(
        &zig,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$CAPTURED_ARGS\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&zig, std::fs::Permissions::from_mode(0o755)).unwrap();

    let response = dir.path().join("linker-arguments");
    let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-zigbuild"));
    command.args(["zig", "cc", "--", "-target", target]);
    if let Some(ref range) = response_range {
        command.args(&args[..range.start]);
        let content = format!("{}\n", args[range.clone()].join("\n"));
        let bytes = if target.ends_with("windows-msvc") {
            std::iter::once(0xFEFF)
                .chain(content.encode_utf16())
                .flat_map(u16::to_le_bytes)
                .collect()
        } else {
            content.into_bytes()
        };
        std::fs::write(&response, bytes).unwrap();
        command.arg(format!("@{}", response.display()));
        command.args(&args[range.end..]);
    } else {
        command.args(args);
    }
    let output = command
        .env("CARGO_ZIGBUILD_ZIG_COMMAND", &zig)
        .env("CARGO_ZIGBUILD_ZIG_COMMAND_ARGS", "")
        .env("CARGO_ZIGBUILD_ZIG_VERSION", zig_version)
        .env("CARGO_ZIGBUILD_RUSTC_VERSION", "1.96.0")
        .env("CAPTURED_ARGS", &captured)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response_args: Vec<String> = if response_range.is_some() {
        let bytes = std::fs::read(&response).unwrap();
        let content = if target.ends_with("windows-msvc") {
            assert_eq!(&bytes[..2], &[255, 254]);
            let utf16: Vec<_> = bytes[2..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            String::from_utf16(&utf16).unwrap()
        } else {
            String::from_utf8(bytes).unwrap()
        };
        content.lines().map(str::to_owned).collect()
    } else {
        vec![]
    };
    // Expand the response file exactly where Zig would receive it.
    std::fs::read_to_string(captured)
        .unwrap()
        .lines()
        .skip(3) // `cc -target <target>` supplied by this helper
        .flat_map(|arg| {
            if arg == format!("@{}", response.display()) {
                response_args.clone()
            } else {
                vec![arg.to_owned()]
            }
        })
        .collect()
}

#[test]
fn list_operands_and_whole_archive_in_both_argument_paths() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("export list");
    let archive = dir.path().join("libnative.a");
    std::fs::write(&list, "_hello\n").unwrap();
    std::fs::write(&archive, "").unwrap();
    for response_file in [false, true] {
        for (version, keep_list) in [
            ("0.11.0", false),
            ("0.15.2", false),
            ("0.16.0", true),
            ("0.17.0", true),
        ] {
            for flag in ["-exported_symbols_list", "--dynamic-list"] {
                for combined in [false, true] {
                    let mut args = if combined {
                        vec![format!("-Wl,{flag},{}", list.display())]
                    } else {
                        vec![format!("-Wl,{flag}"), format!("-Wl,{}", list.display())]
                    };
                    let mut expected = if keep_list { args.clone() } else { vec![] };
                    args.extend([
                        "-Wl,--whole-archive".to_owned(),
                        format!("-Wl,{}", archive.display()),
                        "-Wl,--no-whole-archive".to_owned(),
                        "-Wl,-dead_strip".to_owned(),
                    ]);
                    expected.extend([
                        "-Wl,--whole-archive".to_owned(),
                        archive.display().to_string(),
                        "-Wl,--no-whole-archive".to_owned(),
                        "-Wl,-dead_strip".to_owned(),
                    ]);
                    for target in [
                        "aarch64-apple-darwin",
                        "x86_64-unknown-linux-gnu",
                        "x86_64-pc-windows-msvc",
                    ] {
                        assert_eq!(
                            filter_args(
                                &args,
                                version,
                                response_file.then_some(0..args.len()),
                                target
                            ),
                            expected,
                            "version={version}, flag={flag}, combined={combined}, response_file={response_file}, target={target}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn list_operand_preservation_ends_after_one_argument() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("list");
    std::fs::write(&list, "_hello\n").unwrap();
    for response_file in [false, true] {
        for operand in [
            vec![list.display().to_string()],
            vec!["-Xlinker".to_owned(), list.display().to_string()],
        ] {
            let mut args = vec!["-Wl,-exported_symbols_list".to_owned()];
            args.extend(operand);
            let mut expected = args.clone();
            args.extend([
                "--target=ignored".to_owned(),
                "-lgcc_s".to_owned(),
                "-lSystem".to_owned(),
                "-lSystem".to_owned(),
            ]);
            expected.extend(["-lunwind".to_owned(), "-lSystem".to_owned()]);
            assert_eq!(
                filter_args(
                    &args,
                    "0.16.0",
                    response_file.then_some(0..args.len()),
                    "aarch64-apple-darwin"
                ),
                expected
            );
        }
        // A missing operand must not panic or invent a filename.
        let args = vec!["-Wl,-exported_symbols_list".to_owned()];
        assert_eq!(
            filter_args(
                &args,
                "0.16.0",
                response_file.then_some(0..args.len()),
                "aarch64-apple-darwin"
            ),
            args
        );
    }
}

#[test]
fn list_pair_crosses_response_file_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("list");
    let archive = dir.path().join("libnative.a");
    std::fs::write(&list, "_hello\n").unwrap();
    std::fs::write(&archive, "").unwrap();
    let args = vec![
        "-Wl,-exported_symbols_list".to_owned(),
        format!("-Wl,{}", list.display()),
        "-Wl,--whole-archive".to_owned(),
        format!("-Wl,{}", archive.display()),
        "-Wl,--no-whole-archive".to_owned(),
    ];
    for range in [0..1, 1..args.len()] {
        for (version, keep_list) in [("0.15.2", false), ("0.16.0", true)] {
            let mut expected = args.clone();
            expected[3] = archive.display().to_string();
            if !keep_list {
                expected.drain(..2);
            }
            assert_eq!(
                filter_args(&args, version, Some(range.clone()), "aarch64-apple-darwin"),
                expected,
                "version={version}, range={range:?}"
            );
        }
    }
}
