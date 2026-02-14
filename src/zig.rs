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
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow, bail};
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
    /// `zig dlltool` wrapper
    #[command(name = "dlltool")]
    Dlltool {
        /// `zig dlltool` arguments
        #[arg(num_args = 1.., trailing_var_arg = true)]
        args: Vec<String>,
    },
}

struct TargetInfo {
    target: Option<String>,
}

impl TargetInfo {
    fn new(target: Option<&String>) -> Self {
        Self {
            target: target.cloned(),
        }
    }

    // Architecture helpers
    fn is_arm(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.starts_with("arm"))
            .unwrap_or_default()
    }

    fn is_aarch64(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.starts_with("aarch64"))
            .unwrap_or_default()
    }

    fn is_aarch64_be(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.starts_with("aarch64_be"))
            .unwrap_or_default()
    }

    fn is_i386(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.starts_with("i386"))
            .unwrap_or_default()
    }

    fn is_i686(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.starts_with("i686") || x.starts_with("x86-"))
            .unwrap_or_default()
    }

    fn is_riscv64(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.starts_with("riscv64"))
            .unwrap_or_default()
    }

    fn is_riscv32(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.starts_with("riscv32"))
            .unwrap_or_default()
    }

    fn is_mips32(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.starts_with("mips") && !x.starts_with("mips64"))
            .unwrap_or_default()
    }

    // libc helpers
    fn is_musl(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("musl"))
            .unwrap_or_default()
    }

    // Platform helpers
    fn is_macos(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("macos"))
            .unwrap_or_default()
    }

    fn is_darwin(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("darwin"))
            .unwrap_or_default()
    }

    fn is_apple_platform(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| {
                x.contains("macos")
                    || x.contains("darwin")
                    || x.contains("ios")
                    || x.contains("tvos")
                    || x.contains("watchos")
                    || x.contains("visionos")
            })
            .unwrap_or_default()
    }

    fn is_ios(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("ios") && !x.contains("visionos"))
            .unwrap_or_default()
    }

    fn is_tvos(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("tvos"))
            .unwrap_or_default()
    }

    fn is_watchos(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("watchos"))
            .unwrap_or_default()
    }

    fn is_visionos(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("visionos"))
            .unwrap_or_default()
    }

    /// Returns the appropriate Apple CPU for the platform
    fn apple_cpu(&self) -> &'static str {
        if self.is_macos() || self.is_darwin() {
            "apple_m1" // M-series for macOS
        } else if self.is_visionos() {
            "apple_m2" // M2 for Apple Vision Pro
        } else if self.is_watchos() {
            "apple_s5" // S-series for Apple Watch
        } else if self.is_ios() || self.is_tvos() {
            "apple_a14" // A-series for iOS/tvOS (iPhone 12 era - good baseline)
        } else {
            "generic"
        }
    }

    fn is_freebsd(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("freebsd"))
            .unwrap_or_default()
    }

    fn is_windows_gnu(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("windows-gnu"))
            .unwrap_or_default()
    }

    fn is_windows_msvc(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("windows-msvc"))
            .unwrap_or_default()
    }

    fn is_ohos(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("ohos"))
            .unwrap_or_default()
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
            Zig::Dlltool { args } => self.execute_dlltool(args),
        }
    }

    /// Execute zig dlltool command
    /// Filter out unsupported options for older zig versions (< 0.12)
    pub fn execute_dlltool(&self, cmd_args: &[String]) -> Result<()> {
        let zig_version = Zig::zig_version()?;
        let needs_filtering = zig_version.major == 0 && zig_version.minor < 12;

        if !needs_filtering {
            return self.execute_tool("dlltool", cmd_args);
        }

        // Filter out --no-leading-underscore, --temp-prefix, and -t (short form)
        // These options are not supported by zig dlltool in versions < 0.12
        let mut filtered_args = Vec::with_capacity(cmd_args.len());
        let mut skip_next = false;
        for arg in cmd_args {
            if skip_next {
                skip_next = false;
                continue;
            }
            if arg == "--no-leading-underscore" {
                continue;
            }
            if arg == "--temp-prefix" || arg == "-t" {
                // Skip this arg and the next one (the value)
                skip_next = true;
                continue;
            }
            // Handle --temp-prefix=value and -t=value forms
            if arg.starts_with("--temp-prefix=") || arg.starts_with("-t=") {
                continue;
            }
            filtered_args.push(arg.clone());
        }

        self.execute_tool("dlltool", &filtered_args)
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
                match self.filter_linker_arg(arg, &rustc_ver, &zig_version, &target_info) {
                    FilteredArg::Keep(filtered) => filtered,
                    FilteredArg::Skip => continue,
                    FilteredArg::SkipWithNext => {
                        skip_next_arg = true;
                        continue;
                    }
                }
            };
            new_cmd_args.extend(args);
        }

        if target_info.is_mips32() {
            // See https://github.com/ziglang/zig/issues/4925#issuecomment-1499823425
            new_cmd_args.push("-Wl,-z,notext".to_string());
        }

        if self.has_undefined_dynamic_lookup(cmd_args) {
            new_cmd_args.push("-Wl,-undefined=dynamic_lookup".to_string());
        }
        if target_info.is_macos() {
            if self.should_add_libcharset(cmd_args, &zig_version) {
                new_cmd_args.push("-lcharset".to_string());
            }
            self.add_macos_specific_args(&mut new_cmd_args, &zig_version)?;
        }

        // For Zig >= 0.15 with macOS, set SDKROOT environment variable
        // if it exists, instead of passing --sysroot
        let mut command = Self::command()?;
        if (zig_version.major, zig_version.minor) >= (0, 15)
            && let Some(sdkroot) = Self::macos_sdk_root()
        {
            command.env("SDKROOT", sdkroot);
        }

        let mut child = command
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
        let content = if target_info.is_windows_msvc() {
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
        let mut link_args: Vec<_> = filter_linker_args(
            content.split('\n').map(|s| s.to_string()),
            rustc_ver,
            zig_version,
            target_info,
        );
        if self.has_undefined_dynamic_lookup(&link_args) {
            link_args.push("-Wl,-undefined=dynamic_lookup".to_string());
        }
        if target_info.is_macos() && self.should_add_libcharset(&link_args, zig_version) {
            link_args.push("-lcharset".to_string());
        }
        if target_info.is_windows_msvc() {
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
    ) -> FilteredArg {
        filter_linker_arg(arg, rustc_ver, zig_version, target_info)
    }
}

