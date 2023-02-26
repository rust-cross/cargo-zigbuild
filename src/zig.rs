use std::env;
#[cfg(target_family = "unix")]
use std::fs::OpenOptions;
use std::io::Write;
#[cfg(target_family = "unix")]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::str;

use anyhow::{bail, Context, Result};
use fs_err as fs;
use path_slash::PathBufExt;
use serde::Deserialize;
use target_lexicon::{Architecture, Environment, OperatingSystem, Triple};

use crate::linux::{ARM_FEATURES_H, FCNTL_H, FCNTL_MAP};
use crate::macos::LIBICONV_TBD;

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
}

impl Zig {
    /// Execute the underlying zig command
    pub fn execute(&self) -> Result<()> {
        match self {
            Zig::Cc { args } => self.execute_compiler("cc", args),
            Zig::Cxx { args } => self.execute_compiler("c++", args),
            Zig::Ar { args } => self.execute_tool("ar", args),
            Zig::Ranlib { args } => self.execute_compiler("ranlib", args),
        }
    }

    /// Execute zig cc/c++ command
    pub fn execute_compiler(&self, cmd: &str, cmd_args: &[String]) -> Result<()> {
        let target = cmd_args
            .iter()
            .position(|x| x == "-target")
            .and_then(|index| cmd_args.get(index + 1));
        let is_musl = target.map(|x| x.contains("musl")).unwrap_or_default();
        let is_windows_gnu = target
            .map(|x| x.contains("windows-gnu"))
            .unwrap_or_default();
        let is_windows_msvc = target
            .map(|x| x.contains("windows-msvc"))
            .unwrap_or_default();
        let is_arm = target.map(|x| x.starts_with("arm")).unwrap_or_default();
        let is_i386 = target.map(|x| x.starts_with("i386")).unwrap_or_default();
        let is_riscv64 = target.map(|x| x.starts_with("riscv64")).unwrap_or_default();

        let rustc_ver = rustc_version::version()?;

        let filter_linker_arg = |arg: &str| {
            if arg == "-lgcc_s" {
                // Replace libgcc_s with libunwind
                return Some("-lunwind".to_string());
            }
            if (is_arm || is_windows_gnu)
                && arg.ends_with(".rlib")
                && arg.contains("libcompiler_builtins-")
            {
                // compiler-builtins is duplicated with zig's compiler-rt
                return None;
            }
            if is_windows_gnu {
                #[allow(clippy::if_same_then_else)]
                if arg == "-lgcc_eh" {
                    // zig doesn't provide gcc_eh alternative
                    // We use libc++ to replace it on windows gnu targets
                    return Some("-lc++".to_string());
                } else if arg == "-lwindows" || arg == "-l:libpthread.a" || arg == "-lgcc" {
                    return None;
                } else if arg == "-Wl,--disable-auto-image-base" {
                    // https://github.com/rust-lang/rust/blob/f0bc76ac41a0a832c9ee621e31aaf1f515d3d6a5/compiler/rustc_target/src/spec/windows_gnu_base.rs#L23
                    // zig doesn't support --disable-auto-image-base
                    return None;
                }
            }
            if is_musl {
                // Avoids duplicated symbols with both zig musl libc and the libc crate
                if arg.ends_with(".o") && arg.contains("self-contained") && arg.contains("crt") {
                    return None;
                }
                if rustc_ver.major == 1
                    && rustc_ver.minor < 59
                    && arg.ends_with(".rlib")
                    && arg.contains("liblibc-")
                {
                    // Rust distributes standalone libc.a in self-contained for musl since 1.59.0
                    // See https://github.com/rust-lang/rust/pull/90527
                    return None;
                }
                if arg == "-lc" {
                    return None;
                }
            }
            // Ignore `-march` option for arm* targets, we use `generic` + cpu features instead
            if is_arm && arg.starts_with("-march=") {
                return None;
            }
            if is_i386 && arg.starts_with("-march=") {
                return None;
            }
            if is_riscv64 && arg.starts_with("-march=") {
                return Some("-march=generic_rv64".to_string());
            }
            Some(arg.to_string())
        };
        let has_undefined_dynamic_lookup = |args: &[String]| {
            let undefined = args
                .iter()
                .position(|x| x == "-undefined")
                .and_then(|i| args.get(i + 1));
            matches!(undefined, Some(x) if x == "dynamic_lookup")
        };

        let mut new_cmd_args = Vec::with_capacity(cmd_args.len());
        for arg in cmd_args {
            let arg = if arg.starts_with('@') && arg.ends_with("linker-arguments") {
                // rustc passes arguments to linker via an @-file when arguments are too long
                // See https://github.com/rust-lang/rust/issues/41190
                // and https://github.com/rust-lang/rust/blob/87937d3b6c302dfedfa5c4b94d0a30985d46298d/compiler/rustc_codegen_ssa/src/back/link.rs#L1373-L1382
                let content_bytes = fs::read(arg.trim_start_matches('@'))?;
                let content = if is_windows_msvc {
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
                let mut link_args: Vec<_> =
                    content.split('\n').filter_map(filter_linker_arg).collect();
                if has_undefined_dynamic_lookup(&link_args) {
                    link_args.push("-Wl,-undefined=dynamic_lookup".to_string());
                }
                if is_windows_msvc {
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
                Some(arg.to_string())
            } else {
                filter_linker_arg(arg)
            };
            if let Some(arg) = arg {
                new_cmd_args.push(arg);
            }
        }
        if has_undefined_dynamic_lookup(cmd_args) {
            new_cmd_args.push("-Wl,-undefined=dynamic_lookup".to_string());
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
        let host_target = &rustc_meta.host;
        for (parsed_target, raw_target) in rust_targets.iter().zip(&cargo.target) {
            let env_target = parsed_target.replace('-', "_");
            let zig_wrapper = prepare_zig_linker(raw_target)?;
            if is_mingw_shell() {
                let mut add_env = |name, value| {
                    if env::var_os(&name).is_none() {
                        cmd.env(name, value);
                    }
                };
                let zig_cc = zig_wrapper.cc.to_slash_lossy();
                let zig_cxx = zig_wrapper.cxx.to_slash_lossy();
                add_env(format!("CC_{env_target}"), &*zig_cc);
                add_env(format!("CXX_{env_target}"), &*zig_cxx);
                add_env(
                    format!("CARGO_TARGET_{}_LINKER", env_target.to_uppercase()),
                    &*zig_cc,
                );
            } else {
                let mut add_env = |name, value| {
                    if env::var_os(&name).is_none() {
                        cmd.env(name, value);
                    }
                };
                add_env(format!("CC_{env_target}"), &zig_wrapper.cc);
                add_env(format!("CXX_{env_target}"), &zig_wrapper.cxx);
                add_env(
                    format!("CARGO_TARGET_{}_LINKER", env_target.to_uppercase()),
                    &zig_wrapper.cc,
                );
            }

            let mut add_env = |name, value| {
                if env::var_os(&name).is_none() {
                    cmd.env(name, value);
                }
            };
            add_env(format!("RANLIB_{env_target}"), &zig_wrapper.ranlib);
            // Only setup AR when explicitly asked to
            // because it need special executable name handling, see src/bin/cargo-zigbuild.rs
            if enable_zig_ar {
                add_env(format!("AR_{env_target}"), &zig_wrapper.ar);
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

            // bindgen support
            if let Ok(lib_dir) = Zig::lib_dir() {
                let bindgen_env = format!("BINDGEN_EXTRA_CLANG_ARGS_{}", env_target);
                let libc = lib_dir.join("libc");
                if raw_target.contains("linux") {
                    if raw_target.contains("musl") {
                        cmd.env(
                            bindgen_env,
                            format!("--sysroot={}", libc.join("musl").display()),
                        );
                    } else if raw_target.contains("gnu") {
                        cmd.env(
                            bindgen_env,
                            format!("--sysroot={}", libc.join("glibc").display()),
                        );
                    }
                } else if raw_target.contains("windows-gnu") {
                    cmd.env(
                        bindgen_env,
                        format!("--sysroot={}", libc.join("mingw").display()),
                    );
                }
            }
        }
        Ok(())
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
                    let mut metadata_cmd = cargo_metadata::MetadataCommand::new();
                    metadata_cmd.manifest_path(manifest_path);
                    let metadata = metadata_cmd.exec()?;
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
                fs::write(deps_dir.join("libiconv.tbd"), LIBICONV_TBD)?;
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
set(CMAKE_RANLIB {ranlib})"#,
            system_name = system_name,
            system_processor = system_processor,
            cc = zig_wrapper.cc.display(),
            cxx = zig_wrapper.cxx.display(),
            ranlib = zig_wrapper.ranlib.display(),
        );
        if enable_zig_ar {
            content.push_str(&format!("\nset(CMAKE_AR {})\n", zig_wrapper.ar.display()));
        }
        fs::write(&toolchain_file, content)?;
        Ok(toolchain_file)
    }
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
}

/// Prepare wrapper scripts for `zig cc` and `zig c++` and returns their paths
///
/// We want to use `zig cc` as linker and c compiler. We want to call `python -m ziglang cc`, but
/// cargo only accepts a path to an executable as linker, so we add a wrapper script. We then also
/// use the wrapper script to pass arguments and substitute an unsupported argument.
///
/// We create different files for different args because otherwise cargo might skip recompiling even
/// if the linker target changed
#[allow(clippy::blocks_in_if_conditions)]
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
        (_, Environment::GnuLlvm) => Environment::Gnu,
        (_, environment) => environment,
    };
    let file_ext = if cfg!(windows) { "bat" } else { "sh" };
    let file_target = target.trim_end_matches('.');
    let zig_cc = format!("zigcc-{file_target}.{file_ext}");
    let zig_cxx = format!("zigcxx-{file_target}.{file_ext}");
    let cc_args = "-g"; // prevent stripping
    let mut cc_args = match triple.operating_system {
        OperatingSystem::Linux => {
            let (zig_arch, zig_cpu) = match arch.as_str() {
                // zig uses _ instead of - in cpu features
                "arm" => match target_env {
                    Environment::Gnueabi | Environment::Musleabi => {
                        ("arm", "-mcpu=generic+v6+strict_align")
                    }
                    Environment::Gnueabihf | Environment::Musleabihf => {
                        ("arm", "-mcpu=generic+v6+strict_align+vfp2-d32")
                    }
                    _ => ("arm", ""),
                },
                "armv5te" => ("arm", "-mcpu=generic+soft_float+strict_align"),
                "armv7" => ("arm", "-mcpu=generic+v7a+vfp3-d32+thumb2-neon"),
                "i586" => ("i386", "-mcpu=pentium"),
                "i686" => ("i386", "-mcpu=pentium4"),
                "riscv64gc" => ("riscv64", "-mcpu=generic_rv64+m+a+f+d+c"),
                "s390x" => ("s390x", "-mcpu=z10-vector"),
                _ => (arch.as_str(), ""),
            };
            format!("-target {zig_arch}-linux-{target_env}{abi_suffix} {zig_cpu} {cc_args}",)
        }
        OperatingSystem::MacOSX { .. } | OperatingSystem::Darwin => {
            let zig_version = Zig::zig_version()?;
            // Zig 0.10.0 switched macOS ABI to none
            // see https://github.com/ziglang/zig/pull/11684
            if zig_version > semver::Version::new(0, 9, 1) {
                format!("-target {arch}-macos-none{abi_suffix} {cc_args}")
            } else {
                format!("-target {arch}-macos-gnu{abi_suffix} {cc_args}")
            }
        }
        OperatingSystem::Windows { .. } => {
            let zig_arch = match arch.as_str() {
                "i686" => "i386",
                arch => arch,
            };
            format!("-target {zig_arch}-windows-{target_env}{abi_suffix} {cc_args}",)
        }
        _ => bail!(format!("unsupported target '{rust_target}'")),
    };

    let zig_linker_dir = cache_dir();
    fs::create_dir_all(&zig_linker_dir)?;

    let fcntl_map = zig_linker_dir.join("fcntl.map");
    fs::write(&fcntl_map, FCNTL_MAP)?;
    let fcntl_h = zig_linker_dir.join("fcntl.h");
    fs::write(&fcntl_h, FCNTL_H)?;

    if triple.operating_system == OperatingSystem::Linux
        && matches!(
            triple.environment,
            Environment::Gnu
                | Environment::Gnuspe
                | Environment::Gnux32
                | Environment::Gnueabi
                | Environment::Gnuabi64
                | Environment::GnuIlp32
                | Environment::Gnueabihf
        )
    {
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
            use std::fmt::Write as _;

            write!(
                cc_args,
                " -Wl,--version-script={} -include {}",
                fcntl_map.display(),
                fcntl_h.display()
            )
            .unwrap();
        }
    }

