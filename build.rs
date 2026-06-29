use std::env;

fn main() {
    // Locate the Senzing native library (libSz.so). Default matches the
    // sz-rust-sdk build.rs; override with SENZING_LIB_PATH when the runtime
    // lives elsewhere.
    let senzing_lib_path =
        env::var("SENZING_LIB_PATH").unwrap_or_else(|_| "/opt/senzing/er/lib".to_string());

    println!("cargo:rustc-link-search=native={senzing_lib_path}");
    println!("cargo:rustc-link-lib=dylib=Sz");
    println!("cargo:rustc-env=LD_LIBRARY_PATH={senzing_lib_path}");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=SENZING_LIB_PATH");
}
