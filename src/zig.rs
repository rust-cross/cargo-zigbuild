use std::env;
use std::ffi::OsStr;
#[cfg(target_family = "unix")]
use std::fs::OpenOptions;
use std::io::Write;
#[cfg(target_family = "unix")]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::str;

use anyhow::{anyhow, bail, Context, Result};
use fs_err as fs;
use path_slash::PathBufExt;
use serde::Deserialize;
use target_lexicon::{Architecture, Environment, OperatingSystem, Triple};

use crate::linux::ARM_FEATURES_H;
use crate::macos::{LIBCHARSET_TBD, LIBICONV_TBD};

/// Zig linker wrapper
#[derive(Clone, Debug, clap::Subcommand)]
pub enum Zig {
    /// `zig cc` wrapper
    #[command(name = "cc")]
    Cc {
        /// `zig cc` arguments
        #[arg(num_args = 1.., trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// `zig c++` wrapper
    #[command(name = "c++")]
    Cxx {
        /// `zig c++` arguments
        #[arg(num_args = 1.., trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// `zig ar` wrapper
    #[command(name = "ar")]
    Ar {
        /// `zig ar` arguments
        #[arg(num_args = 1.., trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// `zig ranlib` wrapper
    #[command(name = "ranlib")]
    Ranlib {
        /// `zig ranlib` arguments
        #[arg(num_args = 1.., trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// `zig lib` wrapper
    #[command(name = "lib")]
    Lib {
        /// `zig lib` arguments
        #[arg(num_args = 1.., trailing_var_arg = true)]
        args: Vec<String>,
    },
}

struct TargetInfo {
    target: Option<String>,
    is_musl: bool,
    is_windows_gnu: bool,
    is_windows_msvc: bool,
    is_arm: bool,
    is_i386: bool,
    is_riscv64: bool,
    is_mips32: bool,
    is_macos: bool,
    is_ohos: bool,
}

impl TargetInfo {
    fn new(target: Option<&String>) -> Self {
        Self {
            target: target.cloned(),
            is_musl: target.map(|x| x.contains("musl")).unwrap_or_default(),
            is_windows_gnu: target
                .map(|x| x.contains("windows-gnu"))
                .unwrap_or_default(),
            is_windows_msvc: target
                .map(|x| x.contains("windows-msvc"))
                .unwrap_or_default(),
            is_arm: target.map(|x| x.starts_with("arm")).unwrap_or_default(),
            is_i386: target.map(|x| x.starts_with("i386")).unwrap_or_default(),
            is_riscv64: target.map(|x| x.starts_with("riscv64")).unwrap_or_default(),
            is_mips32: target
                .map(|x| x.starts_with("mips") && !x.starts_with("mips64"))
                .unwrap_or_default(),
            is_macos: target.map(|x| x.contains("macos")).unwrap_or_default(),
            is_ohos: target.map(|x| x.contains("ohos")).unwrap_or_default(),
        }
    }
}

impl Zig {
    /// Execute the underlying zig command
    pub fn execute(&self) -> Result<()> {
        match self {
            Zig::Cc { args } => self.execute_compiler("cc", args),
            Zig::Cxx { args } => self.execute_compiler("c++", args),
            Zig::Ar { args } => self.execute_tool("ar", args),
            Zig::Ranlib { args } => self.execute_compiler("ranlib", args),
            Zig::Lib { args } => self.execute_compiler("lib", args),
        }
    }

    /// Execute zig cc/c++ command
    pub fn execute_compiler(&self, cmd: &str, cmd_args: &[String]) -> Result<()> {
        let target = cmd_args
            .iter()
            .position(|x| x == "-target")
            .and_then(|index| cmd_args.get(index + 1));
        let target_info = TargetInfo::new(target);

        let rustc_ver = match env::var("CARGO_ZIGBUILD_RUSTC_VERSION") {
            Ok(version) => version.parse()?,
            Err(_) => rustc_version::version()?,
        };
        let zig_version = Zig::zig_version()?;

        let mut new_cmd_args = Vec::with_capacity(cmd_args.len());
        let mut skip_next_arg = false;
        for arg in cmd_args {
            if skip_next_arg {
                skip_next_arg = false;
                continue;
            }
            let args = if arg.starts_with('@') && arg.ends_with("linker-arguments") {
                vec![self.process_linker_response_file(
                    arg,
                    &rustc_ver,
                    &zig_version,
                    &target_info,
                )?]
            } else {
                self.filter_linker_arg(arg, &rustc_ver, &zig_version, &target_info)
            };
            for arg in args {
                if arg == "-Wl,-exported_symbols_list" {
                    // Filter out this and the next argument
                    skip_next_arg = true;
                } else {
                    new_cmd_args.push(arg);
                }
            }
        }

        if target_info.is_mips32 {
            // See https://github.com/ziglang/zig/issues/4925#issuecomment-1499823425
            new_cmd_args.push("-Wl,-z,notext".to_string());
        }

        if self.has_undefined_dynamic_lookup(cmd_args) {
            new_cmd_args.push("-Wl,-undefined=dynamic_lookup".to_string());
        }
        if target_info.is_macos {
            if self.should_add_libcharset(cmd_args, &zig_version) {
                new_cmd_args.push("-lcharset".to_string());
            }
            self.add_macos_specific_args(&mut new_cmd_args, &zig_version)?;
        }

        let mut child = Self::command()?
            .arg(cmd)
            .args(new_cmd_args)
            .spawn()
            .with_context(|| format!("Failed to run `zig {cmd}`"))?;
        let status = child.wait().expect("Failed to wait on zig child process");
        if !status.success() {
            process::exit(status.code().unwrap_or(1));
        }
        Ok(())
    }

    fn process_linker_response_file(
        &self,
        arg: &str,
        rustc_ver: &rustc_version::Version,
        zig_version: &semver::Version,
        target_info: &TargetInfo,
    ) -> Result<String> {
        // rustc passes arguments to linker via an @-file when arguments are too long
        // See https://github.com/rust-lang/rust/issues/41190
        // and https://github.com/rust-lang/rust/blob/87937d3b6c302dfedfa5c4b94d0a30985d46298d/compiler/rustc_codegen_ssa/src/back/link.rs#L1373-L1382
        let content_bytes = fs::read(arg.trim_start_matches('@'))?;
        let content = if target_info.is_windows_msvc {
            if content_bytes[0..2] != [255, 254] {
                bail!(
                    "linker response file `{}` didn't start with a utf16 BOM",
                    &arg
                );
            }
            let content_utf16: Vec<u16> = content_bytes[2..]
                .chunks_exact(2)
                .map(|a| u16::from_ne_bytes([a[0], a[1]]))
                .collect();
            String::from_utf16(&content_utf16).with_context(|| {
                format!(
                    "linker response file `{}` didn't contain valid utf16 content",
                    &arg
                )
            })?
        } else {
            String::from_utf8(content_bytes).with_context(|| {
                format!(
                    "linker response file `{}` didn't contain valid utf8 content",
                    &arg
                )
            })?
        };
        let mut link_args: Vec<_> = content
            .split('\n')
            .flat_map(|arg| self.filter_linker_arg(arg, &rustc_ver, &zig_version, &target_info))
            .collect();
        if self.has_undefined_dynamic_lookup(&link_args) {
            link_args.push("-Wl,-undefined=dynamic_lookup".to_string());
        }
        if target_info.is_macos && self.should_add_libcharset(&link_args, &zig_version) {
            link_args.push("-lcharset".to_string());
        }
        if target_info.is_windows_msvc {
            let new_content = link_args.join("\n");
            let mut out = Vec::with_capacity((1 + new_content.len()) * 2);
            // start the stream with a UTF-16 BOM
            for c in std::iter::once(0xFEFF).chain(new_content.encode_utf16()) {
                // encode in little endian
                out.push(c as u8);
                out.push((c >> 8) as u8);
            }
            fs::write(arg.trim_start_matches('@'), out)?;
        } else {
            fs::write(arg.trim_start_matches('@'), link_args.join("\n").as_bytes())?;
        }
        Ok(arg.to_string())
    }

    fn filter_linker_arg(
        &self,
        arg: &str,
        rustc_ver: &rustc_version::Version,
        zig_version: &semver::Version,
        target_info: &TargetInfo,
    ) -> Vec<String> {
        if arg == "-lgcc_s" {
            // Replace libgcc_s with libunwind
            return vec!["-lunwind".to_string()];
        } else if arg.starts_with("--target=") {
            // We have already passed target via `-target`
            return vec![];
        }
        if (target_info.is_arm || target_info.is_windows_gnu)
            && arg.ends_with(".rlib")
            && arg.contains("libcompiler_builtins-")
        {
            // compiler-builtins is duplicated with zig's compiler-rt
            return vec![];
        }
        if target_info.is_windows_gnu {
            #[allow(clippy::if_same_then_else)]
            if arg == "-lgcc_eh" {
                // zig doesn't provide gcc_eh alternative
                // We use libc++ to replace it on windows gnu targets
                return vec!["-lc++".to_string()];
            } else if arg == "-Wl,-Bdynamic" && (zig_version.major, zig_version.minor) >= (0, 11) {
                // https://github.com/ziglang/zig/pull/16058
                // zig changes the linker behavior, -Bdynamic won't search *.a for mingw, but this may be fixed in the later version
                // here is a workaround to replace the linker switch with -search_paths_first, which will search for *.dll,*lib first,
                // then fallback to *.a
                return vec!["-Wl,-search_paths_first".to_owned()];
            } else if arg == "-lwindows" || arg == "-l:libpthread.a" || arg == "-lgcc" {
                return vec![];
            } else if arg == "-Wl,--disable-auto-image-base"
                || arg == "-Wl,--dynamicbase"
                || arg == "-Wl,--large-address-aware"
                || (arg.starts_with("-Wl,")
                    && (arg.ends_with("/list.def") || arg.ends_with("\\list.def")))
            {
                // https://github.com/rust-lang/rust/blob/f0bc76ac41a0a832c9ee621e31aaf1f515d3d6a5/compiler/rustc_target/src/spec/windows_gnu_base.rs#L23
                // https://github.com/rust-lang/rust/blob/2fb0e8d162a021f8a795fb603f5d8c0017855160/compiler/rustc_target/src/spec/windows_gnu_base.rs#L22
                // https://github.com/rust-lang/rust/blob/f0bc76ac41a0a832c9ee621e31aaf1f515d3d6a5/compiler/rustc_target/src/spec/i686_pc_windows_gnu.rs#L16
                // zig doesn't support --disable-auto-image-base, --dynamicbase and --large-address-aware
                return vec![];
            } else if arg == "-lmsvcrt" {
                return vec![];
            }
        } else if arg == "-Wl,--no-undefined-version" {
            // https://github.com/rust-lang/rust/blob/542ed2bf72b232b245ece058fc11aebb1ca507d7/compiler/rustc_codegen_ssa/src/back/linker.rs#L723
            // zig doesn't support --no-undefined-version
            return vec![];
        } else if arg == "-Wl,-znostart-stop-gc" {
            // https://github.com/rust-lang/rust/blob/c580c498a1fe144d7c5b2dfc7faab1a229aa288b/compiler/rustc_codegen_ssa/src/back/link.rs#L3371
            // zig doesn't support -znostart-stop-gc
            return vec![];
        }
        if target_info.is_musl || target_info.is_ohos {
            // Avoids duplicated symbols with both zig musl libc and the libc crate
            if arg.ends_with(".o") && arg.contains("self-contained") && arg.contains("crt") {
                return vec![];
            } else if arg == "-Wl,-melf_i386" {
                // unsupported linker arg: -melf_i386
                return vec![];
            }
            if rustc_ver.major == 1
                && rustc_ver.minor < 59
                && arg.ends_with(".rlib")
                && arg.contains("liblibc-")
            {
                // Rust distributes standalone libc.a in self-contained for musl since 1.59.0
                // See https://github.com/rust-lang/rust/pull/90527
                return vec![];
            }
            if arg == "-lc" {
                return vec![];
            }
        }
        if arg.starts_with("-march=") {
            // Ignore `-march` option for arm* targets, we use `generic` + cpu features instead
            if target_info.is_arm || target_info.is_i386 {
                return vec![];
            } else if target_info.is_riscv64 {
                return vec!["-march=generic_rv64".to_string()];
            } else if arg.starts_with("-march=armv8-a") {
                let mut args_march = if target_info
                    .target
                    .as_ref()
                    .map(|x| x.starts_with("aarch64-macos"))
                    .unwrap_or_default()
                {
                    vec![arg.replace("armv8-a", "apple_m1")]
                } else if target_info
                    .target
                    .as_ref()
                    .map(|x| x.starts_with("aarch64-linux"))
                    .unwrap_or_default()
                {
                    vec![arg
                        .replace("armv8-a", "generic+v8a")
                        .replace("simd", "neon")]
                } else {
                    vec![arg.to_string()]
                };
                if arg == "-march=armv8-a+crypto" {
                    // Workaround for building sha1-asm on aarch64
                    // See:
                    // https://github.com/rust-cross/cargo-zigbuild/issues/149
                    // https://github.com/RustCrypto/asm-hashes/blob/master/sha1/build.rs#L17-L19
                    // https://github.com/ziglang/zig/issues/10411
                    args_march.append(&mut vec![
                        "-Xassembler".to_owned(),
                        "-march=armv8-a+crypto".to_owned(),
                    ]);
                }
                return args_march;
            }
        }
        if target_info.is_macos {
            if arg.starts_with("-Wl,-exported_symbols_list,") {
                // zig doesn't support -exported_symbols_list arg
                // https://clang.llvm.org/docs/ClangCommandLineReference.html#cmdoption-clang-exported_symbols_list
                return vec![];
            }
            if arg == "-Wl,-dylib" {
                // zig doesn't support -dylib
                return vec![];
            }
        }
        vec![arg.to_string()]
    }

    fn has_undefined_dynamic_lookup(&self, args: &[String]) -> bool {
        let undefined = args
            .iter()
            .position(|x| x == "-undefined")
            .and_then(|i| args.get(i + 1));
        matches!(undefined, Some(x) if x == "dynamic_lookup")
    }

    fn should_add_libcharset(&self, args: &[String], zig_version: &semver::Version) -> bool {
        // See https://github.com/apple-oss-distributions/libiconv/blob/a167071feb7a83a01b27ec8d238590c14eb6faff/xcodeconfig/libiconv.xcconfig
        if (zig_version.major, zig_version.minor) >= (0, 12) {
            args.iter().any(|x| x == "-liconv") && !args.iter().any(|x| x == "-lcharset")
        } else {
            false
        }
    }

    fn add_macos_specific_args(
        &self,
        new_cmd_args: &mut Vec<String>,
        zig_version: &semver::Version,
    ) -> Result<()> {
        let sdkroot = Self::macos_sdk_root();
        if (zig_version.major, zig_version.minor) >= (0, 12) {
            // Zig 0.12.0+ requires passing `--sysroot`
            if let Some(ref sdkroot) = sdkroot {
                new_cmd_args.push(format!("--sysroot={}", sdkroot.display()));
            }
        }
        if let Some(ref sdkroot) = sdkroot {
            let include_prefix = if (zig_version.major, zig_version.minor) < (0, 14) {
                sdkroot
            } else {
                Path::new("/")
            };
            new_cmd_args.extend_from_slice(&[
                "-isystem".to_string(),
                format!("{}", include_prefix.join("usr").join("include").display()),
                format!("-L{}", include_prefix.join("usr").join("lib").display()),
                format!(
                    "-F{}",
                    include_prefix
                        .join("System")
                        .join("Library")
                        .join("Frameworks")
                        .display()
                ),
                "-DTARGET_OS_IPHONE=0".to_string(),
            ]);
        }

        // Add the deps directory that contains `.tbd` files to the library search path
        let cache_dir = cache_dir();
        let deps_dir = cache_dir.join("deps");
        fs::create_dir_all(&deps_dir)?;
        write_tbd_files(&deps_dir)?;
        new_cmd_args.push("-L".to_string());
        new_cmd_args.push(format!("{}", deps_dir.display()));
        Ok(())
    }

    /// Execute zig ar/ranlib command
    pub fn execute_tool(&self, cmd: &str, cmd_args: &[String]) -> Result<()> {
        let mut child = Self::command()?
            .arg(cmd)
            .args(cmd_args)
            .spawn()
            .with_context(|| format!("Failed to run `zig {cmd}`"))?;
        let status = child.wait().expect("Failed to wait on zig child process");
        if !status.success() {
            process::exit(status.code().unwrap_or(1));
        }
        Ok(())
    }

    /// Build the zig command line
    pub fn command() -> Result<Command> {
        let (zig, zig_args) = Self::find_zig()?;
        let mut cmd = Command::new(zig);
        cmd.args(zig_args);
        Ok(cmd)
    }

    fn zig_version() -> Result<semver::Version> {
        let output = Self::command()?.arg("version").output()?;
        let version_str =
            str::from_utf8(&output.stdout).context("`zig version` didn't return utf8 output")?;
        let version = semver::Version::parse(version_str.trim())?;
        Ok(version)
    }

    /// Search for `python -m ziglang` first and for `zig` second.
    pub fn find_zig() -> Result<(PathBuf, Vec<String>)> {
        Self::find_zig_python()
            .or_else(|_| Self::find_zig_bin())
            .context("Failed to find zig")
    }

    /// Detect the plain zig binary
    fn find_zig_bin() -> Result<(PathBuf, Vec<String>)> {
        let zig_path = zig_path()?;
        let output = Command::new(&zig_path).arg("version").output()?;

        let version_str = str::from_utf8(&output.stdout).with_context(|| {
            format!("`{} version` didn't return utf8 output", zig_path.display())
        })?;
        Self::validate_zig_version(version_str)?;
        Ok((zig_path, Vec::new()))
    }

    /// Detect the Python ziglang package
    fn find_zig_python() -> Result<(PathBuf, Vec<String>)> {
        let python_path = python_path()?;
        let output = Command::new(&python_path)
            .args(["-m", "ziglang", "version"])
            .output()?;

        let version_str = str::from_utf8(&output.stdout).with_context(|| {
            format!(
                "`{} -m ziglang version` didn't return utf8 output",
                python_path.display()
            )
        })?;
        Self::validate_zig_version(version_str)?;
        Ok((python_path, vec!["-m".to_string(), "ziglang".to_string()]))
    }

    fn validate_zig_version(version: &str) -> Result<()> {
        let min_ver = semver::Version::new(0, 9, 0);
        let version = semver::Version::parse(version.trim())?;
        if version >= min_ver {
            Ok(())
        } else {
            bail!(
                "zig version {} is too old, need at least {}",
                version,
                min_ver
            )
        }
    }

    /// Find zig lib directory
    pub fn lib_dir() -> Result<PathBuf> {
        let (zig, zig_args) = Self::find_zig()?;
        let output = Command::new(zig).args(zig_args).arg("env").output()?;
        let zig_env: ZigEnv = serde_json::from_slice(&output.stdout)?;
        Ok(PathBuf::from(zig_env.lib_dir))
    }

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
        let rust_targets = cargo
            .target
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
        for (parsed_target, raw_target) in rust_targets.iter().zip(&cargo.target) {
            let env_target = parsed_target.replace('-', "_");
            let zig_wrapper = prepare_zig_linker(raw_target)?;

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
            {
                if let Ok(cmake_toolchain_file) =
                    Self::setup_cmake_toolchain(parsed_target, &zig_wrapper, enable_zig_ar)
                {
                    cmd.env(cmake_toolchain_file_env, cmake_toolchain_file);
                }
            }

            if raw_target.contains("windows-gnu") {
                cmd.env("WINAPI_NO_BUNDLED_LIBRARIES", "1");
            }

            if raw_target.contains("apple-darwin") {
                if let Some(sdkroot) = Self::macos_sdk_root() {
                    if env::var_os("PKG_CONFIG_SYSROOT_DIR").is_none() {
                        // Set PKG_CONFIG_SYSROOT_DIR for pkg-config crate
                        cmd.env("PKG_CONFIG_SYSROOT_DIR", sdkroot);
                    }
                }
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
            let escaped_options = shlex::try_join(options.iter().map(|s| &s[..]))?;
            let bindgen_env = "BINDGEN_EXTRA_CLANG_ARGS";
            let fallback_value = env::var(bindgen_env);
            for target in [&env_target[..], parsed_target] {
                let name = format!("{bindgen_env}_{target}");
                if let Ok(mut value) = env::var(&name).or(fallback_value.clone()) {
                    if shlex::split(&value).is_none() {
                        // bindgen treats the whole string as a single argument if split fails
                        value = shlex::try_quote(&value)?.into_owned();
                    }
                    if !value.is_empty() {
                        value.push(' ');
                    }
                    value.push_str(&escaped_options);
                    env::set_var(name, value);
                } else {
                    env::set_var(name, escaped_options.clone());
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
        let cpp_pre_len = cpp_paths
            .iter()
            .position(|p| {
                p == c_paths
                    .iter()
                    .filter(|(kind, _)| *kind == Kind::Normal)
                    .next()
                    .unwrap()
            })
            .unwrap_or_default();
        let cpp_post_len = cpp_paths.len()
            - cpp_paths
                .iter()
                .position(|p| p == c_paths.last().unwrap())
                .unwrap_or_default()
            - 1;

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
            ]
            .into_iter()
            .map(ToString::to_string),
        );
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

        for (kind, path) in cpp_paths.drain(cpp_paths.len() - cpp_post_len..) {
            assert!(kind == Kind::Normal);
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
            } else if target.contains("windows-gnu") {
                if let Ok(lib_dir) = Zig::lib_dir() {
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
        }
        Ok(())
    }

    fn setup_cmake_toolchain(
        target: &str,
        zig_wrapper: &ZigWrapper,
        enable_zig_ar: bool,
    ) -> Result<PathBuf> {
        let cmake = cache_dir().join("cmake");
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
        write_file(&toolchain_file, &content)?;
        Ok(toolchain_file)
    }

    #[cfg(target_os = "macos")]
    fn macos_sdk_root() -> Option<PathBuf> {
        match env::var_os("SDKROOT") {
            Some(sdkroot) => {
                if !sdkroot.is_empty() {
                    Some(sdkroot.into())
                } else {
                    None
                }
            }
            None => {
                let output = Command::new("xcrun")
                    .args(["--sdk", "macosx", "--show-sdk-path"])
                    .output();
                if let Ok(output) = output {
                    if output.status.success() {
                        if let Ok(stdout) = String::from_utf8(output.stdout) {
                            let stdout = stdout.trim();
                            if !stdout.is_empty() {
                                return Some(stdout.into());
                            }
                        }
                    }
                }
                None
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn macos_sdk_root() -> Option<PathBuf> {
        match env::var_os("SDKROOT") {
            Some(sdkroot) if !sdkroot.is_empty() => Some(sdkroot.into()),
            _ => None,
        }
    }
}

fn write_file(path: &Path, content: &str) -> Result<(), anyhow::Error> {
    let existing_content = fs::read_to_string(path).unwrap_or_default();
    if existing_content != content {
        fs::write(path, content)?;
    }
    Ok(())
}

fn write_tbd_files(deps_dir: &Path) -> Result<(), anyhow::Error> {
    write_file(&deps_dir.join("libiconv.tbd"), LIBICONV_TBD)?;
    write_file(&deps_dir.join("libcharset.1.tbd"), LIBCHARSET_TBD)?;
    write_file(&deps_dir.join("libcharset.tbd"), LIBCHARSET_TBD)?;
    Ok(())
}

fn cache_dir() -> PathBuf {
    env::var("CARGO_ZIGBUILD_CACHE_DIR")
        .ok()
        .map(|s| s.into())
        .or_else(dirs::cache_dir)
        // If the really is no cache dir, cwd will also do
        .unwrap_or_else(|| env::current_dir().expect("Failed to get current dir"))
        .join(env!("CARGO_PKG_NAME"))
        .join(env!("CARGO_PKG_VERSION"))
}

#[derive(Debug, Deserialize)]
struct ZigEnv {
    lib_dir: String,
}

/// zig wrapper paths
#[derive(Debug, Clone)]
pub struct ZigWrapper {
    pub cc: PathBuf,
    pub cxx: PathBuf,
    pub ar: PathBuf,
    pub ranlib: PathBuf,
    pub lib: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct TargetFlags {
    pub target_cpu: String,
    pub target_feature: String,
}

impl TargetFlags {
    pub fn parse_from_encoded(encoded: &OsStr) -> Result<Self> {
        let mut parsed = Self::default();

        let f = rustflags::from_encoded(encoded);
        for flag in f {
            if let rustflags::Flag::Codegen { opt, value } = flag {
                let key = opt.replace('-', "_");
                match key.as_str() {
                    "target_cpu" => {
                        if let Some(value) = value {
                            parsed.target_cpu = value;
                        }
                    }
                    "target_feature" => {
                        // See https://github.com/rust-lang/rust/blob/7e3ba5b8b7556073ab69822cc36b93d6e74cd8c9/compiler/rustc_session/src/options.rs#L1233
                        if let Some(value) = value {
                            if !parsed.target_feature.is_empty() {
                                parsed.target_feature.push(',');
                            }
                            parsed.target_feature.push_str(&value);
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(parsed)
    }
}

/// Prepare wrapper scripts for `zig cc` and `zig c++` and returns their paths
///
/// We want to use `zig cc` as linker and c compiler. We want to call `python -m ziglang cc`, but
/// cargo only accepts a path to an executable as linker, so we add a wrapper script. We then also
/// use the wrapper script to pass arguments and substitute an unsupported argument.
///
/// We create different files for different args because otherwise cargo might skip recompiling even
/// if the linker target changed
#[allow(clippy::blocks_in_conditions)]
pub fn prepare_zig_linker(target: &str) -> Result<ZigWrapper> {
    let (rust_target, abi_suffix) = target.split_once('.').unwrap_or((target, ""));
    let abi_suffix = if abi_suffix.is_empty() {
        String::new()
    } else {
        if abi_suffix
            .split_once('.')
            .filter(|(x, y)| {
                !x.is_empty()
                    && x.chars().all(|c| c.is_ascii_digit())
                    && !y.is_empty()
                    && y.chars().all(|c| c.is_ascii_digit())
            })
            .is_none()
        {
            bail!("Malformed zig target abi suffix.")
        }
        format!(".{abi_suffix}")
    };
    let triple: Triple = rust_target
        .parse()
        .with_context(|| format!("Unsupported Rust target '{rust_target}'"))?;
    let arch = triple.architecture.to_string();
    let target_env = match (triple.architecture, triple.environment) {
        (Architecture::Mips32(..), Environment::Gnu) => Environment::Gnueabihf,
        (Architecture::Powerpc, Environment::Gnu) => Environment::Gnueabihf,
        (_, Environment::GnuLlvm) => Environment::Gnu,
        (_, environment) => environment,
    };
    let file_ext = if cfg!(windows) { "bat" } else { "sh" };
    let file_target = target.trim_end_matches('.');

    let mut cc_args = vec![
        // prevent stripping
        "-g".to_owned(),
        // disable sanitizers
        "-fno-sanitize=all".to_owned(),
    ];

    // TODO: Maybe better to assign mcpu according to:
    // rustc --target <target> -Z unstable-options --print target-spec-json
    let zig_mcpu_default = match triple.operating_system {
        OperatingSystem::Linux => {
            match arch.as_str() {
                // zig uses _ instead of - in cpu features
                "arm" => match target_env {
                    Environment::Gnueabi | Environment::Musleabi => "generic+v6+strict_align",
                    Environment::Gnueabihf | Environment::Musleabihf => {
                        "generic+v6+strict_align+vfp2-d32"
                    }
                    _ => "",
                },
                "armv5te" => "generic+soft_float+strict_align",
                "armv7" => "generic+v7a+vfp3-d32+thumb2-neon",
                arch_str @ ("i586" | "i686") => {
                    if arch_str == "i586" {
                        "pentium"
                    } else {
                        "pentium4"
                    }
                }
                "riscv64gc" => "generic_rv64+m+a+f+d+c",
                "s390x" => "z10-vector",
                _ => "",
            }
        }
        _ => "",
    };

    // Override mcpu from RUSTFLAGS if provided. The override happens when
    // commands like `cargo-zigbuild build` are invoked.
    // Currently we only override according to target_cpu.
    let zig_mcpu_override = {
        let cargo_config = cargo_config2::Config::load()?;
        let rust_flags = cargo_config.rustflags(rust_target)?.unwrap_or_default();
        let encoded_rust_flags = rust_flags.encode()?;
        let target_flags = TargetFlags::parse_from_encoded(OsStr::new(&encoded_rust_flags))?;
        // Note: zig uses _ instead of - for target_cpu and target_feature
        // target_cpu may be empty string, which means target_cpu is not specified.
        target_flags.target_cpu.replace('-', "_")
    };

    if !zig_mcpu_override.is_empty() {
        cc_args.push(format!("-mcpu={zig_mcpu_override}"));
    } else if !zig_mcpu_default.is_empty() {
        cc_args.push(format!("-mcpu={zig_mcpu_default}"));
    }

    match triple.operating_system {
        OperatingSystem::Linux => {
            let zig_arch = match arch.as_str() {
                // zig uses _ instead of - in cpu features
                "arm" => "arm",
                "armv5te" => "arm",
                "armv7" => "arm",
                "i586" | "i686" => {
                    let zig_version = Zig::zig_version()?;
                    if zig_version.major == 0 && zig_version.minor >= 11 {
                        "x86"
                    } else {
                        "i386"
                    }
                }
                "riscv64gc" => "riscv64",
                "s390x" => "s390x",
                _ => arch.as_str(),
            };
            cc_args.push(format!("-target {zig_arch}-linux-{target_env}{abi_suffix}"));
        }
        OperatingSystem::MacOSX { .. } | OperatingSystem::Darwin(_) => {
            let zig_version = Zig::zig_version()?;
            // Zig 0.10.0 switched macOS ABI to none
            // see https://github.com/ziglang/zig/pull/11684
            if zig_version > semver::Version::new(0, 9, 1) {
                cc_args.push(format!("-target {arch}-macos-none{abi_suffix}"));
            } else {
                cc_args.push(format!("-target {arch}-macos-gnu{abi_suffix}"));
            }
        }
        OperatingSystem::Windows { .. } => {
            let zig_arch = match arch.as_str() {
                "i686" => {
                    let zig_version = Zig::zig_version()?;
                    if zig_version.major == 0 && zig_version.minor >= 11 {
                        "x86"
                    } else {
                        "i386"
                    }
                }
                arch => arch,
            };
            cc_args.push(format!(
                "-target {zig_arch}-windows-{target_env}{abi_suffix}"
            ));
        }
        OperatingSystem::Emscripten => {
            cc_args.push(format!("-target {arch}-emscripten{abi_suffix}"));
        }
        OperatingSystem::Wasi => {
            cc_args.push(format!("-target {arch}-wasi{abi_suffix}"));
        }
        OperatingSystem::WasiP1 => {
            cc_args.push(format!("-target {arch}-wasi.0.1.0{abi_suffix}"));
        }
        OperatingSystem::Unknown => {
            if triple.architecture == Architecture::Wasm32
                || triple.architecture == Architecture::Wasm64
            {
                cc_args.push(format!("-target {arch}-freestanding{abi_suffix}"));
            } else {
                bail!("unsupported target '{rust_target}'")
            }
        }
        _ => bail!(format!("unsupported target '{rust_target}'")),
    };

    let zig_linker_dir = cache_dir();
    fs::create_dir_all(&zig_linker_dir)?;

    if triple.operating_system == OperatingSystem::Linux {
        if matches!(
            triple.environment,
            Environment::Gnu
                | Environment::Gnuspe
                | Environment::Gnux32
                | Environment::Gnueabi
                | Environment::Gnuabi64
                | Environment::GnuIlp32
                | Environment::Gnueabihf
        ) {
            let glibc_version = if abi_suffix.is_empty() {
                (2, 17)
            } else {
                let mut parts = abi_suffix[1..].split('.');
                let major: usize = parts.next().unwrap().parse()?;
                let minor: usize = parts.next().unwrap().parse()?;
                (major, minor)
            };
            // See https://github.com/ziglang/zig/issues/9485
            if glibc_version < (2, 28) {
                use crate::linux::{FCNTL_H, FCNTL_MAP};

                let zig_version = Zig::zig_version()?;
                if zig_version.major == 0 && zig_version.minor < 11 {
                    let fcntl_map = zig_linker_dir.join("fcntl.map");
                    let existing_content = fs::read_to_string(&fcntl_map).unwrap_or_default();
                    if existing_content != FCNTL_MAP {
                        fs::write(&fcntl_map, FCNTL_MAP)?;
                    }
                    let fcntl_h = zig_linker_dir.join("fcntl.h");
                    let existing_content = fs::read_to_string(&fcntl_h).unwrap_or_default();
                    if existing_content != FCNTL_H {
                        fs::write(&fcntl_h, FCNTL_H)?;
                    }

                    cc_args.push(format!("-Wl,--version-script={}", fcntl_map.display()));
                    cc_args.push(format!("-include {}", fcntl_h.display()));
                }
            }
        } else if matches!(
            triple.environment,
            Environment::Musl
                | Environment::Muslabi64
                | Environment::Musleabi
                | Environment::Musleabihf
        ) {
            use crate::linux::MUSL_WEAK_SYMBOLS_MAPPING_SCRIPT;

            let zig_version = Zig::zig_version()?;
            let rustc_version = rustc_version::version_meta()?.semver;

            // as zig 0.11.0 is released, its musl has been upgraded to 1.2.4 with break changes
            // but rust is still with musl 1.2.3
            // we need this workaround before rust 1.72
            // https://github.com/ziglang/zig/pull/16098
            if (zig_version.major, zig_version.minor) >= (0, 11)
                && (rustc_version.major, rustc_version.minor) < (1, 72)
            {
                let weak_symbols_map = zig_linker_dir.join("musl_weak_symbols_map.ld");
                fs::write(&weak_symbols_map, MUSL_WEAK_SYMBOLS_MAPPING_SCRIPT)?;

                cc_args.push(format!("-Wl,-T,{}", weak_symbols_map.display()));
            }
        }
    }

    let cc_args_str = cc_args.join(" ");
    let hash = crc::Crc::<u16>::new(&crc::CRC_16_IBM_SDLC).checksum(cc_args_str.as_bytes());
    let zig_cc = zig_linker_dir.join(format!("zigcc-{file_target}-{:x}.{file_ext}", hash));
    let zig_cxx = zig_linker_dir.join(format!("zigcxx-{file_target}-{:x}.{file_ext}", hash));
    let zig_ranlib = zig_linker_dir.join(format!("zigranlib.{file_ext}"));
    write_linker_wrapper(&zig_cc, "cc", &cc_args_str)?;
    write_linker_wrapper(&zig_cxx, "c++", &cc_args_str)?;
    write_linker_wrapper(&zig_ranlib, "ranlib", "")?;

    let exe_ext = if cfg!(windows) { ".exe" } else { "" };
    let zig_ar = zig_linker_dir.join(format!("ar{exe_ext}"));
    symlink_wrapper(&zig_ar)?;
    let zig_lib = zig_linker_dir.join(format!("lib{exe_ext}"));
    symlink_wrapper(&zig_lib)?;

    Ok(ZigWrapper {
        cc: zig_cc,
        cxx: zig_cxx,
        ar: zig_ar,
        ranlib: zig_ranlib,
        lib: zig_lib,
    })
}

fn symlink_wrapper(target: &Path) -> Result<()> {
    let current_exe = if let Ok(exe) = env::var("CARGO_BIN_EXE_cargo-zigbuild") {
        PathBuf::from(exe)
    } else {
        env::current_exe()?
    };
    #[cfg(windows)]
    {
        if !target.exists() {
            // symlink on Windows requires admin privileges so we use hardlink instead
            if std::fs::hard_link(&current_exe, target).is_err() {
                // hard_link doesn't support cross-device links so we fallback to copy
                std::fs::copy(&current_exe, target)?;
            }
        }
    }

    #[cfg(unix)]
    {
        if !target.exists() {
            if fs::read_link(target).is_ok() {
                // remove broken symlink
                fs::remove_file(target)?;
            }
            std::os::unix::fs::symlink(current_exe, target)?;
        }
    }
    Ok(())
}

/// Write a zig cc wrapper batch script for unix
#[cfg(target_family = "unix")]
fn write_linker_wrapper(path: &Path, command: &str, args: &str) -> Result<()> {
    let mut buf = Vec::<u8>::new();
    let current_exe = if let Ok(exe) = env::var("CARGO_BIN_EXE_cargo-zigbuild") {
        PathBuf::from(exe)
    } else {
        env::current_exe()?
    };
    writeln!(&mut buf, "#!/bin/sh")?;
    writeln!(
        &mut buf,
        "exec \"{}\" zig {} -- {} \"$@\"",
        current_exe.display(),
        command,
        args
    )?;

    // Try not to write the file again if it's already the same.
    // This is more friendly for cache systems like ccache, which by default
    // uses mtime to determine if a recompilation is needed.
    let existing_content = fs::read(path).unwrap_or_default();
    if existing_content != buf {
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o700)
            .open(path)?
            .write_all(&buf)?;
    }
    Ok(())
}

/// Write a zig cc wrapper batch script for windows
#[cfg(not(target_family = "unix"))]
fn write_linker_wrapper(path: &Path, command: &str, args: &str) -> Result<()> {
    let mut buf = Vec::<u8>::new();
    let current_exe = if let Ok(exe) = env::var("CARGO_BIN_EXE_cargo-zigbuild") {
        PathBuf::from(exe)
    } else {
        env::current_exe()?
    };
    let current_exe = if is_mingw_shell() {
        current_exe.to_slash_lossy().to_string()
    } else {
        current_exe.display().to_string()
    };
    writeln!(
        &mut buf,
        "\"{}\" zig {} -- {} %*",
        adjust_canonicalization(current_exe),
        command,
        args
    )?;

    let existing_content = fs::read(path).unwrap_or_default();
    if existing_content != buf {
        fs::write(path, buf)?;
    }
    Ok(())
}

pub(crate) fn is_mingw_shell() -> bool {
    env::var_os("MSYSTEM").is_some() && env::var_os("SHELL").is_some()
}

// https://stackoverflow.com/a/50323079/3549270
#[cfg(target_os = "windows")]
pub fn adjust_canonicalization(p: String) -> String {
    const VERBATIM_PREFIX: &str = r#"\\?\"#;
    if p.starts_with(VERBATIM_PREFIX) {
        p[VERBATIM_PREFIX.len()..].to_string()
    } else {
        p
    }
}

fn python_path() -> Result<PathBuf> {
    let python = env::var("CARGO_ZIGBUILD_PYTHON_PATH").unwrap_or_else(|_| "python3".to_string());
    Ok(which::which(python)?)
}

fn zig_path() -> Result<PathBuf> {
    let zig = env::var("CARGO_ZIGBUILD_ZIG_PATH").unwrap_or_else(|_| "zig".to_string());
    Ok(which::which(zig)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_target_flags() {
        let cases = [
            // Input, TargetCPU, TargetFeature
            ("-C target-feature=-crt-static", "", "-crt-static"),
            ("-C target-cpu=native", "native", ""),
            (
                "--deny warnings --codegen target-feature=+crt-static",
                "",
                "+crt-static",
            ),
            ("-C target_cpu=skylake-avx512", "skylake-avx512", ""),
            ("-Ctarget_cpu=x86-64-v3", "x86-64-v3", ""),
            (
                "-C target-cpu=native --cfg foo -C target-feature=-avx512bf16,-avx512bitalg",
                "native",
                "-avx512bf16,-avx512bitalg",
            ),
            (
                "--target x86_64-unknown-linux-gnu --codegen=target-cpu=x --codegen=target-cpu=x86-64",
                "x86-64",
                "",
            ),
            (
                "-Ctarget-feature=+crt-static -Ctarget-feature=+avx",
                "",
                "+crt-static,+avx",
            ),
        ];

        for (input, expected_target_cpu, expected_target_feature) in cases.iter() {
            let args = cargo_config2::Flags::from_space_separated(input);
            let encoded_rust_flags = args.encode().unwrap();
            let flags = TargetFlags::parse_from_encoded(OsStr::new(&encoded_rust_flags)).unwrap();
            assert_eq!(flags.target_cpu, *expected_target_cpu, "{}", input);
            assert_eq!(flags.target_feature, *expected_target_feature, "{}", input);
        }
    }
}
