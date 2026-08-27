pub(crate) struct TargetInfo {
    target: Option<String>,
}

impl TargetInfo {
    pub(crate) fn new(target: Option<&String>) -> Self {
        Self {
            target: target.cloned(),
        }
    }

    // Architecture helpers
    pub(crate) fn is_arm(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.starts_with("arm"))
            .unwrap_or_default()
    }

    pub(crate) fn is_aarch64(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.starts_with("aarch64"))
            .unwrap_or_default()
    }

    pub(crate) fn is_aarch64_be(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.starts_with("aarch64_be"))
            .unwrap_or_default()
    }

    pub(crate) fn is_i386(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.starts_with("i386"))
            .unwrap_or_default()
    }

    pub(crate) fn is_i686(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.starts_with("i686") || x.starts_with("x86-"))
            .unwrap_or_default()
    }

    pub(crate) fn is_riscv64(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.starts_with("riscv64"))
            .unwrap_or_default()
    }

    pub(crate) fn is_riscv32(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.starts_with("riscv32"))
            .unwrap_or_default()
    }

    pub(crate) fn is_mips32(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.starts_with("mips") && !x.starts_with("mips64"))
            .unwrap_or_default()
    }

    pub(crate) fn is_powerpc(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.starts_with("powerpc"))
            .unwrap_or_default()
    }

    pub(crate) fn is_s390x(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.starts_with("s390x"))
            .unwrap_or_default()
    }

    // libc helpers
    pub(crate) fn is_musl(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("musl"))
            .unwrap_or_default()
    }

    // Platform helpers
    pub(crate) fn is_macos(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("macos") || x.contains("maccatalyst"))
            .unwrap_or_default()
    }

    pub(crate) fn is_darwin(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("darwin"))
            .unwrap_or_default()
    }

    pub(crate) fn is_apple_platform(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| {
                x.contains("macos")
                    || x.contains("darwin")
                    || x.contains("ios")
                    || x.contains("tvos")
                    || x.contains("watchos")
                    || x.contains("visionos")
                    || x.contains("maccatalyst")
            })
            .unwrap_or_default()
    }

    pub(crate) fn is_ios(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("ios") && !x.contains("visionos"))
            .unwrap_or_default()
    }

    pub(crate) fn is_tvos(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("tvos"))
            .unwrap_or_default()
    }

    pub(crate) fn is_watchos(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("watchos"))
            .unwrap_or_default()
    }

    pub(crate) fn is_visionos(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("visionos"))
            .unwrap_or_default()
    }

    /// Returns the appropriate Apple CPU for the platform
    pub(crate) fn apple_cpu(&self) -> &'static str {
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

    pub(crate) fn is_freebsd(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("freebsd"))
            .unwrap_or_default()
    }

    pub(crate) fn is_windows_gnu(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("windows-gnu"))
            .unwrap_or_default()
    }

    pub(crate) fn is_windows_msvc(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("windows-msvc"))
            .unwrap_or_default()
    }

    pub(crate) fn is_ohos(&self) -> bool {
        self.target
            .as_ref()
            .map(|x| x.contains("ohos"))
            .unwrap_or_default()
    }
}
