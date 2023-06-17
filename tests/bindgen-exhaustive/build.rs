use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=sys/sysdep.h");
    println!("cargo:rerun-if-changed=sys/isoc.h");
    println!("cargo:rerun-if-changed=sys/isocpp.hpp");

    generate(
        bindgen::Builder::default()
            .header("src/sysdep.h")
            .header("src/isoc.h")
            .allowlist_type(r"zigbuild_.*"),
        "c.rs",
    );

    generate(
        bindgen::Builder::default()
            .header("src/sysdep.h")
            .header("src/isocpp.hpp")
            .allowlist_type(r"zigbuild_.*"),
        "cpp.rs",
    );
}

fn generate(builder: bindgen::Builder, filename: &str) {
    // Tip: use `-###` to see cc1 options (`-v` normally prints them, but not in this case)
    let builder = builder
        .clang_arg("-v")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks));

    match builder.generate() {
        Ok(bindings) => {
            bindings
                .write_to_file(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join(filename))
                .unwrap();
        }
        Err(e) => {
            eprintln!();

            // print (lightly formatted) BINDGEN_EXTRA_CLANG_ARGS for diagnostics
            let target = env::var("TARGET").unwrap().replace("-", "_");
            let bindgen_env = format!("BINDGEN_EXTRA_CLANG_ARGS_{target}");
            if let Ok(value) = env::var(&bindgen_env) {
                eprintln!("{bindgen_env}:");
                for arg in value.split_whitespace() {
                    eprintln!("  {arg}");
                }
                eprintln!();
            }

            if let bindgen::BindgenError::ClangDiagnostic(s) = e {
                // `cargo build` will print stderr anyway, let's cut it
                panic!(
                    "Failed to invoke clang (see above for the full message):\n{:.500}",
                    s
                );
            } else {
                panic!("{}", e);
            }
        }
    }
}
