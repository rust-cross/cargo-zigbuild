use std::env;
use std::ffi::OsStr;
#[cfg(target_family = "unix")]
use std::fs::OpenOptions;
#[cfg(target_family = "unix")]
use std::io::Write;
#[cfg(target_family = "unix")]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(target_os = "macos")]
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow, bail};
use fs_err as fs;
use path_slash::PathBufExt;
use target_lexicon::Triple;

use crate::linux::ARM_FEATURES_H;
use crate::macos::{LIBCHARSET_TBD, LIBICONV_TBD};

use super::locate::cache_dir;
use super::wrapper::{
    ZigWrapper, is_mingw_shell, prepare_zig_linker_with_cli_config, symlink_wrapper,
};
use super::{Zig, has_system_dlltool};

impl Zig {
    fn add_env_if_missing<K, V>(command: &mut Command, name: K, value: V)
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        let command_env_contains_no_key =
            |name: &K| !command.get_envs().any(|(key, _)| name.as_ref() == key);

        if command_env_contains_no_key(&name) && env::var_os(&name).is_none() {
            command.env(name, value);
        }
    }

    pub(crate) fn apply_command_env(
        manifest_path: Option<&Path>,
        release: bool,
        cargo: &cargo_options::CommonOptions,
        cmd: &mut Command,
        enable_zig_ar: bool,
    ) -> Result<()> {
        // setup zig as linker
        let cargo_config = cargo_config2::Config::load()?;
        // Use targets from CLI args, or fall back to cargo config's build.target
        let config_targets;
        let raw_targets: &[String] = if cargo.target.is_empty() {
            if let Some(targets) = &cargo_config.build.target {
                config_targets = targets
                    .iter()
                    .map(|t| t.triple().to_string())
                    .collect::<Vec<_>>();
                &config_targets
            } else {
                &cargo.target
            }
        } else {
            &cargo.target
        };
        #[cfg(target_os = "macos")]
        if !raw_targets.is_empty()
            && let Err(err) = crate::macos::rlimit::raise_nofile_limit()
        {
            eprintln!(
                "warning: failed to raise the open file limit: {err}; large builds may fail with ProcessFdQuotaExceeded (try `ulimit -n 65536`)"
            );
        }
        let rust_targets = raw_targets
            .iter()
            .map(|target| target.split_once('.').map(|(t, _)| t).unwrap_or(target))
            .collect::<Vec<&str>>();
        let rustc_meta = rustc_version::version_meta()?;
        Self::add_env_if_missing(
            cmd,
            "CARGO_ZIGBUILD_RUSTC_VERSION",
            rustc_meta.semver.to_string(),
        );
        let host_target = &rustc_meta.host;
        // This is an output, so any CARGO_ZIGBUILD_TARGET* inherited from an outer
        // cargo zigbuild is stale; drop it before exporting the ones for this build.
        for (key, _) in env::vars_os() {
            let mut name = key.to_string_lossy();
            // Windows environment variables are case-insensitive, Unix ones are not
            if cfg!(windows) {
                name = name.to_ascii_uppercase().into();
            }
            if name == "CARGO_ZIGBUILD_TARGET" || name.starts_with("CARGO_ZIGBUILD_TARGET_") {
                cmd.env_remove(&key);
            }
        }

        for (parsed_target, raw_target) in rust_targets.iter().zip(raw_targets) {
            let env_target = parsed_target.replace('-', "_");
            let zig_wrapper =
                prepare_zig_linker_with_cli_config(raw_target, &cargo_config, &cargo.config)?;

            // Export the resolved zig target for build scripts that do not go
            // through `cc` and so cannot recover the glibc version. Unlike the
            // variables around it this is an output, so a value inherited from an
            // outer build is stale rather than an override, and is replaced.
            // The unsuffixed name is single-target only, since one variable cannot
            // answer for several targets.
            cmd.env(
                format!("CARGO_ZIGBUILD_TARGET_{env_target}"),
                &zig_wrapper.target,
            );
            if raw_targets.len() == 1 {
                cmd.env("CARGO_ZIGBUILD_TARGET", &zig_wrapper.target);
            }

            if is_mingw_shell() {
                let zig_cc = zig_wrapper.cc.to_slash_lossy();
                let zig_cxx = zig_wrapper.cxx.to_slash_lossy();
                Self::add_env_if_missing(cmd, format!("CC_{env_target}"), &*zig_cc);
                Self::add_env_if_missing(cmd, format!("CXX_{env_target}"), &*zig_cxx);
                if !parsed_target.contains("wasm") {
                    Self::add_env_if_missing(
                        cmd,
                        format!("CARGO_TARGET_{}_LINKER", env_target.to_uppercase()),
                        &*zig_cc,
                    );
                }
            } else {
                Self::add_env_if_missing(cmd, format!("CC_{env_target}"), &zig_wrapper.cc);
                Self::add_env_if_missing(cmd, format!("CXX_{env_target}"), &zig_wrapper.cxx);
                if !parsed_target.contains("wasm") {
                    Self::add_env_if_missing(
                        cmd,
                        format!("CARGO_TARGET_{}_LINKER", env_target.to_uppercase()),
                        &zig_wrapper.cc,
                    );
                }
            }

            Self::add_env_if_missing(cmd, format!("RANLIB_{env_target}"), &zig_wrapper.ranlib);
            // Only setup AR when explicitly asked to
            // because it need special executable name handling, see src/bin/cargo-zigbuild.rs
            if enable_zig_ar {
                if parsed_target.contains("msvc") {
                    Self::add_env_if_missing(cmd, format!("AR_{env_target}"), &zig_wrapper.lib);
                } else {
                    Self::add_env_if_missing(cmd, format!("AR_{env_target}"), &zig_wrapper.ar);
                }
            }

            Self::setup_os_deps(manifest_path, release, cargo)?;

            let cmake_toolchain_file_env = format!("CMAKE_TOOLCHAIN_FILE_{env_target}");
            if env::var_os(&cmake_toolchain_file_env).is_none()
                && env::var_os(format!("CMAKE_TOOLCHAIN_FILE_{parsed_target}")).is_none()
                && env::var_os("TARGET_CMAKE_TOOLCHAIN_FILE").is_none()
                && env::var_os("CMAKE_TOOLCHAIN_FILE").is_none()
                && let Ok(cmake_toolchain_file) =
                    Self::setup_cmake_toolchain(parsed_target, &zig_wrapper, enable_zig_ar)
            {
                cmd.env(cmake_toolchain_file_env, cmake_toolchain_file);
            }

            // On Windows, cmake defaults to the Visual Studio generator which ignores
            // CMAKE_C_COMPILER from the toolchain file. Force Ninja to ensure zig cc
            // is used for cross-compilation.
            // See https://github.com/rust-cross/cargo-zigbuild/issues/174
            if cfg!(target_os = "windows")
                && env::var_os("CMAKE_GENERATOR").is_none()
                && which::which("ninja").is_ok()
            {
                cmd.env("CMAKE_GENERATOR", "Ninja");
            }

            if raw_target.contains("windows-gnu") {
                cmd.env("WINAPI_NO_BUNDLED_LIBRARIES", "1");
                // Add the cache directory to PATH so rustc can find architecture-specific dlltool
                // (e.g., x86_64-w64-mingw32-dlltool), but only if no system dlltool exists
                // If system mingw-w64 dlltool exists, prefer it over zig's dlltool
                let triple: Triple = parsed_target.parse().unwrap_or_else(|_| Triple::unknown());
                if !has_system_dlltool(&triple.architecture) {
                    // zig_wrapper.ar lives in the per-exe wrapper dir
                    let wrapper_dir = zig_wrapper.ar.parent().unwrap();
                    let existing_path = env::var_os("PATH").unwrap_or_default();
                    let paths = std::iter::once(wrapper_dir.to_path_buf())
                        .chain(env::split_paths(&existing_path));
                    if let Ok(new_path) = env::join_paths(paths) {
                        cmd.env("PATH", new_path);
                    }
                }
            }

            if raw_target.contains("apple-darwin")
                && let Some(sdkroot) = Self::macos_sdk_root()
                && env::var_os("PKG_CONFIG_SYSROOT_DIR").is_none()
            {
                // Set PKG_CONFIG_SYSROOT_DIR for pkg-config crate
                cmd.env("PKG_CONFIG_SYSROOT_DIR", sdkroot);
            }

            // Enable unstable `target-applies-to-host` option automatically
            // when target is the same as host but may have specified glibc version
            if host_target == parsed_target {
                if !matches!(rustc_meta.channel, rustc_version::Channel::Nightly) {
                    // Hack to use the unstable feature on stable Rust
                    // https://github.com/rust-lang/cargo/pull/9753#issuecomment-1022919343
                    cmd.env("__CARGO_TEST_CHANNEL_OVERRIDE_DO_NOT_USE_THIS", "nightly");
                }
                cmd.env("CARGO_UNSTABLE_TARGET_APPLIES_TO_HOST", "true");
                cmd.env("CARGO_TARGET_APPLIES_TO_HOST", "false");
            }

            // Pass options used by zig cc down to bindgen, if possible
            let mut options = Self::collect_zig_cc_options(&zig_wrapper, raw_target)
                .context("Failed to collect `zig cc` options")?;
            if raw_target.contains("apple-darwin") {
                // everyone seems to miss `#import <TargetConditionals.h>`...
                options.push("-DTARGET_OS_IPHONE=0".to_string());
            }
            let escaped_options = shell_words::join(options.iter().map(|s| &s[..]));
            let bindgen_env = "BINDGEN_EXTRA_CLANG_ARGS";
            let fallback_value = env::var(bindgen_env);
            for target in [&env_target[..], parsed_target] {
                let name = format!("{bindgen_env}_{target}");
                if let Ok(mut value) = env::var(&name).or(fallback_value.clone()) {
                    if shell_words::split(&value).is_err() {
                        // bindgen treats the whole string as a single argument if split fails
                        value = shell_words::quote(&value).into_owned();
                    }
                    if !value.is_empty() {
                        value.push(' ');
                    }
                    value.push_str(&escaped_options);
                    unsafe { env::set_var(name, value) };
                } else {
                    unsafe { env::set_var(name, escaped_options.clone()) };
                }
            }
        }
        Ok(())
    }

    /// Collects compiler options used by `zig cc` for given target.
    /// Used for the case where `zig cc` cannot be used but underlying options should be retained,
    /// for example, as in bindgen (which requires libclang.so and thus is independent from zig).
    fn collect_zig_cc_options(zig_wrapper: &ZigWrapper, raw_target: &str) -> Result<Vec<String>> {
        #[derive(Debug, PartialEq, Eq)]
        enum Kind {
            Normal,
            Framework,
        }

        #[derive(Debug)]
        struct PerLanguageOptions {
            glibc_minor_ver: Option<u32>,
            include_paths: Vec<(Kind, String)>,
        }

        fn collect_per_language_options(
            program: &Path,
            ext: &str,
            raw_target: &str,
        ) -> Result<PerLanguageOptions> {
            // We can't use `-x c` or `-x c++` because pre-0.11 Zig doesn't handle them
            let empty_file_path = cache_dir().join(format!(".intentionally-empty-file.{ext}"));
            if !empty_file_path.exists() {
                fs::write(&empty_file_path, "")?;
            }

            let output = Command::new(program)
                .arg("-E")
                .arg(&empty_file_path)
                .arg("-v")
                .output()?;
            // Clang always generates UTF-8 regardless of locale, so this is okay.
            let stderr = String::from_utf8(output.stderr)?;
            if !output.status.success() {
                bail!(
                    "Failed to run `zig cc -v` with status {}: {}",
                    output.status,
                    stderr.trim(),
                );
            }

            // Collect some macro definitions from cc1 options. We can't directly use
            // them though, as we can't distinguish options added by zig from options
            // added by clang driver (e.g. `__GCC_HAVE_DWARF2_CFI_ASM`).
            let glibc_minor_ver = if let Some(start) = stderr.find("__GLIBC_MINOR__=") {
                let stderr = &stderr[start + 16..];
                let end = stderr
                    .find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(stderr.len());
                stderr[..end].parse().ok()
            } else {
                None
            };

            let start = stderr
                .find("#include <...> search starts here:")
                .ok_or_else(|| anyhow!("Failed to parse `zig cc -v` output"))?
                + 34;
            let end = stderr
                .find("End of search list.")
                .ok_or_else(|| anyhow!("Failed to parse `zig cc -v` output"))?;

            let mut include_paths = Vec::new();
            for mut line in stderr[start..end].lines() {
                line = line.trim();
                let mut kind = Kind::Normal;
                if line.ends_with(" (framework directory)") {
                    line = line[..line.len() - 22].trim();
                    kind = Kind::Framework;
                } else if line.ends_with(" (headermap)") {
                    bail!("C/C++ search path includes header maps, which are not supported");
                }
                if !line.is_empty() {
                    include_paths.push((kind, line.to_owned()));
                }
            }

            // In openharmony, we should add search header path by default which is useful for bindgen.
            if raw_target.contains("ohos") {
                let ndk = env::var("OHOS_NDK_HOME").expect("Can't get NDK path");
                include_paths.push((Kind::Normal, format!("{}/native/sysroot/usr/include", ndk)));
            }

            Ok(PerLanguageOptions {
                include_paths,
                glibc_minor_ver,
            })
        }

        let c_opts = collect_per_language_options(&zig_wrapper.cc, "c", raw_target)?;
        let cpp_opts = collect_per_language_options(&zig_wrapper.cxx, "cpp", raw_target)?;

        // Ensure that `c_opts` and `cpp_opts` are almost identical in the way we expect.
        if c_opts.glibc_minor_ver != cpp_opts.glibc_minor_ver {
            bail!(
                "`zig cc` gives a different glibc minor version for C ({:?}) and C++ ({:?})",
                c_opts.glibc_minor_ver,
                cpp_opts.glibc_minor_ver,
            );
        }
        let c_paths = c_opts.include_paths;
        let mut cpp_paths = cpp_opts.include_paths;
        // The C++ search list is expected to be the C search list with extra
        // libc++ paths prepended and appended, but zig's layout has varied
        // across versions/targets, so fall back to zero-length pre/post
        // regions instead of panicking when a shared path can't be found.
        let cpp_pre_len = c_paths
            .iter()
            .find(|(kind, _)| *kind == Kind::Normal)
            .and_then(|first_c| cpp_paths.iter().position(|p| p == first_c))
            .unwrap_or_default();
        let cpp_post_len = c_paths
            .last()
            .and_then(|last_c| cpp_paths.iter().rposition(|p| p == last_c))
            .map(|pos| cpp_paths.len() - pos - 1)
            .unwrap_or_default();

        // <digression>
        //
        // So, why we do need all of these?
        //
        // Bindgen wouldn't look at our `zig cc` (which doesn't contain `libclang.so` anyway),
        // but it does collect include paths from the local clang and feed them to `libclang.so`.
        // We want those include paths to come from our `zig cc` instead of the local clang.
        // There are three main mechanisms possible:
        //
        // 1. Replace the local clang with our version.
        //
        //    Bindgen, internally via clang-sys, recognizes `CLANG_PATH` and `PATH`.
        //    They are unfortunately a global namespace and simply setting them may break
        //    existing build scripts, so we can't confidently override them.
        //
        //    Clang-sys can also look at target-prefixed clang if arguments contain `-target`.
        //    Unfortunately clang-sys can only recognize `-target xxx`, which very slightly
        //    differs from what bindgen would pass (`-target=xxx`), so this is not yet possible.
        //
        //    It should be also noted that we need to collect not only include paths
        //    but macro definitions added by Zig, for example `-D__GLIBC_MINOR__`.
        //    Clang-sys can't do this yet, so this option seems less robust than we want.
        //
        // 2. Set the environment variable `BINDGEN_EXTRA_CLANG_ARGS` and let bindgen to
        //    append them to arguments passed to `libclang.so`.
        //
        //    This unfortunately means that we have the same set of arguments for C and C++.
        //    Also we have to support older versions of clang, as old as clang 5 (2017).
        //    We do have options like `-c-isystem` (cc1 only) and `-cxx-isystem`,
        //    but we need to be aware of other options may affect our added options
        //    and this requires a nitty gritty of clang driver and cc1---really annoying.
        //
        // 3. Fix either bindgen or clang-sys or Zig to ease our jobs.
        //
        //    This is not the option for now because, even after fixes, we have to support
        //    older versions of bindgen or Zig which won't have those fixes anyway.
        //    But it seems that minor changes to bindgen can indeed fix lots of issues
        //    we face, so we are looking for them in the future.
        //
        // For this reason, we chose the option 2 and overrode `BINDGEN_EXTRA_CLANG_ARGS`.
        // The following therefore assumes some understanding about clang option handling,
        // including what the heck is cc1 (see the clang FAQ) and how driver options get
        // translated to cc1 options (no documentation at all, as it's supposedly unstable).
        // Fortunately for us, most (but not all) `-i...` options are passed through cc1.
        //
        // If you do experience weird compilation errors during bindgen, there's a chance
        // that this code has overlooked some edge cases. You can put `.clang_arg("-###")`
        // to print the final cc1 options, which would give a lot of information about
        // how it got screwed up and help a lot when we fix the issue.
        //
        // </digression>

        let mut args = Vec::new();

        // Never include default include directories,
        // otherwise `__has_include` will be totally confused.
        args.push("-nostdinc".to_owned());

        // Add various options for libc++ and glibc.
        // Should match what `Compilation.zig` internally does:
        //
        // https://github.com/ziglang/zig/blob/0.9.0/src/Compilation.zig#L3390-L3427
        // https://github.com/ziglang/zig/blob/0.9.1/src/Compilation.zig#L3408-L3445
        // https://github.com/ziglang/zig/blob/0.10.0/src/Compilation.zig#L4163-L4211
        // https://github.com/ziglang/zig/blob/0.10.1/src/Compilation.zig#L4240-L4288
        if raw_target.contains("musl") || raw_target.contains("ohos") {
            args.push("-D_LIBCPP_HAS_MUSL_LIBC".to_owned());
            // for musl or openharmony
            // https://github.com/ziglang/zig/pull/16098
            args.push("-D_LARGEFILE64_SOURCE".to_owned());
        }
        args.extend(
            [
                "-D_LIBCPP_DISABLE_VISIBILITY_ANNOTATIONS",
                "-D_LIBCPP_HAS_NO_VENDOR_AVAILABILITY_ANNOTATIONS",
                "-D_LIBCXXABI_DISABLE_VISIBILITY_ANNOTATIONS",
                "-D_LIBCPP_PSTL_CPU_BACKEND_SERIAL",
                "-D_LIBCPP_ABI_VERSION=1",
                "-D_LIBCPP_ABI_NAMESPACE=__1",
                "-D_LIBCPP_HARDENING_MODE=_LIBCPP_HARDENING_MODE_FAST",
                // Required by zig 0.15+ libc++ for streambuf and other I/O headers
                "-D_LIBCPP_HAS_LOCALIZATION=1",
                "-D_LIBCPP_HAS_WIDE_CHARACTERS=1",
                "-D_LIBCPP_HAS_UNICODE=1",
                "-D_LIBCPP_HAS_THREADS=1",
                "-D_LIBCPP_HAS_MONOTONIC_CLOCK",
                // Required by zig 0.17+ libc++ (LLVM 21); harmless no-ops on
                // older versions, which use the spellings above instead.
                // Should match `addCxxArgs` in zig's src/libs/libcxx.zig
                "-D_LIBCPP_ASSERTION_SEMANTIC_DEFAULT=_LIBCPP_ASSERTION_SEMANTIC_ENFORCE",
                "-D_LIBCPP_PSTL_BACKEND_SERIAL",
                "-D_LIBCPP_HAS_VENDOR_AVAILABILITY_ANNOTATIONS=0",
                "-D_LIBCPP_HAS_TERMINAL",
                "-D_LIBCPP_HAS_RANDOM_DEVICE",
                "-D_LIBCPP_HAS_NO_STD_MODULES",
            ]
            .into_iter()
            .map(ToString::to_string),
        );
        args.push(format!(
            "-D_LIBCPP_HAS_FILESYSTEM={}",
            if raw_target.contains("wasi") { 0 } else { 1 }
        ));
        if raw_target.contains("linux") {
            args.push("-D_LIBCPP_HAS_TIME_ZONE_DATABASE".to_owned());
        }
        if let Some(ver) = c_opts.glibc_minor_ver {
            // Handled separately because we have no way to infer this without Zig
            args.push(format!("-D__GLIBC_MINOR__={ver}"));
        }

        for (kind, path) in cpp_paths.drain(..cpp_pre_len) {
            if kind != Kind::Normal {
                // may also be Kind::Framework on macOS
                continue;
            }
            // Ideally this should be `-stdlib++-isystem`, which can be disabled by
            // passing `-nostdinc++`, but it is fairly new: https://reviews.llvm.org/D64089
            //
            // (Also note that `-stdlib++-isystem` is a driver-only option,
            // so it will be moved relative to other `-isystem` options against our will.)
            args.push("-cxx-isystem".to_owned());
            args.push(path);
        }

        for (kind, path) in c_paths {
            match kind {
                Kind::Normal => {
                    // A normal `-isystem` is preferred over `-cxx-isystem` by cc1...
                    args.push("-Xclang".to_owned());
                    args.push("-c-isystem".to_owned());
                    args.push("-Xclang".to_owned());
                    args.push(path.clone());
                    args.push("-cxx-isystem".to_owned());
                    args.push(path);
                }
                Kind::Framework => {
                    args.push("-iframework".to_owned());
                    args.push(path);
                }
            }
        }

        let post_start = cpp_paths.len().saturating_sub(cpp_post_len);
        for (kind, path) in cpp_paths.drain(post_start..) {
            if kind != Kind::Normal {
                // may also be Kind::Framework on macOS
                continue;
            }
            args.push("-cxx-isystem".to_owned());
            args.push(path);
        }

        Ok(args)
    }

    fn setup_os_deps(
        manifest_path: Option<&Path>,
        release: bool,
        cargo: &cargo_options::CommonOptions,
    ) -> Result<()> {
        for target in &cargo.target {
            if target.contains("apple") {
                let target_dir = if let Some(target_dir) = cargo.target_dir.clone() {
                    target_dir.join(target)
                } else {
                    let manifest_path = manifest_path.unwrap_or_else(|| Path::new("Cargo.toml"));
                    if !manifest_path.exists() {
                        // cargo install doesn't pass a manifest path so `Cargo.toml` in cwd may not exist
                        continue;
                    }
                    let metadata = cargo_metadata::MetadataCommand::new()
                        .manifest_path(manifest_path)
                        .no_deps()
                        .exec()?;
                    metadata.target_directory.into_std_path_buf().join(target)
                };
                let profile = match cargo.profile.as_deref() {
                    Some("dev" | "test") => "debug",
                    Some("release" | "bench") => "release",
                    Some(profile) => profile,
                    None => {
                        if release {
                            "release"
                        } else {
                            "debug"
                        }
                    }
                };
                let deps_dir = target_dir.join(profile).join("deps");
                fs::create_dir_all(&deps_dir)?;
                if !target_dir.join("CACHEDIR.TAG").is_file() {
                    // Create a CACHEDIR.TAG file to exclude target directory from backup
                    let _ = write_file(
                        &target_dir.join("CACHEDIR.TAG"),
                        "Signature: 8a477f597d28d172789f06886806bc55
# This file is a cache directory tag created by cargo.
# For information about cache directory tags see https://bford.info/cachedir/
",
                    );
                }
                write_tbd_files(&deps_dir)?;
            } else if target.contains("arm") && target.contains("linux") {
                // See https://github.com/ziglang/zig/issues/3287
                if let Ok(lib_dir) = Zig::lib_dir() {
                    let arm_features_h = lib_dir
                        .join("libc")
                        .join("glibc")
                        .join("sysdeps")
                        .join("arm")
                        .join("arm-features.h");
                    if !arm_features_h.is_file() {
                        fs::write(arm_features_h, ARM_FEATURES_H)?;
                    }
                }
            } else if target.contains("windows-gnu")
                && let Ok(lib_dir) = Zig::lib_dir()
            {
                let lib_common = lib_dir.join("libc").join("mingw").join("lib-common");
                let synchronization_def = lib_common.join("synchronization.def");
                if !synchronization_def.is_file() {
                    let api_ms_win_core_synch_l1_2_0_def =
                        lib_common.join("api-ms-win-core-synch-l1-2-0.def");
                    // Ignore error
                    fs::copy(api_ms_win_core_synch_l1_2_0_def, synchronization_def).ok();
                }
            }
        }
        Ok(())
    }

    fn setup_cmake_toolchain(
        target: &str,
        zig_wrapper: &ZigWrapper,
        enable_zig_ar: bool,
    ) -> Result<PathBuf> {
        // Place cmake toolchain files alongside the other wrappers in the
        // per-exe directory to avoid races between parallel builds.
        let wrapper_dir = zig_wrapper.cc.parent().unwrap();
        let cmake = wrapper_dir.join("cmake");
        fs::create_dir_all(&cmake)?;

        let toolchain_file = cmake.join(format!("{target}-toolchain.cmake"));
        let triple: Triple = target.parse()?;
        let os = triple.operating_system.to_string();
        let arch = triple.architecture.to_string();
        let (system_name, system_processor) = match (os.as_str(), arch.as_str()) {
            ("darwin", "x86_64") => ("Darwin", "x86_64"),
            ("darwin", "aarch64") => ("Darwin", "arm64"),
            ("linux", arch) => {
                let cmake_arch = match arch {
                    "powerpc" => "ppc",
                    "powerpc64" => "ppc64",
                    "powerpc64le" => "ppc64le",
                    _ => arch,
                };
                ("Linux", cmake_arch)
            }
            ("windows", "x86_64") => ("Windows", "AMD64"),
            ("windows", "i686") => ("Windows", "X86"),
            ("windows", "aarch64") => ("Windows", "ARM64"),
            (os, arch) => (os, arch),
        };
        let mut content = format!(
            r#"
set(CMAKE_SYSTEM_NAME {system_name})
set(CMAKE_SYSTEM_PROCESSOR {system_processor})
set(CMAKE_C_COMPILER {cc})
set(CMAKE_CXX_COMPILER {cxx})
set(CMAKE_RANLIB {ranlib})
set(CMAKE_C_LINKER_DEPFILE_SUPPORTED FALSE)
set(CMAKE_CXX_LINKER_DEPFILE_SUPPORTED FALSE)"#,
            system_name = system_name,
            system_processor = system_processor,
            cc = zig_wrapper.cc.to_slash_lossy(),
            cxx = zig_wrapper.cxx.to_slash_lossy(),
            ranlib = zig_wrapper.ranlib.to_slash_lossy(),
        );
        if enable_zig_ar {
            content.push_str(&format!(
                "\nset(CMAKE_AR {})\n",
                zig_wrapper.ar.to_slash_lossy()
            ));
        }
        // When cross-compiling to Darwin from a non-macOS host, CMake requires
        // install_name_tool and otool which don't exist on Linux/Windows.
        // Provide our own install_name_tool implementation via symlink wrapper,
        // and a no-op script for otool (not needed for builds) if no system otool exists.
        if system_name == "Darwin" && !cfg!(target_os = "macos") {
            let exe_ext = if cfg!(windows) { ".exe" } else { "" };
            let install_name_tool = wrapper_dir.join(format!("install_name_tool{exe_ext}"));
            symlink_wrapper(&install_name_tool)?;
            content.push_str(&format!(
                "\nset(CMAKE_INSTALL_NAME_TOOL {})",
                install_name_tool.to_slash_lossy()
            ));

            if which::which("otool").is_err() {
                let script_ext = if cfg!(windows) { "bat" } else { "sh" };
                let otool = cmake.join(format!("otool.{script_ext}"));
                write_noop_script(&otool)?;
                content.push_str(&format!("\nset(CMAKE_OTOOL {})", otool.to_slash_lossy()));
            }
        }
        // Prevent cmake from searching the host system's include and library paths,
        // which can conflict with zig's bundled headers (e.g. __COLD in sys/cdefs.h).
        // See https://github.com/rust-cross/cargo-zigbuild/issues/268
        content.push_str(
            r#"
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)"#,
        );
        write_file(&toolchain_file, &content)?;
        Ok(toolchain_file)
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn macos_sdk_root() -> Option<PathBuf> {
        static SDK_ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();

        SDK_ROOT
            .get_or_init(|| match env::var_os("SDKROOT") {
                Some(sdkroot) if !sdkroot.is_empty() => Some(sdkroot.into()),
                _ => {
                    let output = Command::new("xcrun")
                        .args(["--sdk", "macosx", "--show-sdk-path"])
                        .output()
                        .ok()?;
                    if output.status.success() {
                        let stdout = String::from_utf8(output.stdout).ok()?;
                        let stdout = stdout.trim();
                        if !stdout.is_empty() {
                            return Some(stdout.into());
                        }
                    }
                    None
                }
            })
            .clone()
    }

    #[cfg(not(target_os = "macos"))]
    pub(crate) fn macos_sdk_root() -> Option<PathBuf> {
        match env::var_os("SDKROOT") {
            Some(sdkroot) if !sdkroot.is_empty() => Some(sdkroot.into()),
            _ => None,
        }
    }
}

