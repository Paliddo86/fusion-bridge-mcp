use std::{env, path::PathBuf};
fn main() {
    println!("cargo:rerun-if-env-changed=FUSION_CPP_SDK");
    for p in ["shim/session.cpp", "shim/session.h"] {
        println!("cargo:rerun-if-changed={p}");
    }
    if env::var_os("CARGO_FEATURE_NATIVE_SDK").is_none() {
        return;
    }
    assert_eq!(env::var("TARGET").unwrap(), "x86_64-pc-windows-msvc");
    let sdk = PathBuf::from(env::var_os("FUSION_CPP_SDK").expect("installed SDK required"));
    cc::Build::new()
        .cpp(true)
        .file("shim/session.cpp")
        .include(sdk.join("include"))
        .std("c++17")
        .flag("/EHsc")
        .compile("session_sdk");
    println!(
        "cargo:rustc-link-search=native={}",
        sdk.join("lib").display()
    );
    println!("cargo:rustc-link-lib=core");
    println!("cargo:rustc-link-lib=fusion");
}
