//! Rustflags extraction from cargo's `--config` CLI option.
//!
//! Cargo accepts arbitrary configuration overrides via `--config KEY=VALUE`
//! (TOML syntax) or `--config <path>.toml`. cargo-zigbuild forwards these to
//! the child cargo process, but `cargo_config2::Config::load()` only reads
//! config files and environment variables, so `target-cpu` set via `--config`
//! would silently not be reflected in the `-mcpu` passed to `zig cc`. This
//! module parses the `--config` arguments so that rustflags provided this way
//! participate in the resolution used for zig's `-mcpu`.
//!
//! Once cargo-config2 natively supports `--config` CLI overrides
//! (<https://github.com/taiki-e/cargo-config2/issues/3>), most of this module
//! (in particular the tier heuristic in [`CliConfig::overlay`]) can be
//! replaced with that API.

use std::collections::BTreeMap;
use std::env;

use anyhow::{Context, Result};
use cargo_config2::Flags;

/// Rustflags provided via cargo's `--config` CLI option.
///
/// Cargo merges `--config` values in left-to-right order with the same logic
/// used for config files: arrays are joined with higher precedence items
/// placed later. CLI values take precedence over config files and config
/// environment variables (`CARGO_TARGET_<T>_RUSTFLAGS`, `CARGO_BUILD_RUSTFLAGS`),
/// but not over the `RUSTFLAGS`/`CARGO_ENCODED_RUSTFLAGS` environment
/// variables, which shortcut the rustflags resolution entirely.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CliConfig {
    build_rustflags: Option<Flags>,
    target_rustflags: BTreeMap<String, Flags>,
}

impl CliConfig {
    /// Parses cargo `--config` arguments (`KEY=VALUE` in TOML syntax, or a
    /// path to an extra config file ending in `.toml`).
    ///
    /// `target.'cfg(...)'` keys are ignored because evaluating cfg
    /// expressions requires querying rustc; cargo still applies them to the
    /// actual build, they just don't influence zig's `-mcpu`.
    pub fn parse(config_args: &[String]) -> Result<Self> {
        let mut parsed = Self::default();
        for arg in config_args {
            // Cargo treats an argument ending in `.toml` as a path to an
            // extra config file; anything else must be TOML `KEY=VALUE`.
            let config: cargo_config2::de::Config = if arg.ends_with(".toml") {
                cargo_config2::de::Config::load_file(arg)?
            } else {
                toml::from_str(arg)
                    .with_context(|| format!("failed to parse --config argument `{arg}`"))?
            };
            if let Some(flags) = &config.build.rustflags {
                append_de_flags(parsed.build_rustflags.get_or_insert_default(), flags);
            }
            for (key, target_config) in &config.target {
                if key.starts_with("cfg(") {
                    continue;
                }
                if let Some(flags) = &target_config.rustflags {
                    append_de_flags(
                        parsed.target_rustflags.entry(key.clone()).or_default(),
                        flags,
                    );
                }
            }
        }
        Ok(parsed)
    }

    /// Resolves the effective rustflags for `rust_target`, overlaying the
    /// CLI-provided values onto the config file/environment resolution.
    ///
    /// Cargo resolves rustflags from four mutually exclusive sources, in
    /// order, using the first one that is set:
    ///
    /// 1. `CARGO_ENCODED_RUSTFLAGS` environment variable
    /// 2. `RUSTFLAGS` environment variable
    /// 3. all matching `target.<triple>.rustflags` and `target.<cfg>.rustflags`
    ///    config entries joined together
    /// 4. `build.rustflags` config value
    ///
    /// `--config` values participate in sources 3 and 4 with the highest
    /// precedence within those sources (joined last).
    pub fn rustflags(
        &self,
        cargo_config: &cargo_config2::Config,
        rust_target: &str,
    ) -> Result<Option<Flags>> {
        let resolved = cargo_config.rustflags(rust_target)?;
        if env::var_os("CARGO_ENCODED_RUSTFLAGS").is_some() || env::var_os("RUSTFLAGS").is_some() {
            // Environment rustflags win over all config values, including CLI.
            return Ok(resolved);
        }
        Ok(self.overlay(rust_target, resolved, cargo_config.build.rustflags.clone()))
    }

