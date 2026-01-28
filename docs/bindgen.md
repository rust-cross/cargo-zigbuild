# Using Bindgen with Cargo Zigbuild

The target you pass to `cargo zigbuild` is passed to your Rust program via the `TARGET` environmental variable. This can be accessed like so:

```rs
let target = env::var("TARGET").unwrap();
```

Which allows you to integrate with [`bindgen`](https://docs.rs/bindgen/latest/bindgen/) and/or [`cc`](https://docs.rs/cc/latest/cc/) in a cohesive way.

An example of using this to build related C code can be demonstrated like so:

```c
// c-src/example.c
int get_num() {
    return 42;
}
```

```h
// example.h
int get_num();
```

```rs
// build.rs
use std::{env, path::PathBuf};

fn main() {
    let target = env::var("TARGET").unwrap();

    // Re-run this build script if the C source or header changes
    println!("cargo:rerun-if-changed=c-src/example.c");
    println!("cargo:rerun-if-changed=c-src/example.h");

    cc::Build::new().file("c-src/example.c").compile("example");

    let bindings = bindgen::builder()
        .header("c-src/example.h")
        .use_core()
        .clang_arg(format!("--target={target}"))
        .generate().unwrap();

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings.write_to_file(out_path.join("example.rs")).unwrap();
}
```

```rs
// src/lib.rs
#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

include!(concat!(env!("OUT_DIR"), "/example.rs"));
```

```rs
// src/main.rs
use your_project::{get_num};

fn main() {
    println!("The number is: {}", unsafe { get_num() });
}
```