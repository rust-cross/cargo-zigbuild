use std::{env, path::PathBuf};

fn main() {
    let builder = bindgen::Builder::default()
        .header("src/sysdep.h")
        .header("src/isoc.h");
    builder
        .allowlist_type(r"zigbuild_.*")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks))
        .generate()
        .unwrap()
        .write_to_file(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("c.rs"))
        .unwrap();

    let builder = bindgen::Builder::default()
        .header("src/sysdep.h")
        .header("src/isocpp.hpp");
    builder
        .allowlist_type(r"zigbuild_.*")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks))
        .generate()
        .unwrap()
        .write_to_file(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("cpp.rs"))
        .unwrap();
}
