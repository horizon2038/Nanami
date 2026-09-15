fn main() {
    println!("cargo:rerun-if-env-changed=NANAMI_HEAP_SOURCE");
    println!("cargo:rustc-check-cfg=cfg(legacy_heap)");
    let source = std::env::var("NANAMI_HEAP_SOURCE")
        .unwrap_or_else(|_| "../../nanami/servers/sdk/rust/libnanami/src/heap.rs".into());
    let source = std::fs::canonicalize(source).expect("heap source must exist");
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rustc-env=NANAMI_HEAP_SOURCE={}", source.display());
}
