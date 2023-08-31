/// arm-features.h
pub static ARM_FEATURES_H: &str = include_str!("arm-features.h");

// Fix glibc undefined symbol fcntl64 error

// fcntl.map
pub static FCNTL_MAP: &str = r#"
GLIBC_2.2.5 {
    fcntl;
};
"#;

// fnctl.h shim
pub static FCNTL_H: &str = r#"
#ifdef __ASSEMBLER__
.symver fcntl64, fcntl@GLIBC_2.2.5
#else
__asm__(".symver fcntl64, fcntl@GLIBC_2.2.5");
#endif
"#;

pub static MUSL_WEAK_SYMBOLS_MAPPING_SCRIPT: &str = r#"
PROVIDE (open64 = open);
PROVIDE (stat64 = stat);
PROVIDE (fstat64 = fstat);
PROVIDE (lseek64 = lseek);
"#;
