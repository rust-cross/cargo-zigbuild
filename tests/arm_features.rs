//! ARM setup must not modify modern Zig installations.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn write_script(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn check_arm_setup(zig_version: &str, legacy: bool, existing_header: bool) {
    let dir = tempfile::tempdir().unwrap();
    let lib_dir = dir.path().join("zig lib");
    let arm_dir = lib_dir.join("libc/glibc/sysdeps/arm");
    fs::create_dir_all(&arm_dir).unwrap();
    let header = arm_dir.join("arm-features.h");
    if existing_header {
        fs::write(&header, "existing header\n").unwrap();
    }
    if !legacy {
        fs::set_permissions(&arm_dir, fs::Permissions::from_mode(0o555)).unwrap();
    }

    let zig = dir.path().join("zig");
    write_script(
        &zig,
        r#"#!/bin/sh
case "$1" in
    env)
        touch "$ZIG_ENV_CALLED"
        printf '.{ .lib_dir = "%s", }\n' "$ZIG_TEST_LIB_DIR"
        ;;
    cc|c++)
        printf '#include <...> search starts here:\n /mock/include\nEnd of search list.\n' >&2
        ;;
    *) exit 1 ;;
esac
"#,
    );
    let cargo = dir.path().join("cargo");
    write_script(&cargo, "#!/bin/sh\ntouch \"$CARGO_CALLED\"\n");
    let cargo_called = dir.path().join("cargo-called");
    let zig_env_called = dir.path().join("zig-env-called");

    let output = Command::new(env!("CARGO_BIN_EXE_cargo-zigbuild"))
        .args(["zigbuild", "--target", "armv7-unknown-linux-gnueabihf.2.17"])
        .current_dir(dir.path())
        .env("CARGO", &cargo)
        .env("CARGO_CALLED", &cargo_called)
        .env("CARGO_ZIGBUILD_CACHE_DIR", dir.path().join("cache"))
        .env("CARGO_ZIGBUILD_ZIG_COMMAND", &zig)
        .env("CARGO_ZIGBUILD_ZIG_COMMAND_ARGS", "")
        .env("CARGO_ZIGBUILD_ZIG_VERSION", zig_version)
        .env("ZIG_TEST_LIB_DIR", &lib_dir)
        .env("ZIG_ENV_CALLED", &zig_env_called)
        .output()
        .unwrap();

    // Restore permissions before asserting so TempDir can clean up on failure.
    fs::set_permissions(&arm_dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        output.status.success(),
        "Zig {zig_version}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(cargo_called.exists(), "Cargo was not invoked");
    assert_eq!(zig_env_called.exists(), legacy, "Zig {zig_version}");
    if existing_header {
        assert_eq!(fs::read_to_string(header).unwrap(), "existing header\n");
    } else if legacy {
        assert_eq!(
            fs::read_to_string(header).unwrap(),
            cargo_zigbuild::linux::ARM_FEATURES_H
        );
    } else {
        // Also catches writes when tests run as root and ignore permissions.
        assert!(
            !header.exists(),
            "Zig {zig_version} installation was modified"
        );
    }
}

#[test]
fn modern_zig_does_not_patch_read_only_lib_dir() {
    for version in ["0.10.1", "0.11.0", "0.17.1", "0.18.0-dev.1", "1.0.0"] {
        check_arm_setup(version, false, false);
    }
}

#[test]
fn legacy_zig_still_gets_missing_arm_header() {
    for version in ["0.9.0", "0.10.0"] {
        check_arm_setup(version, true, false);
    }
}

#[test]
fn legacy_zig_preserves_existing_arm_header() {
    check_arm_setup("0.10.0", true, true);
}