    let zig_cc = zig_linker_dir.join(zig_cc);
    let zig_cxx = zig_linker_dir.join(zig_cxx);
    let zig_ranlib = zig_linker_dir.join(format!("zigranlib.{file_ext}"));
    write_linker_wrapper(&zig_cc, "cc", &cc_args)?;
    write_linker_wrapper(&zig_cxx, "c++", &cc_args)?;
    write_linker_wrapper(&zig_ranlib, "ranlib", "")?;

    let exe_ext = if cfg!(windows) { ".exe" } else { "" };
    let zig_ar = zig_linker_dir.join(format!("ar{exe_ext}"));
    symlink_wrapper(&zig_ar)?;

    Ok(ZigWrapper {
        cc: zig_cc,
        cxx: zig_cxx,
        ar: zig_ar,
        ranlib: zig_ranlib,
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
    let mut custom_linker_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o700)
        .open(path)?;
    let current_exe = if let Ok(exe) = env::var("CARGO_BIN_EXE_cargo-zigbuild") {
        PathBuf::from(exe)
    } else {
        env::current_exe()?
    };
    writeln!(&mut custom_linker_file, "#!/usr/bin/env sh")?;
    writeln!(
        &mut custom_linker_file,
        "{} zig {} -- {} $@",
        current_exe.display(),
        command,
        args
    )?;
    Ok(())
}

/// Write a zig cc wrapper batch script for windows
#[cfg(not(target_family = "unix"))]
fn write_linker_wrapper(path: &Path, command: &str, args: &str) -> Result<()> {
    let mut custom_linker_file = fs::File::create(path)?;
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
        &mut custom_linker_file,
        "{} zig {} -- {} %*",
        adjust_canonicalization(current_exe),
        command,
        args
    )?;
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
