use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=FUSION_CPP_SDK");
    println!("cargo:rerun-if-changed=shim/sdk.cpp");
    println!("cargo:rerun-if-changed=shim/sdk.h");
    println!("cargo:rerun-if-changed=shim/mechanical.inc");
    println!("cargo:rerun-if-changed=shim/document_lifecycle.inc");
    println!("cargo:rerun-if-changed=shim/bom.inc");
    println!("cargo:rerun-if-changed=shim/sketch.inc");
    println!("cargo:rerun-if-changed=shim/sketch_geometry.inc");
    println!("cargo:rerun-if-changed=shim/extrusion.inc");
    println!("cargo:rerun-if-changed=shim/materials.inc");
    println!("cargo:rerun-if-changed=shim/purchased.inc");
    println!("cargo:rerun-if-changed=shim/physical.inc");
    println!("cargo:rerun-if-changed=shim/line_state.inc");
    println!("cargo:rerun-if-changed=shim/reference_presence.h");
    println!("cargo:rerun-if-changed=shim/isolated_line_incidence.h");
    println!("cargo:rerun-if-changed=shim/relations.inc");
    println!("cargo:rerun-if-changed=shim/dimensions.inc");
    println!("cargo:rerun-if-changed=shim/assembly.inc");
    println!("cargo:rerun-if-changed=shim/placement.inc");
    println!("cargo:rerun-if-changed=shim/dimension_creator.h");
    println!("cargo:rerun-if-changed=shim/relation_identity.h");
    println!("cargo:rerun-if-changed=shim/events.cpp");
    if env::var_os("CARGO_FEATURE_NATIVE_SDK").is_none() {
        return;
    }
    let target = env::var("TARGET").unwrap();
    assert_eq!(
        target, "x86_64-pc-windows-msvc",
        "This first native probe is qualified for MSVC Windows only"
    );
    let sdk = PathBuf::from(
        env::var_os("FUSION_CPP_SDK")
            .expect("FUSION_CPP_SDK must identify the installed Fusion API/CPP directory"),
    );
    for relative in ["include/Core/Application/Application.h", "lib/core.lib"] {
        assert!(
            sdk.join(relative).is_file(),
            "Missing SDK input: {relative}"
        );
        println!("cargo:rerun-if-changed={}", sdk.join(relative).display());
    }
    println!("cargo:rerun-if-changed={}", sdk.join("include").display());
    cc::Build::new()
        .cpp(true)
        .file("shim/sdk.cpp")
        .file("shim/events.cpp")
        .include(sdk.join("include"))
        .std("c++17")
        .warnings(true)
        .flag("/EHsc")
        .compile("fusion_sdk_adapter");
    println!(
        "cargo:rustc-link-search=native={}",
        sdk.join("lib").display()
    );
    println!("cargo:rustc-link-lib=core");
    println!("cargo:rustc-link-lib=fusion");
}