pub(crate) fn write_file(path: &Path, content: &str) -> Result<(), anyhow::Error> {
    let existing_content = fs::read_to_string(path).unwrap_or_default();
    if existing_content != content {
        fs::write(path, content)?;
    }
    Ok(())
}

/// Write a no-op shell/batch script for use as a placeholder tool.
/// Used for macOS-specific tools (install_name_tool, otool) when cross-compiling
/// to Darwin from non-macOS hosts.
#[cfg(target_family = "unix")]
fn write_noop_script(path: &Path) -> Result<()> {
    let content = "#!/bin/sh\nexit 0\n";
    let existing = fs::read_to_string(path).unwrap_or_default();
    if existing != content {
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o700)
            .open(path)?
            .write_all(content.as_bytes())?;
    }
    Ok(())
}

#[cfg(not(target_family = "unix"))]
fn write_noop_script(path: &Path) -> Result<()> {
    let content = "@echo off\r\nexit /b 0\r\n";
    let existing = fs::read_to_string(path).unwrap_or_default();
    if existing != content {
        fs::write(path, content)?;
    }
    Ok(())
}

pub(crate) fn write_tbd_files(deps_dir: &Path) -> Result<(), anyhow::Error> {
    write_file(&deps_dir.join("libiconv.tbd"), LIBICONV_TBD)?;
    write_file(&deps_dir.join("libcharset.1.tbd"), LIBCHARSET_TBD)?;
    write_file(&deps_dir.join("libcharset.tbd"), LIBCHARSET_TBD)?;
    Ok(())
}
