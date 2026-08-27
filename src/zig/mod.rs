mod cargo_env;
mod linker_args;
mod locate;
mod target_info;
mod wrapper;

use std::env;
use std::process;

use anyhow::{Context, Result, bail};
use fs_err as fs;
use target_lexicon::Architecture;

use cargo_env::{write_file, write_tbd_files};
use linker_args::{FilteredArg, dedup_apple_link_libs, filter_linker_arg, filter_linker_args};
use locate::cache_dir;
use target_info::TargetInfo;

#[cfg(target_os = "windows")]
pub use wrapper::adjust_canonicalization;
pub use wrapper::{ZigWrapper, prepare_zig_linker};

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
        let mut seen_target = false;
        for arg in cmd_args {
            if skip_next_arg {
                skip_next_arg = false;
                continue;
            }
            // Our wrapper script already passes the correct -target;
            // skip any duplicate -target from rustc to avoid conflicts
            // (e.g. rustc passes arm64 which zig doesn't recognize for some targets)
            if arg == "-target" {
                if seen_target {
                    skip_next_arg = true;
                    continue;
                }
                seen_target = true;
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

        if target_info.is_apple_platform() {
            new_cmd_args = dedup_apple_link_libs(new_cmd_args);
        }

        if target_info.is_mips32() {
            // See https://github.com/ziglang/zig/issues/4925#issuecomment-1499823425
            new_cmd_args.push("-Wl,-z,notext".to_string());
        }

        // Rust's libstd for strict-align arm targets calls the ARM RTABI
        // unaligned-access helpers (__aeabi_uread4 etc.), which libgcc
        // provides but zig's compiler-rt does not; link weak definitions
        if target_info.is_arm() && !cmd_args.iter().any(|x| x == "-c" || x == "-E" || x == "-S") {
            let cache_dir = cache_dir();
            fs::create_dir_all(&cache_dir)?;
            let shim_path = cache_dir.join("aeabi_unaligned.c");
            write_file(&shim_path, AEABI_UNALIGNED_C)?;
            new_cmd_args.push(shim_path.display().to_string());
        }

        if target_info.is_windows_gnu() && (zig_version.major, zig_version.minor) >= (0, 16) {
            new_cmd_args.push("-lcompiler_rt".to_string());
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
}

/// Weak definitions of the ARM RTABI unaligned-access helpers
/// (run-time ABI for the ARM architecture, IHI0043, section 4.3.3).
/// libgcc provides these but LLVM's (and zig's) compiler-rt does not,
/// and Rust's libstd for strict-align arm targets calls them.
const AEABI_UNALIGNED_C: &str = r#"
#ifdef __cplusplus
extern "C" {
#endif
__attribute__((weak)) int __aeabi_uread4(void *address) {
    int value;
    __builtin_memcpy(&value, address, 4);
    return value;
}
__attribute__((weak)) int __aeabi_uwrite4(int value, void *address) {
    __builtin_memcpy(address, &value, 4);
    return value;
}
__attribute__((weak)) long long __aeabi_uread8(void *address) {
    long long value;
    __builtin_memcpy(&value, address, 8);
    return value;
}
__attribute__((weak)) long long __aeabi_uwrite8(long long value, void *address) {
    __builtin_memcpy(address, &value, 8);
    return value;
}
#ifdef __cplusplus
}
#endif
"#;

/// Get the dlltool executable name for the given architecture
/// On Windows, rustc looks for "dlltool.exe"
/// On non-Windows hosts, rustc looks for architecture-specific names
pub(crate) fn get_dlltool_name(arch: &Architecture) -> &'static str {
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
pub(crate) fn has_system_dlltool(arch: &Architecture) -> bool {
    which::which(get_dlltool_name(arch)).is_ok()
}
