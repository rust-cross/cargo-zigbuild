//! Exercise Zig exit handling through the CLI without requiring Zig.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

fn zig_command(script: &str) -> (tempfile::TempDir, Command) {
    let dir = tempfile::tempdir().unwrap();
    let zig = dir.path().join("zig");
    std::fs::write(&zig, format!("#!/bin/sh\nulimit -c 0\n{script}\n")).unwrap();
    std::fs::set_permissions(&zig, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-zigbuild"));
    command
        .env("CARGO_ZIGBUILD_ZIG_COMMAND", &zig)
        .env("CARGO_ZIGBUILD_ZIG_COMMAND_ARGS", "")
        .env("CARGO_ZIGBUILD_ZIG_VERSION", "0.15.2")
        .env("CARGO_ZIGBUILD_RUSTC_VERSION", "1.88.0")
        .env("CARGO_ZIGBUILD_CACHE_DIR", dir.path().join("cache"))
        .current_dir(dir.path());
    (dir, command)
}

fn run_zig(tool: &str, script: &str) -> Output {
    let (_dir, mut command) = zig_command(script);
    command.args(["zig", tool, "--", "-v"]).output().unwrap()
}

#[test]
fn reports_signal_termination() {
    // Cover both execute_compiler and execute_tool, including the cc -v probe.
    for tool in ["cc", "c++", "ar", "ranlib", "lib", "dlltool"] {
        for (signal, number) in [("SEGV", 11), ("TERM", 15)] {
            let output = run_zig(tool, &format!("kill -{signal} $$"));
            let stderr = String::from_utf8(output.stderr).unwrap();
            assert_eq!(output.status.code(), Some(128 + number), "{tool}: {stderr}");
            assert!(stderr.contains(&format!("zig {tool}")), "{stderr}");
            assert!(stderr.contains(&format!("signal: {number}")), "{stderr}");
            assert!(stderr.contains(&format!("SIG{signal}")), "{stderr}");
        }
    }
}

#[test]
fn preserves_normal_exit_codes_and_output() {
    for tool in ["cc", "ar"] {
        // An ordinary exit 139 must not be mistaken for SIGSEGV.
        for code in [0, 1, 23, 139] {
            let output = run_zig(tool, &format!("echo stdout\necho stderr >&2\nexit {code}"));
            assert_eq!(output.status.code(), Some(code), "{tool}");
            assert_eq!(output.stdout, b"stdout\n");
            assert_eq!(output.stderr, b"stderr\n");
        }
    }
}

#[test]
fn includes_signal_in_compiler_probe_error() {
    let (dir, mut command) = zig_command("kill -SEGV $$");
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"signal-probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();

    // This fails during environment setup, before Cargo builds anything, so
    // neither Zig nor the target's Rust standard library needs to be installed.
    let output = command
        .args([
            "zigbuild",
            "--offline",
            "--target",
            "x86_64-unknown-linux-musl",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("Failed to run `zig cc -v` with status exit status: 139"),
        "{stderr}"
    );
    assert!(stderr.contains("zig cc failed: signal: 11"), "{stderr}");
    assert!(stderr.contains("SIGSEGV"), "{stderr}");
}
