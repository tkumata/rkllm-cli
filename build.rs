use std::env;
use std::path::PathBuf;

fn main() {
    // Get the absolute path to src/lib
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let lib_path = PathBuf::from(&manifest_dir).join("src/lib");
    let lib_path_str = lib_path.to_str().unwrap();

    // Tell cargo to look for shared libraries in src/lib
    println!("cargo:rustc-link-search=native={}", lib_path_str);

    // Tell cargo to link the rkllmrt library
    println!("cargo:rustc-link-lib=dylib=rkllmrt");

    // Set RPATH so the binary can find the library at runtime
    // This embeds the library path into the binary
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_path_str);

    // Tell cargo to rerun this build script if the library changes
    println!("cargo:rerun-if-changed=src/lib/librkllmrt.so");
}
