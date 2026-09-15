fn main() {
    println!("cargo:rustc-check-cfg=cfg(legacy_heap)");
    for (variable, default) in [
        (
            "NANAMI_HEAP_SOURCE",
            "../../nanami/servers/sdk/rust/libnanami/src/heap.rs",
        ),
        (
            "NANAMI_PROCESS_SOURCE",
            "../../nanami/src/nanami_core/process.rs",
        ),
        (
            "NANAMI_TIMER_SOURCE",
            "../../nanami/servers/core-services/timer-server/src/timers.rs",
        ),
    ] {
        println!("cargo:rerun-if-env-changed={variable}");
        let source = std::env::var(variable).unwrap_or_else(|_| default.into());
        let source = std::fs::canonicalize(source).expect("test source must exist");
        println!("cargo:rerun-if-changed={}", source.display());
        println!("cargo:rustc-env={variable}={}", source.display());
    }
}