enum FilteredArg {
    Keep(Vec<String>),
    Skip,
    SkipWithNext,
}

fn filter_linker_args(
    args: impl IntoIterator<Item = String>,
    rustc_ver: &rustc_version::Version,
    zig_version: &semver::Version,
    target_info: &TargetInfo,
) -> Vec<String> {
    let mut result = Vec::new();
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        match filter_linker_arg(&arg, rustc_ver, zig_version, target_info) {
            FilteredArg::Keep(filtered) => result.extend(filtered),
            FilteredArg::Skip => {}
            FilteredArg::SkipWithNext => {
                skip_next = true;
            }
        }
    }
    result
}

fn filter_linker_arg(
    arg: &str,
    rustc_ver: &rustc_version::Version,
    zig_version: &semver::Version,
    target_info: &TargetInfo,
) -> FilteredArg {
    if arg == "-lgcc_s" {
        return FilteredArg::Keep(vec!["-lunwind".to_string()]);
    } else if arg.starts_with("--target=") {
        return FilteredArg::Skip;
    } else if arg.starts_with("-e") && arg.len() > 2 && !arg.starts_with("-export") {
        let entry = &arg[2..];
        return FilteredArg::Keep(vec![format!("-Wl,--entry={}", entry)]);
    }
    if (target_info.is_arm() || target_info.is_windows_gnu())
        && arg.ends_with(".rlib")
        && arg.contains("libcompiler_builtins-")
    {
        return FilteredArg::Skip;
    }
    if target_info.is_windows_gnu() {
        #[allow(clippy::if_same_then_else)]
        if arg == "-lgcc_eh"
            && ((zig_version.major, zig_version.minor) < (0, 14) || target_info.is_i686())
        {
            return FilteredArg::Keep(vec!["-lc++".to_string()]);
        } else if arg.ends_with("rsbegin.o") || arg.ends_with("rsend.o") {
            if target_info.is_i686() {
                return FilteredArg::Skip;
            }
        } else if arg == "-Wl,-Bdynamic" && (zig_version.major, zig_version.minor) >= (0, 11) {
            return FilteredArg::Keep(vec!["-Wl,-search_paths_first".to_owned()]);
        } else if arg == "-lwindows" || arg == "-l:libpthread.a" || arg == "-lgcc" {
            return FilteredArg::Skip;
        } else if arg == "-Wl,--disable-auto-image-base"
            || arg == "-Wl,--dynamicbase"
            || arg == "-Wl,--large-address-aware"
            || (arg.starts_with("-Wl,")
                && (arg.ends_with("/list.def") || arg.ends_with("\\list.def")))
        {
            return FilteredArg::Skip;
        } else if arg == "-lmsvcrt" {
            return FilteredArg::Skip;
        }
    } else if arg == "-Wl,--no-undefined-version"
        || arg == "-Wl,-znostart-stop-gc"
        || arg.starts_with("-Wl,-plugin-opt")
    {
        return FilteredArg::Skip;
    }
    if target_info.is_musl() || target_info.is_ohos() {
        if (arg.ends_with(".o") && arg.contains("self-contained") && arg.contains("crt"))
            || arg == "-Wl,-melf_i386"
        {
            return FilteredArg::Skip;
        }
        if rustc_ver.major == 1
            && rustc_ver.minor < 59
            && arg.ends_with(".rlib")
            && arg.contains("liblibc-")
        {
            return FilteredArg::Skip;
        }
        if arg == "-lc" {
            return FilteredArg::Skip;
        }
    }
    if arg.starts_with("-march=") {
        if target_info.is_arm() || target_info.is_i386() {
            return FilteredArg::Skip;
        } else if target_info.is_riscv64() {
            return FilteredArg::Keep(vec!["-march=generic_rv64".to_string()]);
        } else if target_info.is_riscv32() {
            return FilteredArg::Keep(vec!["-march=generic_rv32".to_string()]);
        } else if arg.starts_with("-march=armv")
            && (target_info.is_aarch64() || target_info.is_aarch64_be())
        {
            let march_value = arg.strip_prefix("-march=").unwrap();
            let features = if let Some(pos) = march_value.find('+') {
                &march_value[pos..]
            } else {
                ""
            };
            let base_cpu = if target_info.is_apple_platform() {
                target_info.apple_cpu()
            } else {
                "generic"
            };
            let mut result = vec![format!("-mcpu={}{}", base_cpu, features)];
            if features.contains("+crypto") {
                result.append(&mut vec!["-Xassembler".to_owned(), arg.to_string()]);
            }
            return FilteredArg::Keep(result);
        }
    }
    if target_info.is_apple_platform() {
        if (zig_version.major, zig_version.minor) < (0, 16) {
            if arg.starts_with("-Wl,-exported_symbols_list,") {
                return FilteredArg::Skip;
            }
            if arg == "-Wl,-exported_symbols_list" {
                return FilteredArg::SkipWithNext;
            }
        }
        if arg == "-Wl,-dylib" {
            return FilteredArg::Skip;
        }
    }
    // Handle two-arg form on all platforms (cross-compilation from non-Apple hosts)
    if (zig_version.major, zig_version.minor) < (0, 16) {
        if arg == "-Wl,-exported_symbols_list" || arg == "-Wl,--dynamic-list" {
            return FilteredArg::SkipWithNext;
        }
        if arg.starts_with("-Wl,-exported_symbols_list,") || arg.starts_with("-Wl,--dynamic-list,")
        {
            return FilteredArg::Skip;
        }
    }
    if target_info.is_freebsd() {
        let ignored_libs = ["-lkvm", "-lmemstat", "-lprocstat", "-ldevstat"];
        if ignored_libs.contains(&arg) {
            return FilteredArg::Skip;
        }
    }
    FilteredArg::Keep(vec![arg.to_string()])
}

