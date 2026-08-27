use super::target_info::TargetInfo;

pub(crate) enum FilteredArg {
    Keep(Vec<String>),
    Skip,
    SkipWithNext,
}

pub(crate) fn filter_linker_args(
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
    if target_info.is_apple_platform() {
        result = dedup_apple_link_libs(result);
    }
    result
}

/// Deduplicate `-l` arguments for Apple targets, keeping the first occurrence.
///
/// ld64 ignores duplicate libraries, but Zig's Mach-O linker (since 0.14) emits
/// one `LC_LOAD_DYLIB` load command per `-l` flag, and newer macOS dyld refuses
/// to load binaries with duplicate linked dylibs.
/// See https://github.com/rust-cross/cargo-zigbuild/issues/457
pub(crate) fn dedup_apple_link_libs(args: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    args.into_iter()
        .filter(|arg| {
            if arg.starts_with("-l") && arg.len() > 2 {
                seen.insert(arg.clone())
            } else {
                true
            }
        })
        .collect()
}

pub(crate) fn filter_linker_arg(
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
    } else if let Some(sym) = arg.strip_prefix("-Wl,--undefined=") {
        // zig cc rejects `--undefined=SYM` as an unsupported linker arg, but
        // accepts the synonymous `-u SYM`; used e.g. by cargo-auditable
        // https://github.com/rust-cross/cargo-zigbuild/issues/162
        return FilteredArg::Keep(vec![format!("-Wl,-u,{sym}")]);
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
        // See https://github.com/rust-lang/rust/pull/155453
        || arg == "-Wl,--fix-cortex-a53-843419"
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
    // zig cc only supports -Wp,-MD, -Wp,-MMD, and -Wp,-MT;
    // strip all other -Wp, args (e.g. -Wp,-U_FORTIFY_SOURCE from CMake)
    // https://github.com/ziglang/zig/blob/0.15.2/src/main.zig#L2798
    if arg.starts_with("-Wp,")
        && !arg.starts_with("-Wp,-MD")
        && !arg.starts_with("-Wp,-MMD")
        && !arg.starts_with("-Wp,-MT")
    {
        return FilteredArg::Skip;
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
            let base_cpu = if target_info.is_apple_platform() {
                target_info.apple_cpu()
            } else {
                "generic"
            };
            let (features, has_crypto) = map_aarch64_features(march_value);
            let mut result = vec![format!("-mcpu={base_cpu}{features}")];
            if has_crypto {
                result.append(&mut vec!["-Xassembler".to_owned(), arg.to_string()]);
            }
            return FilteredArg::Keep(result);
        } else {
            // -march values on the remaining architectures are CPU names,
            // e.g. -march=x86-64-v3
            let value = arg.strip_prefix("-march=").unwrap();
            return FilteredArg::Keep(vec![format!("-march={}", map_cpu_name(value, target_info))]);
        }
    }
    if let Some((flag @ ("-mcpu" | "-mtune"), value)) = arg.split_once('=') {
        if target_info.is_aarch64() || target_info.is_aarch64_be() {
            let cpu = value.split('+').next().unwrap();
            let (features, _) = map_aarch64_features(value);
            return FilteredArg::Keep(vec![format!(
                "{flag}={}{features}",
                map_cpu_name(cpu, target_info)
            )]);
        }
        return FilteredArg::Keep(vec![format!("{flag}={}", map_cpu_name(value, target_info))]);
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

/// Map the CPU-name part of a GCC/clang `-mcpu`/`-mtune`/`-march` value to
/// zig's spelling: zig uses underscores where GCC/clang use dashes
/// (e.g. cortex-a53 -> cortex_a53, x86-64-v3 -> x86_64_v3), and powerpc
/// CPUs use the LLVM names (e.g. power9 -> pwr9). Any `+ext` suffixes are
/// preserved as-is; zig-spelled values pass through unchanged.
fn map_cpu_name(value: &str, target_info: &TargetInfo) -> String {
    if target_info.is_s390x() {
        // zig s390x CPU specs like z10-vector use `-` for feature
        // subtraction, and GCC/clang s390x CPU names contain no dashes
        return value.to_string();
    }
    let (cpu, features) = match value.find('+') {
        Some(pos) => value.split_at(pos),
        None => (value, ""),
    };
    if target_info.is_powerpc() {
        let cpu = match cpu {
            "powerpc" => "ppc".to_string(),
            "powerpc64" => "ppc64".to_string(),
            "powerpc64le" => "ppc64le".to_string(),
            _ => match cpu.strip_prefix("power") {
                Some(n) if n.starts_with(|c: char| c.is_ascii_digit()) => format!("pwr{n}"),
                _ => cpu.to_string(),
            },
        };
        return format!("{cpu}{features}");
    }
    format!("{}{features}", cpu.replace('-', "_"))
}

/// Convert the `+ext` suffixes of a GCC/clang aarch64 `-march`/`-mcpu` value
/// (e.g. `armv8.2-a+sve+nofp16`) into zig's feature syntax (`+sve-fullfp16`).
/// Returns the feature string and whether `+crypto` was requested.
fn map_aarch64_features(value: &str) -> (String, bool) {
    let mut extensions = value.split('+');
    let _cpu_or_arch = extensions.next();
    let mut features = String::new();
    let mut has_crypto = false;
    for ext in extensions.filter(|e| !e.is_empty() && *e != "none") {
        has_crypto |= ext == "crypto";
        // GCC/clang spell disabling an extension `+no<ext>`,
        // zig's -mcpu spells it `-<feature>`
        let (sign, name) = match ext.strip_prefix("no") {
            Some(name) if !name.is_empty() => ('-', name),
            _ => ('+', ext),
        };
        features.push(sign);
        // zig spells multi-word feature names with underscores
        // (e.g. sve2_aes) and parses `-` as feature subtraction
        for c in map_aarch64_arch_extension(name).chars() {
            features.push(if c == '-' { '_' } else { c });
        }
    }
    (features, has_crypto)
}

/// Map GCC/clang aarch64 `-march` arch-extension names to the LLVM feature
/// names understood by zig's `-mcpu` parser, e.g. `fp16` -> `fullfp16`
///
/// The mapping comes from the `ExtensionWithMArch` definitions in
/// https://github.com/llvm/llvm-project/blob/main/llvm/lib/Target/AArch64/AArch64Features.td
fn map_aarch64_arch_extension(ext: &str) -> &str {
    match ext {
        "fp" => "fp-armv8",
        "fp16" => "fullfp16",
        "simd" => "neon",
        "profile" => "spe",
        "rng" => "rand",
        "memtag" => "mte",
        "pmuv3" => "perfmon",
        "jscvt" => "jsconv",
        "fcma" => "complxnum",
        "predres2" => "specres2",
        _ => ext,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "-Wl,--fix-cortex-a53-843419",
            "-Wl,-plugin-opt=O2",
        ];
        for arg in removed {
            let result = run_filter_one(arg, linux, (13, 0));
            assert!(result.is_empty(), "{arg} should be removed");
        }
    }

    #[test]
    fn test_filter_undefined_arg() {
        let linux = Some("x86_64-unknown-linux-gnu");
        // `--undefined=SYM` is rewritten to the `-u SYM` synonym zig supports
        let result = run_filter_one("-Wl,--undefined=AUDITABLE_VERSION_INFO", linux, (13, 0));
        assert_eq!(result, vec!["-Wl,-u,AUDITABLE_VERSION_INFO"]);
        // macOS `-undefined dynamic_lookup` (single dash) must not match
        let result = run_filter_one(
            "-Wl,-undefined=dynamic_lookup",
            Some("aarch64-apple-darwin"),
            (13, 0),
        );
        assert_eq!(result, vec!["-Wl,-undefined=dynamic_lookup"]);
    }

    #[test]
    fn test_filter_wp_args() {
        let linux = Some("x86_64-unknown-linux-gnu");
        // Unsupported -Wp, args should be removed
        for arg in &[
            "-Wp,-U_FORTIFY_SOURCE",
            "-Wp,-DFOO=1",
            "-Wp,-MF,/tmp/t.d",
            "-Wp,-MQ,foo",
            "-Wp,-MP",
        ] {
            let result = run_filter_one(arg, linux, (13, 0));
            assert!(result.is_empty(), "{arg} should be removed");
        }
        // Supported -Wp, args should be kept (-MD, -MMD, -MT)
        for arg in &["-Wp,-MD,/tmp/test.d", "-Wp,-MMD,/tmp/test.d", "-Wp,-MT,foo"] {
            let result = run_filter_one(arg, linux, (13, 0));
            assert_eq!(result, vec![*arg], "{arg} should be kept");
        }
        // bare -U and -D should be kept (zig cc supports them directly)
        let result = run_filter_one("-U_FORTIFY_SOURCE", linux, (13, 0));
        assert_eq!(result, vec!["-U_FORTIFY_SOURCE"]);
        let result = run_filter_one("-DFOO=1", linux, (13, 0));
        assert_eq!(result, vec!["-DFOO=1"]);
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
            // aarch64: GCC/clang extension names mapped to LLVM feature names
            // https://github.com/rust-cross/cargo-zigbuild/issues/456
            (
                "-march=armv8.2-a+sve+fp16",
                "aarch64-unknown-linux-musl",
                &["-mcpu=generic+sve+fullfp16"],
            ),
            (
                "-march=armv8.2-a+simd+profile+rng+memtag",
                "aarch64-unknown-linux-gnu",
                &["-mcpu=generic+neon+spe+rand+mte"],
            ),
            // aarch64: dashed feature names use underscores in zig,
            // where `-` would mean feature subtraction
            (
                "-march=armv9-a+sve2-aes+fp",
                "aarch64-unknown-linux-gnu",
                &["-mcpu=generic+sve2_aes+fp_armv8"],
            ),
            // aarch64: `+no<ext>` disables a feature
            (
                "-march=armv8.2-a+nofp16+nosimd",
                "aarch64-unknown-linux-gnu",
                &["-mcpu=generic-fullfp16-neon"],
            ),
            // x86-64: -march values are CPU names, spelled with underscores in zig
            (
                "-march=x86-64-v3",
                "x86_64-unknown-linux-gnu",
                &["-march=x86_64_v3"],
            ),
            (
                "-march=haswell",
                "x86_64-unknown-linux-gnu",
                &["-march=haswell"],
            ),
        ];
        for (input, target, expected) in cases {
            let result = run_filter_one(input, Some(target), (13, 0));
            assert_eq!(&result, expected, "filter({input}, {target})");
        }
    }

    #[test]
    fn test_filter_mcpu_mtune_args() {
        // (input, target, expected)
        let cases: &[(&str, &str, &[&str])] = &[
            // zig spells CPU names with underscores
            (
                "-mcpu=cortex-a53",
                "aarch64-unknown-linux-gnu",
                &["-mcpu=cortex_a53"],
            ),
            (
                "-mcpu=cortex-a7",
                "arm-unknown-linux-gnueabihf",
                &["-mcpu=cortex_a7"],
            ),
            (
                "-mtune=x86-64-v3",
                "x86_64-unknown-linux-gnu",
                &["-mtune=x86_64_v3"],
            ),
            // aarch64 -mcpu arch extensions are mapped like -march ones
            (
                "-mcpu=cortex-a53+crypto+nofp16",
                "aarch64-unknown-linux-gnu",
                &["-mcpu=cortex_a53+crypto-fullfp16"],
            ),
            // powerpc CPU names use the LLVM spellings
            (
                "-mcpu=power9",
                "powerpc64le-unknown-linux-gnu",
                &["-mcpu=pwr9"],
            ),
            (
                "-mcpu=powerpc64le",
                "powerpc64le-unknown-linux-gnu",
                &["-mcpu=ppc64le"],
            ),
            // zig-spelled values pass through unchanged: `-` after a `+` is
            // zig's feature subtraction, and s390x specs like z10-vector are
            // never rewritten
            (
                "-mcpu=generic+v6+strict_align+vfp2-d32",
                "arm-unknown-linux-gnueabihf",
                &["-mcpu=generic+v6+strict_align+vfp2-d32"],
            ),
            (
                "-mcpu=z10-vector",
                "s390x-unknown-linux-gnu",
                &["-mcpu=z10-vector"],
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
    fn test_dedup_apple_duplicate_libs() {
        // See https://github.com/rust-cross/cargo-zigbuild/issues/457
        let darwin = Some("aarch64-apple-darwin");
        let result = run_filter(
            &[
                "-lobjc",
                "-framework",
                "AppKit",
                "-lobjc",
                "-liconv",
                "-lobjc",
                "main.o",
            ],
            darwin,
            (16, 0),
        );
        assert_eq!(
            result,
            vec!["-lobjc", "-framework", "AppKit", "-liconv", "main.o"]
        );
        // duplicates are kept on non-Apple targets where link order can matter
        let result = run_filter(
            &["-lfoo", "-lfoo"],
            Some("x86_64-unknown-linux-gnu"),
            (16, 0),
        );
        assert_eq!(result, vec!["-lfoo", "-lfoo"]);
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
