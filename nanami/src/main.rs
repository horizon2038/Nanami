#![no_std]
#![no_main]

extern crate alloc;

mod alpha_booter;
#[allow(dead_code)] // Booter links only the shared launch/ELF/capability mechanisms.
mod nanami_core;
#[allow(dead_code)]
mod nanami_utils;

use nun::InitInfo;

nun::entry!(main);

fn main(init_info: &InitInfo) {
    crate::force_info!(r#" _   _                             _ "#);
    crate::force_info!(r#"| \ | | __ _ _ __   __ _ _ __ ___ (_)"#);
    crate::force_info!(r#"|  \| |/ _` | '_ \ / _` | '_ ` _ \| |"#);
    crate::force_info!(r#"| |\  | (_| | | | | (_| | | | | | | |"#);
    crate::force_info!(r#"|_| \_|\__,_|_| |_|\__,_|_| |_| |_|_|"#);
    crate::force_info!("");

    crate::force_info!("Nanami v{}", env!("CARGO_PKG_VERSION"));
    crate::force_info!(
        "Kernel: A9N v{}.{}.{}-{}+{}",
        init_info.kernel_major_version,
        init_info.kernel_minor_version,
        init_info.kernel_patch_version,
        init_info.get_pre_release_string(),
        init_info.get_build_metadata_string()
    );
    crate::force_info!(
        "Environment: architecture={} platform={} cores={}",
        init_info.get_architecture_name_string(),
        init_info.get_platform_name_string(),
        init_info.core_count
    );

    alpha_booter::boot(init_info);
}
