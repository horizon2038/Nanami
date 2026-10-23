fn main() {
    // Cargo does not discover scripts referenced only through the target JSON.
    // Relink Alpha when its address layout changes, not only when Rust changes.
    if std::env::var_os("CARGO_FEATURE_ALPHA_ENTRY").is_some() {
        let architecture = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
        println!("cargo:rerun-if-changed=../targets/{architecture}/alpha.ld");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
