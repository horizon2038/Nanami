mod build;
mod cli;
mod cpio;
mod payload;
mod qemu;
mod rootfs;
mod terminfo;
mod util;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{Cli, Task};
use std::path::Path;

#[cfg(test)]
extern crate self as nun;
#[cfg(test)]
#[derive(Debug)]
pub enum CapabilityError {
    InvalidArgument,
}

fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("xtask repository root")?;
    let cli = Cli::parse();
    let common = match &cli.task {
        Task::Build(a) | Task::Servers(a) | Task::Rootfs(a) => a,
        Task::Run(a) => &a.common,
    };
    if common.offline {
        std::env::set_var("CARGO_NET_OFFLINE", "true");
    }
    match cli.task {
        Task::Build(args) => {
            build::image(root, &args)?;
        }
        Task::Servers(args) => {
            build::servers(root, &args)?;
        }
        Task::Rootfs(args) => {
            rootfs::create(root, &args)?;
        }
        Task::Run(args) => {
            // Validate the launch plan before performing any requested build.
            qemu::validate(&args)?;
            if args.build {
                build::image(root, &args.common)?;
            }
            qemu::run(root, &args)?;
        }
    }
    Ok(())
}
