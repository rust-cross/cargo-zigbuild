pub mod install_name_tool;
#[cfg(target_os = "macos")]
pub(crate) mod rlimit;

/// libiconv.tbd
pub static LIBICONV_TBD: &str = include_str!("libiconv.tbd");
/// libcharset.tbd
pub static LIBCHARSET_TBD: &str = include_str!("libcharset.1.tbd");
