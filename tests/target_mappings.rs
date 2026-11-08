//! Verify the target-mapping CLI without requiring Zig or a Cargo project.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn list_target_mappings_uses_rustc_and_does_not_build() {
    let dir = tempfile::tempdir().unwrap();
    let fake_rustc = dir.path().join("rustc-target-list");
    fs::write(
        &fake_rustc,
        "#!/bin/sh\n[ \"$1\" = --print ] && [ \"$2\" = target-list ] || exit 5\nprintf '%s\\n' x86_64-unknown-redox x86_64-unknown-linux-gnu i686-pc-windows-gnu wasm32-wasip1\n",
    )
    .unwrap();
    fs::set_permissions(&fake_rustc, fs::Permissions::from_mode(0o755)).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_cargo-zigbuild"))
        .current_dir(dir.path()) // Intentionally no Cargo.toml.
        .args(["zigbuild", "--list-target-mappings"])
        .env("RUSTC", &fake_rustc)
        .env("CARGO_ZIGBUILD_ZIG_VERSION", "0.17.0")
        .env("CARGO_ZIGBUILD_ZIG_PATH", "/nonexistent/zig")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        concat!(
            "Rust target\tZig target (Zig 0.17.0)\n",
            "i686-pc-windows-gnu\tx86-windows-gnu\n",
            "wasm32-wasip1\twasm32-wasi.0.1.0\n",
            "x86_64-unknown-linux-gnu\tx86_64-linux-gnu\n",
        )
    );
}

#[test]
fn list_target_mappings_reports_rustc_failure() {
    let dir = tempfile::tempdir().unwrap();
    let fake_rustc = dir.path().join("rustc-fail");
    fs::write(&fake_rustc, "#!/bin/sh\necho failed-to-list >&2\nexit 37\n").unwrap();
    fs::set_permissions(&fake_rustc, fs::Permissions::from_mode(0o755)).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_cargo-zigbuild"))
        .current_dir(dir.path())
        .args(["zigbuild", "--list-target-mappings"])
        .env("RUSTC", &fake_rustc)
        .env("CARGO_ZIGBUILD_ZIG_VERSION", "0.17.0")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("rustc --print target-list failed"),
        "{stderr}"
    );
    assert!(stderr.contains("failed-to-list"), "{stderr}");
}
