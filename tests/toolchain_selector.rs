//! Verify that direct `cargo-zigbuild +toolchain build` still uses the Zig-aware
//! build path, rather than forwarding a raw `cargo +toolchain build` invocation.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn direct_toolchain_selector_preserves_zigbuild_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let cargo = dir.path().join("fake-cargo");
    fs::write(
        &cargo,
        "#!/bin/sh\nprintf 'toolchain=%s\\n' \"$RUSTUP_TOOLCHAIN\"\nprintf 'arg=%s\\n' \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();

    // An ordinary build without --target should not need a Zig installation.
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-zigbuild"))
        .args(["+stable", "build", "--release"])
        .env("CARGO", &cargo)
        .env_remove("RUSTUP_TOOLCHAIN")
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let out = String::from_utf8(output.stdout).unwrap();
    assert!(out.contains("toolchain=stable\n"), "{out}");
    assert!(out.contains("arg=build\n"), "{out}");
    assert!(!out.contains("arg=+stable\n"), "{out}");
}
