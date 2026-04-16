use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=src/ffi/engine.h");
    println!("cargo:rerun-if-env-changed=LITERT_LM_LIB_PATH");

    let bindings = bindgen::Builder::default()
        .header("src/ffi/engine.h")
        .allowlist_function("litert_lm_.*")
        .allowlist_type("LiteRtLm.*")
        .allowlist_type("InputData.*")
        .allowlist_var("kInput.*")
        .generate_comments(true)
        .generate()
        .expect("Unable to generate bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings");

    println!("cargo:rustc-link-lib=dylib=engine");

    if let Ok(p) = env::var("LITERT_LM_LIB_PATH") {
        println!("cargo:rustc-link-search=native={p}");
        // Bake the path into the binary's RPATH so LD_LIBRARY_PATH isn't needed at runtime.
        println!("cargo:rustc-link-arg=-Wl,-rpath,{p}");
    }

    #[cfg(target_os = "macos")]
    println!("cargo:rustc-link-lib=dylib=c++");

    #[cfg(target_os = "linux")]
    println!("cargo:rustc-link-lib=dylib=stdc++");
}
