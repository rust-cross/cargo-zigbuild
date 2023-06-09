#![allow(
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    dead_code,
    improper_ctypes
)]

mod c {
    include!(concat!(env!("OUT_DIR"), "/c.rs"));

    // verify that sysdep.h generates specific types
    const _: () = {
        #[cfg(target_os = "linux")]
        zigbuild_is_linux { x: 0 };

        #[cfg(target_os = "windows")]
        zigbuild_is_win32 { x: 0 };

        #[cfg(target_os = "macos")]
        zigbuild_is_macos { x: 0 };
    };
}

mod cpp {
    include!(concat!(env!("OUT_DIR"), "/cpp.rs"));

    // verify that sysdep.h generates specific types
    const _: () = {
        #[cfg(target_os = "linux")]
        zigbuild_is_linux { x: 0 };

        #[cfg(target_os = "windows")]
        zigbuild_is_win32 { x: 0 };

        #[cfg(target_os = "macos")]
        zigbuild_is_macos { x: 0 };
    };
}