    /// Pure overlay of the CLI-provided rustflags onto the resolved config
    /// values.
    ///
    /// `resolved` is the config file/environment resolution for the target
    /// (source 3 if any target entries matched, source 4 otherwise) and
    /// `build_flags` is the resolved `build.rustflags`. cargo_config2 does
    /// not expose which source `resolved` came from, so when it equals
    /// `build_flags` we assume it came from `build.rustflags`.
    fn overlay(
        &self,
        rust_target: &str,
        resolved: Option<Flags>,
        build_flags: Option<Flags>,
    ) -> Option<Flags> {
        let from_build_tier = resolved == build_flags;
        if let Some(cli_target) = self.target_rustflags.get(rust_target) {
            // CLI target flags activate source 3: join file/env target
            // entries (if any) with the CLI entries placed last;
            // `build.rustflags` no longer applies.
            let mut flags = if from_build_tier {
                Flags::default()
            } else {
                resolved.unwrap_or_default()
            };
            flags.flags.extend(cli_target.flags.iter().cloned());
            Some(flags)
        } else if let Some(cli_build) = &self.build_rustflags {
            if from_build_tier {
                let mut flags = resolved.unwrap_or_default();
                flags.flags.extend(cli_build.flags.iter().cloned());
                Some(flags)
            } else {
                // A target.<triple>.rustflags entry matched; source 3 wins
                // and build.rustflags (including the CLI value) is ignored.
                resolved
            }
        } else {
            resolved
        }
    }
}