impl Zig {
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
            // However, for Zig 0.15+, we should use SDKROOT environment variable instead
            // to avoid issues with library paths being interpreted relative to sysroot
            if let Some(ref sdkroot) = sdkroot
                && (zig_version.major, zig_version.minor) < (0, 15)
            {
                new_cmd_args.push(format!("--sysroot={}", sdkroot.display()));
            }
            // For Zig >= 0.15, SDKROOT will be set as environment variable
        }
        if let Some(ref sdkroot) = sdkroot {
            if (zig_version.major, zig_version.minor) < (0, 15) {
                // For zig < 0.15, we need to explicitly add SDK paths with --sysroot
                new_cmd_args.extend_from_slice(&[
                    "-isystem".to_string(),
                    format!("{}", sdkroot.join("usr").join("include").display()),
                    format!("-L{}", sdkroot.join("usr").join("lib").display()),
                    format!(
                        "-F{}",
                        sdkroot
                            .join("System")
                            .join("Library")
                            .join("Frameworks")
                            .display()
                    ),
                    "-DTARGET_OS_IPHONE=0".to_string(),
                ]);
            } else {
                // For zig >= 0.15 with SDKROOT, we still need to add framework paths
                // Use -iframework for framework header search
                new_cmd_args.extend_from_slice(&[
                    "-isystem".to_string(),
                    format!("{}", sdkroot.join("usr").join("include").display()),
                    format!("-L{}", sdkroot.join("usr").join("lib").display()),
                    format!(
                        "-F{}",
                        sdkroot
                            .join("System")
                            .join("Library")
                            .join("Frameworks")
                            .display()
                    ),
                    // Also add the SYSTEM framework search path
                    "-iframework".to_string(),
                    format!(
                        "{}",
                        sdkroot
                            .join("System")
                            .join("Library")
                            .join("Frameworks")
                            .display()
                    ),
                    "-DTARGET_OS_IPHONE=0".to_string(),
                ]);
            }
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
        static ZIG_VERSION: OnceLock<semver::Version> = OnceLock::new();

        if let Some(version) = ZIG_VERSION.get() {
            return Ok(version.clone());
        }
        // Check for cached version from environment variable first
        if let Ok(version_str) = env::var("CARGO_ZIGBUILD_ZIG_VERSION")
            && let Ok(version) = semver::Version::parse(&version_str)
        {
            return Ok(ZIG_VERSION.get_or_init(|| version).clone());
        }
        let output = Self::command()?.arg("version").output()?;
        let version_str =
            str::from_utf8(&output.stdout).context("`zig version` didn't return utf8 output")?;
        let version = semver::Version::parse(version_str.trim())?;
        Ok(ZIG_VERSION.get_or_init(|| version).clone())
    }

    /// Search for `python -m ziglang` first and for `zig` second.
    pub fn find_zig() -> Result<(PathBuf, Vec<String>)> {
        static ZIG_PATH: OnceLock<(PathBuf, Vec<String>)> = OnceLock::new();

        if let Some(cached) = ZIG_PATH.get() {
            return Ok(cached.clone());
        }
        let result = Self::find_zig_python()
            .or_else(|_| Self::find_zig_bin())
            .context("Failed to find zig")?;
        Ok(ZIG_PATH.get_or_init(|| result).clone())
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
        static LIB_DIR: OnceLock<PathBuf> = OnceLock::new();

        if let Some(cached) = LIB_DIR.get() {
            return Ok(cached.clone());
        }
        let (zig, zig_args) = Self::find_zig()?;
        let zig_version = Self::zig_version()?;
        let output = Command::new(zig).args(zig_args).arg("env").output()?;
        let parse_zon_lib_dir = || -> Result<PathBuf> {
            let output_str =
                str::from_utf8(&output.stdout).context("`zig env` didn't return utf8 output")?;
            let lib_dir = output_str
                .find(".lib_dir")
                .and_then(|idx| {
                    let bytes = output_str.as_bytes();
                    let mut start = idx;
                    while start < bytes.len() && bytes[start] != b'"' {
                        start += 1;
                    }
                    if start >= bytes.len() {
                        return None;
                    }
                    let mut end = start + 1;
                    while end < bytes.len() && bytes[end] != b'"' {
                        end += 1;
                    }
                    if end >= bytes.len() {
                        return None;
                    }
                    Some(&output_str[start + 1..end])
                })
                .context("Failed to parse lib_dir from `zig env` ZON output")?;
            Ok(PathBuf::from(lib_dir))
        };
        let lib_dir = if zig_version >= semver::Version::new(0, 15, 0) {
            parse_zon_lib_dir()?
        } else {
            serde_json::from_slice::<ZigEnv>(&output.stdout)
                .map(|zig_env| PathBuf::from(zig_env.lib_dir))
                .or_else(|_| parse_zon_lib_dir())?
        };
        Ok(LIB_DIR.get_or_init(|| lib_dir).clone())
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
        for (parsed_target, raw_target) in rust_targets.iter().zip(raw_targets) {
            let env_target = parsed_target.replace('-', "_");
            let zig_wrapper = prepare_zig_linker(raw_target, &cargo_config)?;

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
                    let cache_dir = cache_dir();
                    let existing_path = env::var_os("PATH").unwrap_or_default();
                    let paths = std::iter::once(cache_dir).chain(env::split_paths(&existing_path));
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
        let cpp_pre_len = cpp_paths
            .iter()
            .position(|p| {
                p == c_paths
                    .iter()
                    .find(|(kind, _)| *kind == Kind::Normal)
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
                // Required by zig 0.15+ libc++ for streambuf and other I/O headers
                "-D_LIBCPP_HAS_LOCALIZATION=1",
                "-D_LIBCPP_HAS_WIDE_CHARACTERS=1",
                "-D_LIBCPP_HAS_UNICODE=1",
                "-D_LIBCPP_HAS_THREADS=1",
                "-D_LIBCPP_HAS_MONOTONIC_CLOCK",
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
    fn macos_sdk_root() -> Option<PathBuf> {
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
pub fn prepare_zig_linker(
    target: &str,
    cargo_config: &cargo_config2::Config,
) -> Result<ZigWrapper> {
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
            let mut zig_target_env = target_env.to_string();

            let zig_version = Zig::zig_version()?;

            // Since Zig 0.15.0, arm-linux-ohos changed to arm-linux-ohoseabi
            // We need to follow the change but target_lexicon follow the LLVM target(https://github.com/bytecodealliance/target-lexicon/pull/123).
            // So we use string directly.
            if zig_version >= semver::Version::new(0, 15, 0)
                && arch.as_str() == "armv7"
                && target_env == Environment::Ohos
            {
                zig_target_env = "ohoseabi".to_string();
            }

            cc_args.push("-target".to_string());
            cc_args.push(format!("{zig_arch}-linux-{zig_target_env}{abi_suffix}"));
        }
        OperatingSystem::MacOSX { .. } | OperatingSystem::Darwin(_) => {
            let zig_version = Zig::zig_version()?;
            // Zig 0.10.0 switched macOS ABI to none
            // see https://github.com/ziglang/zig/pull/11684
            if zig_version > semver::Version::new(0, 9, 1) {
                cc_args.push("-target".to_string());
                cc_args.push(format!("{arch}-macos-none{abi_suffix}"));
            } else {
                cc_args.push("-target".to_string());
                cc_args.push(format!("{arch}-macos-gnu{abi_suffix}"));
            }
        }
        OperatingSystem::Windows => {
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
            cc_args.push("-target".to_string());
            cc_args.push(format!("{zig_arch}-windows-{target_env}{abi_suffix}"));
        }
        OperatingSystem::Emscripten => {
            cc_args.push("-target".to_string());
            cc_args.push(format!("{arch}-emscripten{abi_suffix}"));
        }
        OperatingSystem::Wasi => {
            cc_args.push("-target".to_string());
            cc_args.push(format!("{arch}-wasi{abi_suffix}"));
        }
        OperatingSystem::WasiP1 => {
            cc_args.push("-target".to_string());
            cc_args.push(format!("{arch}-wasi.0.1.0{abi_suffix}"));
        }
        OperatingSystem::Freebsd => {
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
            cc_args.push("-target".to_string());
            cc_args.push(format!("{zig_arch}-freebsd"));
        }
        OperatingSystem::Unknown => {
            if triple.architecture == Architecture::Wasm32
                || triple.architecture == Architecture::Wasm64
            {
                cc_args.push("-target".to_string());
                cc_args.push(format!("{arch}-freestanding{abi_suffix}"));
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
                    cc_args.push("-include".to_string());
                    cc_args.push(fcntl_h.display().to_string());
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

    // Use platform-specific quoting: shell_words for Unix (single quotes),
    // custom quoting for Windows batch files (double quotes)
    let cc_args_str = join_args_for_script(&cc_args);
    let hash = crc::Crc::<u16>::new(&crc::CRC_16_IBM_SDLC).checksum(cc_args_str.as_bytes());
    let zig_cc = zig_linker_dir.join(format!("zigcc-{file_target}-{:x}.{file_ext}", hash));
    let zig_cxx = zig_linker_dir.join(format!("zigcxx-{file_target}-{:x}.{file_ext}", hash));
    let zig_ranlib = zig_linker_dir.join(format!("zigranlib.{file_ext}"));
    let zig_version = Zig::zig_version()?;
    write_linker_wrapper(&zig_cc, "cc", &cc_args_str, &zig_version)?;
    write_linker_wrapper(&zig_cxx, "c++", &cc_args_str, &zig_version)?;
    write_linker_wrapper(&zig_ranlib, "ranlib", "", &zig_version)?;

    let exe_ext = if cfg!(windows) { ".exe" } else { "" };
    let zig_ar = zig_linker_dir.join(format!("ar{exe_ext}"));
    symlink_wrapper(&zig_ar)?;
    let zig_lib = zig_linker_dir.join(format!("lib{exe_ext}"));
    symlink_wrapper(&zig_lib)?;

    // Create dlltool symlinks for Windows GNU targets, but only if no system dlltool exists
    // On Windows hosts, rustc looks for "dlltool.exe"
    // On non-Windows hosts, rustc looks for architecture-specific names
    //
    // See https://github.com/rust-lang/rust/blob/a18e6d9d1473d9b25581dd04bef6c7577999631c/compiler/rustc_codegen_ssa/src/back/archive.rs#L275-L309
    if matches!(triple.operating_system, OperatingSystem::Windows)
        && matches!(triple.environment, Environment::Gnu)
    {
        // Only create zig dlltool wrapper if no system dlltool is found
        // System dlltool (from mingw-w64) handles raw-dylib better than zig's dlltool
        if !has_system_dlltool(&triple.architecture) {
            let dlltool_name = get_dlltool_name(&triple.architecture);
            let zig_dlltool = zig_linker_dir.join(format!("{dlltool_name}{exe_ext}"));
            symlink_wrapper(&zig_dlltool)?;
        }
    }

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

/// Join arguments for Unix shell script using shell_words (single quotes)
#[cfg(target_family = "unix")]
fn join_args_for_script<I, S>(args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    shell_words::join(args)
}

/// Quote a string for Windows batch file (cmd.exe)
///
/// - `%` expands even inside quotes, so we escape it as `%%`.
/// - We disable delayed expansion in the wrapper script, so `!` should not expand.
/// - Internal `"` are escaped by doubling them (`""`).
#[cfg(not(target_family = "unix"))]
fn quote_for_batch(s: &str) -> String {
    let needs_quoting_or_escaping = s.is_empty()
        || s.contains(|c: char| {
            matches!(
                c,
                ' ' | '\t' | '"' | '&' | '|' | '<' | '>' | '^' | '%' | '(' | ')' | '!'
            )
        });

    if !needs_quoting_or_escaping {
        return s.to_string();
    }

    let mut out = String::with_capacity(s.len() + 8);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\"\""),
            '%' => out.push_str("%%"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Join arguments for Windows batch file using double quotes
#[cfg(not(target_family = "unix"))]
fn join_args_for_script<I, S>(args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter()
        .map(|s| quote_for_batch(s.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Write a zig cc wrapper batch script for unix
#[cfg(target_family = "unix")]
fn write_linker_wrapper(
    path: &Path,
    command: &str,
    args: &str,
    zig_version: &semver::Version,
) -> Result<()> {
    let mut buf = Vec::<u8>::new();
    let current_exe = if let Ok(exe) = env::var("CARGO_BIN_EXE_cargo-zigbuild") {
        PathBuf::from(exe)
    } else {
        env::current_exe()?
    };
    writeln!(&mut buf, "#!/bin/sh")?;

    // Export zig version to avoid spawning `zig version` subprocess
    writeln!(
        &mut buf,
        "export CARGO_ZIGBUILD_ZIG_VERSION={}",
        zig_version
    )?;

    // Pass through SDKROOT if it exists at runtime
    writeln!(&mut buf, "if [ -n \"$SDKROOT\" ]; then export SDKROOT; fi")?;

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
fn write_linker_wrapper(
    path: &Path,
    command: &str,
    args: &str,
    zig_version: &semver::Version,
) -> Result<()> {
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
    writeln!(&mut buf, "@echo off")?;
    // Prevent `!VAR!` expansion surprises (delayed expansion) in user-controlled args.
    writeln!(&mut buf, "setlocal DisableDelayedExpansion")?;
    // Set zig version to avoid spawning `zig version` subprocess
    writeln!(&mut buf, "set CARGO_ZIGBUILD_ZIG_VERSION={}", zig_version)?;
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

/// Get the dlltool executable name for the given architecture
/// On Windows, rustc looks for "dlltool.exe"
/// On non-Windows hosts, rustc looks for architecture-specific names
fn get_dlltool_name(arch: &Architecture) -> &'static str {
    if cfg!(windows) {
        "dlltool"
    } else {
        match arch {
            Architecture::X86_64 => "x86_64-w64-mingw32-dlltool",
            Architecture::X86_32(_) => "i686-w64-mingw32-dlltool",
            Architecture::Aarch64(_) => "aarch64-w64-mingw32-dlltool",
            _ => "dlltool",
        }
    }
}

/// Check if a dlltool for the given architecture exists in PATH
/// Returns true if found, false otherwise
fn has_system_dlltool(arch: &Architecture) -> bool {
    which::which(get_dlltool_name(arch)).is_ok()
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

    #[test]
    fn test_join_args_for_script() {
        // Test basic arguments without special characters
        let args = vec!["-target", "x86_64-linux-gnu"];
        let result = join_args_for_script(&args);
        assert!(result.contains("-target"));
        assert!(result.contains("x86_64-linux-gnu"));
    }

    #[test]
    #[cfg(not(target_family = "unix"))]
    fn test_quote_for_batch() {
        // Simple argument without special characters - no quoting needed
        assert_eq!(quote_for_batch("-target"), "-target");
        assert_eq!(quote_for_batch("x86_64-linux-gnu"), "x86_64-linux-gnu");

        // Arguments with spaces need quoting
        assert_eq!(
            quote_for_batch("C:\\Users\\John Doe\\path"),
            "\"C:\\Users\\John Doe\\path\""
        );

        // Empty string needs quoting
        assert_eq!(quote_for_batch(""), "\"\"");

        // Arguments with special batch characters need quoting
        assert_eq!(quote_for_batch("foo&bar"), "\"foo&bar\"");
        assert_eq!(quote_for_batch("foo|bar"), "\"foo|bar\"");
        assert_eq!(quote_for_batch("foo<bar"), "\"foo<bar\"");
        assert_eq!(quote_for_batch("foo>bar"), "\"foo>bar\"");
        assert_eq!(quote_for_batch("foo^bar"), "\"foo^bar\"");
        assert_eq!(quote_for_batch("foo%bar"), "\"foo%bar\"");

        // Internal double quotes are escaped by doubling
        assert_eq!(quote_for_batch("foo\"bar"), "\"foo\"\"bar\"");
    }

    #[test]
    #[cfg(not(target_family = "unix"))]
    fn test_join_args_for_script_windows() {
        // Test with path containing spaces
        let args = vec![
            "-target",
            "x86_64-linux-gnu",
            "-L",
            "C:\\Users\\John Doe\\path",
        ];
        let result = join_args_for_script(&args);
        // The path with space should be quoted
        assert!(result.contains("\"C:\\Users\\John Doe\\path\""));
        // Simple args should not be quoted
        assert!(result.contains("-target"));
        assert!(!result.contains("\"-target\""));
    }

    fn make_rustc_ver(major: u64, minor: u64, patch: u64) -> rustc_version::Version {
        rustc_version::Version::new(major, minor, patch)
    }

    fn make_zig_ver(major: u64, minor: u64, patch: u64) -> semver::Version {
        semver::Version::new(major, minor, patch)
    }

    fn run_filter(args: &[&str], target: Option<&str>, zig_ver: (u64, u64)) -> Vec<String> {
        let rustc_ver = make_rustc_ver(1, 80, 0);
        let zig_version = make_zig_ver(0, zig_ver.0, zig_ver.1);
        let target_info = TargetInfo::new(target.map(|s| s.to_string()).as_ref());
        filter_linker_args(
            args.iter().map(|s| s.to_string()),
            &rustc_ver,
            &zig_version,
            &target_info,
        )
    }

    fn run_filter_one(arg: &str, target: Option<&str>, zig_ver: (u64, u64)) -> Vec<String> {
        run_filter(&[arg], target, zig_ver)
    }

    fn run_filter_one_rustc(
        arg: &str,
        target: Option<&str>,
        zig_ver: (u64, u64),
        rustc_minor: u64,
    ) -> Vec<String> {
        let rustc_ver = make_rustc_ver(1, rustc_minor, 0);
        let zig_version = make_zig_ver(0, zig_ver.0, zig_ver.1);
        let target_info = TargetInfo::new(target.map(|s| s.to_string()).as_ref());
        filter_linker_args(
            std::iter::once(arg.to_string()),
            &rustc_ver,
            &zig_version,
            &target_info,
        )
    }

    #[test]
    fn test_filter_common_replacements() {
        let linux = Some("x86_64-unknown-linux-gnu");
        // -lgcc_s -> -lunwind
        assert_eq!(run_filter_one("-lgcc_s", linux, (13, 0)), vec!["-lunwind"]);
        // --target= stripped (already passed via -target)
        assert!(run_filter_one("--target=x86_64-unknown-linux-gnu", linux, (13, 0)).is_empty());
        // -e<entry> transformed to -Wl,--entry=<entry>
        assert_eq!(
            run_filter_one("-emain", linux, (13, 0)),
            vec!["-Wl,--entry=main"]
        );
        // -export-* should NOT be transformed
        assert_eq!(
            run_filter_one("-export-dynamic", linux, (13, 0)),
            vec!["-export-dynamic"]
        );
    }

    #[test]
    fn test_filter_compiler_builtins_removed() {
        for target in &["armv7-unknown-linux-gnueabihf", "x86_64-pc-windows-gnu"] {
            let result = run_filter_one(
                "/path/to/libcompiler_builtins-abc123.rlib",
                Some(target),
                (13, 0),
            );
            assert!(
                result.is_empty(),
                "compiler_builtins should be removed for {target}"
            );
        }
    }

    #[test]
    fn test_filter_windows_gnu_args() {
        let gnu = Some("x86_64-pc-windows-gnu");
        // Args that should be removed entirely
        let removed: &[&str] = &[
            "-lwindows",
            "-l:libpthread.a",
            "-lgcc",
            "-Wl,--disable-auto-image-base",
            "-Wl,--dynamicbase",
            "-Wl,--large-address-aware",
            "-Wl,/path/to/list.def",
            "-Wl,C:\\path\\to\\list.def",
            "-lmsvcrt",
        ];
        for arg in removed {
            let result = run_filter_one(arg, gnu, (13, 0));
            assert!(result.is_empty(), "{arg} should be removed for windows-gnu");
        }
        // Args that get replaced
        let replaced: &[(&str, (u64, u64), &str)] = &[
            ("-lgcc_eh", (13, 0), "-lc++"),
            ("-Wl,-Bdynamic", (13, 0), "-Wl,-search_paths_first"),
        ];
        for (arg, zig_ver, expected) in replaced {
            let result = run_filter_one(arg, gnu, *zig_ver);
            assert_eq!(result, vec![*expected], "filter({arg})");
        }
        // -lgcc_eh kept on zig >= 0.14 for x86_64
        let result = run_filter_one("-lgcc_eh", gnu, (14, 0));
        assert_eq!(result, vec!["-lgcc_eh"]);
    }

    #[test]
    fn test_filter_windows_gnu_rsbegin() {
        // i686: rsbegin.o filtered out
        let result = run_filter_one("/path/to/rsbegin.o", Some("i686-pc-windows-gnu"), (13, 0));
        assert!(result.is_empty());
        // x86_64: rsbegin.o kept
        let result = run_filter_one("/path/to/rsbegin.o", Some("x86_64-pc-windows-gnu"), (13, 0));
        assert_eq!(result, vec!["/path/to/rsbegin.o"]);
    }

    #[test]
    fn test_filter_unsupported_linker_args() {
        let linux = Some("x86_64-unknown-linux-gnu");
        let removed: &[&str] = &[
            "-Wl,--no-undefined-version",
            "-Wl,-znostart-stop-gc",
            "-Wl,-plugin-opt=O2",
        ];
        for arg in removed {
            let result = run_filter_one(arg, linux, (13, 0));
            assert!(result.is_empty(), "{arg} should be removed");
        }
    }

    #[test]
    fn test_filter_musl_args() {
        let musl = Some("x86_64-unknown-linux-musl");
        let removed: &[&str] = &["/path/self-contained/crt1.o", "-lc"];
        for arg in removed {
            let result = run_filter_one(arg, musl, (13, 0));
            assert!(result.is_empty(), "{arg} should be removed for musl");
        }
        // -Wl,-melf_i386 for i686 musl
        let result = run_filter_one("-Wl,-melf_i386", Some("i686-unknown-linux-musl"), (13, 0));
        assert!(result.is_empty());
        // liblibc removed for old rustc (<1.59), kept for new
        let result = run_filter_one_rustc("/path/to/liblibc-abc123.rlib", musl, (13, 0), 58);
        assert!(result.is_empty());
        let result = run_filter_one_rustc("/path/to/liblibc-abc123.rlib", musl, (13, 0), 59);
        assert_eq!(result, vec!["/path/to/liblibc-abc123.rlib"]);
    }

    #[test]
    fn test_filter_march_args() {
        // (input, target, expected)
        let cases: &[(&str, &str, &[&str])] = &[
            // arm: removed
            ("-march=armv7-a", "armv7-unknown-linux-gnueabihf", &[]),
            // riscv64: replaced
            (
                "-march=rv64gc",
                "riscv64gc-unknown-linux-gnu",
                &["-march=generic_rv64"],
            ),
            // riscv32: replaced
            (
                "-march=rv32imac",
                "riscv32imac-unknown-none-elf",
                &["-march=generic_rv32"],
            ),
            // aarch64 armv: converted to -mcpu=generic
            (
                "-march=armv8.4-a",
                "aarch64-unknown-linux-gnu",
                &["-mcpu=generic"],
            ),
            // aarch64 armv with crypto: adds -Xassembler
            (
                "-march=armv8.4-a+crypto",
                "aarch64-unknown-linux-gnu",
                &[
                    "-mcpu=generic+crypto",
                    "-Xassembler",
                    "-march=armv8.4-a+crypto",
                ],
            ),
            // apple aarch64: uses apple cpu name
            (
                "-march=armv8.4-a",
                "aarch64-apple-darwin",
                &["-mcpu=apple_m1"],
            ),
        ];
        for (input, target, expected) in cases {
            let result = run_filter_one(input, Some(target), (13, 0));
            assert_eq!(&result, expected, "filter({input}, {target})");
        }
    }

    #[test]
    fn test_filter_apple_args() {
        let darwin = Some("aarch64-apple-darwin");
        let result = run_filter_one("-Wl,-dylib", darwin, (13, 0));
        assert!(result.is_empty());
    }

    #[test]
    fn test_filter_freebsd_libs_removed() {
        for lib in &["-lkvm", "-lmemstat", "-lprocstat", "-ldevstat"] {
            let result = run_filter_one(lib, Some("x86_64-unknown-freebsd"), (13, 0));
            assert!(result.is_empty(), "{lib} should be removed for freebsd");
        }
    }

    #[test]
    fn test_filter_exported_symbols_list_two_arg_apple() {
        let result = run_filter(
            &[
                "-arch",
                "arm64",
                "-Wl,-exported_symbols_list",
                "-Wl,/tmp/rustcXXX/list",
                "-o",
                "output.dylib",
            ],
            Some("aarch64-apple-darwin"),
            (13, 0),
        );
        assert_eq!(result, vec!["-arch", "arm64", "-o", "output.dylib"]);
    }

    #[test]
    fn test_filter_exported_symbols_list_two_arg_cross_platform() {
        let result = run_filter(
            &[
                "-arch",
                "arm64",
                "-Wl,-exported_symbols_list",
                "-Wl,C:\\Users\\RUNNER~1\\AppData\\Local\\Temp\\rustcXXX\\list",
                "-o",
                "output.dylib",
            ],
            None,
            (13, 0),
        );
        assert_eq!(result, vec!["-arch", "arm64", "-o", "output.dylib"]);
    }

    #[test]
    fn test_filter_exported_symbols_list_single_arg_comma() {
        let result = run_filter(
            &[
                "-Wl,-exported_symbols_list,/tmp/rustcXXX/list",
                "-o",
                "output.dylib",
            ],
            Some("aarch64-apple-darwin"),
            (13, 0),
        );
        assert_eq!(result, vec!["-o", "output.dylib"]);
    }

    #[test]
    fn test_filter_exported_symbols_list_not_filtered_zig_016() {
        let result = run_filter(
            &[
                "-Wl,-exported_symbols_list",
                "-Wl,/tmp/rustcXXX/list",
                "-o",
                "output.dylib",
            ],
            Some("aarch64-apple-darwin"),
            (16, 0),
        );
        assert_eq!(
            result,
            vec![
                "-Wl,-exported_symbols_list",
                "-Wl,/tmp/rustcXXX/list",
                "-o",
                "output.dylib"
            ]
        );
    }

    #[test]
    fn test_filter_dynamic_list_two_arg() {
        let result = run_filter(
            &[
                "-Wl,--dynamic-list",
                "-Wl,/tmp/rustcXXX/list",
                "-o",
                "output.so",
            ],
            Some("x86_64-unknown-linux-gnu"),
            (13, 0),
        );
        assert_eq!(result, vec!["-o", "output.so"]);
    }

    #[test]
    fn test_filter_dynamic_list_single_arg_comma() {
        let result = run_filter(
            &["-Wl,--dynamic-list,/tmp/rustcXXX/list", "-o", "output.so"],
            Some("x86_64-unknown-linux-gnu"),
            (13, 0),
        );
        assert_eq!(result, vec!["-o", "output.so"]);
    }

    #[test]
    fn test_filter_preserves_normal_args() {
        let result = run_filter(
            &["-arch", "arm64", "-lSystem", "-lc", "-o", "output"],
            Some("aarch64-apple-darwin"),
            (13, 0),
        );
        assert_eq!(
            result,
            vec!["-arch", "arm64", "-lSystem", "-lc", "-o", "output"]
        );
    }

    #[test]
    fn test_filter_skip_next_at_end_of_args() {
        let result = run_filter(
            &["-o", "output", "-Wl,-exported_symbols_list"],
            Some("aarch64-apple-darwin"),
            (13, 0),
        );
        assert_eq!(result, vec!["-o", "output"]);
    }
}