fn append_de_flags(flags: &mut Flags, de_flags: &cargo_config2::de::Flags) {
    flags
        .flags
        .extend(de_flags.flags.iter().map(|value| value.val.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const TARGET: &str = "x86_64-unknown-linux-gnu";

    fn flags(s: &str) -> Flags {
        Flags::from_space_separated(s)
    }

    #[test]
    fn test_parse_target_rustflags_array() {
        let config = CliConfig::parse(&[format!(
            "target.{TARGET}.rustflags=['-C','target-cpu=x86-64-v4']"
        )])
        .unwrap();
        assert_eq!(
            config.target_rustflags[TARGET].flags,
            vec!["-C", "target-cpu=x86-64-v4"]
        );
        assert!(config.build_rustflags.is_none());
    }

    #[test]
    fn test_parse_target_rustflags_string() {
        let config = CliConfig::parse(&[format!(
            "target.{TARGET}.rustflags='-C target-cpu=x86-64-v4'"
        )])
        .unwrap();
        assert_eq!(
            config.target_rustflags[TARGET].flags,
            vec!["-C", "target-cpu=x86-64-v4"]
        );
    }

    #[test]
    fn test_parse_build_rustflags() {
        let config =
            CliConfig::parse(&["build.rustflags=['-Ctarget-cpu=neoverse-n1']".to_string()])
                .unwrap();
        assert_eq!(
            config.build_rustflags.unwrap().flags,
            vec!["-Ctarget-cpu=neoverse-n1"]
        );
    }

    #[test]
    fn test_parse_multiple_args_join_left_to_right() {
        let config = CliConfig::parse(&[
            format!("target.{TARGET}.rustflags=['-Ctarget-cpu=x86-64-v2']"),
            format!("target.{TARGET}.rustflags=['-Ctarget-cpu=x86-64-v4']"),
        ])
        .unwrap();
        // Arrays are joined with later (higher precedence) items placed last.
        assert_eq!(
            config.target_rustflags[TARGET].flags,
            vec!["-Ctarget-cpu=x86-64-v2", "-Ctarget-cpu=x86-64-v4"]
        );
    }

    #[test]
    fn test_parse_config_file() {
        let mut file = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
        writeln!(
            file,
            "[target.{TARGET}]\nrustflags = ['-C', 'target-cpu=x86-64-v3']"
        )
        .unwrap();
        let config = CliConfig::parse(&[file.path().to_str().unwrap().to_string()]).unwrap();
        assert_eq!(
            config.target_rustflags[TARGET].flags,
            vec!["-C", "target-cpu=x86-64-v3"]
        );
    }

    #[test]
    fn test_parse_ignores_unrelated_and_cfg_keys() {
        let config = CliConfig::parse(&[
            "net.git-fetch-with-cli=true".to_string(),
            "profile.release.lto=true".to_string(),
            "target.'cfg(target_arch = \"x86_64\")'.rustflags=['-Ctarget-cpu=x86-64-v4']"
                .to_string(),
        ])
        .unwrap();
        assert_eq!(config, CliConfig::default());
    }

    #[test]
    fn test_overlay_cli_target_replaces_build_tier() {
        let config = CliConfig::parse(&[format!(
            "target.{TARGET}.rustflags=['-Ctarget-cpu=x86-64-v4']"
        )])
        .unwrap();
        // `resolved` came from build.rustflags: CLI target flags activate
        // source 3 and build.rustflags no longer applies.
        let result = config.overlay(
            TARGET,
            Some(flags("-Ctarget-cpu=x86-64-v2")),
            Some(flags("-Ctarget-cpu=x86-64-v2")),
        );
        assert_eq!(result.unwrap().flags, vec!["-Ctarget-cpu=x86-64-v4"]);
    }

    #[test]
    fn test_overlay_cli_target_joins_file_target_tier() {
        let config = CliConfig::parse(&[format!(
            "target.{TARGET}.rustflags=['-Ctarget-cpu=x86-64-v4']"
        )])
        .unwrap();
        // `resolved` came from target.<triple>.rustflags in a config file:
        // entries are joined with CLI values last, so the CLI target-cpu wins.
        let result = config.overlay(TARGET, Some(flags("-Ctarget-cpu=x86-64-v2")), None);
        assert_eq!(
            result.unwrap().flags,
            vec!["-Ctarget-cpu=x86-64-v2", "-Ctarget-cpu=x86-64-v4"]
        );
    }

    #[test]
    fn test_overlay_cli_target_other_triple_is_ignored() {
        let config = CliConfig::parse(&[
            "target.aarch64-unknown-linux-gnu.rustflags=['-Ctarget-cpu=neoverse-n1']".to_string(),
        ])
        .unwrap();
        let result = config.overlay(TARGET, None, None);
        assert_eq!(result, None);
    }

    #[test]
    fn test_overlay_cli_build_joins_build_tier() {
        let config =
            CliConfig::parse(&["build.rustflags=['-Ctarget-cpu=x86-64-v4']".to_string()]).unwrap();
        let result = config.overlay(
            TARGET,
            Some(flags("-Ctarget-cpu=x86-64-v2")),
            Some(flags("-Ctarget-cpu=x86-64-v2")),
        );
        assert_eq!(
            result.unwrap().flags,
            vec!["-Ctarget-cpu=x86-64-v2", "-Ctarget-cpu=x86-64-v4"]
        );
    }

    #[test]
    fn test_overlay_cli_build_ignored_when_target_tier_present() {
        let config =
            CliConfig::parse(&["build.rustflags=['-Ctarget-cpu=x86-64-v4']".to_string()]).unwrap();
        // `resolved` differs from build.rustflags, so it came from a
        // target.<triple>.rustflags entry, which wins over build.rustflags.
        let result = config.overlay(TARGET, Some(flags("-Ctarget-cpu=x86-64-v2")), None);
        assert_eq!(result.unwrap().flags, vec!["-Ctarget-cpu=x86-64-v2"]);
    }

    #[test]
    fn test_overlay_no_cli_flags_keeps_resolved() {
        let config = CliConfig::default();
        let result = config.overlay(TARGET, Some(flags("-Ctarget-cpu=x86-64-v2")), None);
        assert_eq!(result.unwrap().flags, vec!["-Ctarget-cpu=x86-64-v2"]);
    }
}
