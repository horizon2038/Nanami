use crate::{
    cli::{Arch, Network, Platform, Run, Storage},
    rootfs, util,
};
use anyhow::{ensure, Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Host {
    Mac,
    Linux,
    Other,
}
fn host() -> Host {
    if cfg!(target_os = "macos") {
        Host::Mac
    } else if cfg!(target_os = "linux") {
        Host::Linux
    } else {
        Host::Other
    }
}

pub fn validate(args: &Run) -> Result<()> {
    ensure!(
        args.common.platform()? != Platform::Rpi4b,
        "Raspberry Pi images must be booted on hardware"
    );
    if args.common.arch == Arch::Aarch64 {
        ensure!(
            args.network() == Network::None,
            "AArch64 has no native network driver yet; use --net none"
        );
    }
    if args.network() == Network::Bridged {
        ensure!(
            args.bridge_if
                .as_ref()
                .is_some_and(|s| !s.is_empty() && !s.contains(',')),
            "bridged networking requires --bridge-if / BRIDGE_IF"
        );
    }
    ensure!(
        matches!(args.accel.as_str(), "auto" | "none" | "tcg" | "kvm" | "hvf"),
        "unsupported accelerator (auto, none, tcg, kvm, hvf)"
    );
    ensure!(!args.sudo || cfg!(unix), "--sudo requires a Unix host");
    Ok(())
}

fn value(path: &Path) -> Result<String> {
    // QEMU's option parser treats commas as separators; doubled commas are literal.
    Ok(util::path_text(path)?.replace(',', ",,"))
}

fn accelerator(args: &Run, host: Host) -> String {
    if args.accel != "auto" {
        return args.accel.clone();
    }
    let native = args.common.arch.name() == std::env::consts::ARCH;
    match host {
        Host::Linux if native && Path::new("/dev/kvm").exists() => "kvm".into(),
        Host::Mac if native && args.common.arch == Arch::X86_64 => "hvf".into(),
        _ => "tcg".into(),
    }
}

fn network(args: &Run, host: Host, out: &mut Vec<String>) -> Result<()> {
    let netdev = match args.network() {
        Network::None => {
            out.extend(["-net".into(), "none".into()]);
            return Ok(());
        }
        Network::User => format!("user,id=net0,hostfwd={}", args.hostfwd),
        Network::Bridged => {
            let interface = args.bridge_if.as_deref().context("bridge interface")?;
            match host {
                Host::Mac => format!("vmnet-bridged,id=net0,ifname={interface}"),
                Host::Linux => format!("bridge,id=net0,br={interface}"),
                Host::Other => anyhow::bail!("bridged networking is unsupported on this host"),
            }
        }
    };
    out.extend([
        "-netdev".into(),
        netdev,
        "-device".into(),
        "virtio-net-pci,netdev=net0,addr=5,disable-legacy=off,disable-modern=on".into(),
    ]);
    if let Some(pcap) = &args.pcap {
        out.extend([
            "-object".into(),
            format!("filter-dump,id=net0-dump,netdev=net0,file={}", value(pcap)?),
        ]);
    }
    Ok(())
}

fn out_dir(root: &Path, args: &Run) -> Result<PathBuf> {
    Ok(root.join(format!(
        "spencer/out/{}-{}-{}",
        args.common.arch.name(),
        args.common.platform()?.name(),
        args.common.profile.name()
    )))
}

fn boot_image(root: &Path, args: &Run) -> Result<PathBuf> {
    Ok(args
        .image
        .as_ref()
        .or(args.common.output_image.as_ref())
        .map(|p| util::absolute(root, p))
        .unwrap_or(out_dir(root, args)?.join("spencer.img")))
}

fn arguments(root: &Path, args: &Run, host: Host) -> Result<Vec<String>> {
    validate(args)?;
    let out = out_dir(root, args)?;
    let image_path = boot_image(root, args)?;
    let image = value(&image_path)?;
    let accel = accelerator(args, host);
    let cpu = args.cpu.clone().unwrap_or_else(|| {
        if matches!(accel.as_str(), "kvm" | "hvf") {
            "host".into()
        } else if args.common.arch == Arch::X86_64 {
            "max".into()
        } else {
            "cortex-a72".into()
        }
    });
    let mut result = vec![
        "-m".into(),
        args.memory.clone(),
        "-cpu".into(),
        cpu,
        "-smp".into(),
        args.smp.to_string(),
    ];
    if accel != "none" {
        result.extend([
            "-accel".into(),
            if accel == "tcg" {
                "tcg,thread=multi".into()
            } else {
                accel
            },
        ]);
    }
    if args.common.arch == Arch::X86_64 {
        result.extend([
            "-machine".into(),
            format!("hpet={}", if args.no_hpet { "off" } else { "on" }),
            "-serial".into(),
            "mon:stdio".into(),
            "-drive".into(),
            format!(
                "if=pflash,format=raw,readonly=on,file={}",
                value(&root.join("spencer/a9nloader-rs/tools/OVMF_CODE.fd"))?
            ),
            "-drive".into(),
            format!(
                "if=pflash,format=raw,file={}",
                value(&out.join("OVMF_VARS.xtask.fd"))?
            ),
        ]);
        let xhci = !args.no_usb_input || args.storage == Storage::Usb;
        // The controller must precede devices whose bus is xhci.0.
        if xhci {
            result.extend([
                "-fw_cfg".into(),
                "name=opt/ovmf/X-PciMmio64Mb,string=0".into(),
                "-device".into(),
                "qemu-xhci,id=xhci,addr=6,msi=off,msix=off".into(),
            ]);
        }
        result.extend([
            "-drive".into(),
            format!("if=none,id=nanami-disk,format=raw,file={image}"),
        ]);
        match args.storage {
            Storage::Usb => result.extend(["-device".into(), "usb-storage,id=usb-root,drive=nanami-disk,bus=xhci.0,port=3,bootindex=1".into()]),
            Storage::Ahci => result.extend(["-device".into(), "ich9-ahci,id=ahci,addr=3".into(), "-device".into(), "ide-hd,drive=nanami-disk,bus=ahci.0,bootindex=1".into()]),
            Storage::Virtio => result.extend(["-device".into(), "virtio-blk-pci,drive=nanami-disk,addr=3,bootindex=1,disable-legacy=off,disable-modern=on".into()]),
        }
        if !args.no_usb_input {
            result.extend([
                "-device".into(),
                "usb-kbd,id=usb-kbd,bus=xhci.0,port=1".into(),
                "-device".into(),
                "usb-mouse,id=usb-mouse,bus=xhci.0,port=2".into(),
            ]);
        }
        if args.headless {
            result.extend(["-display".into(), "none".into()]);
        }
    } else {
        result.extend([
            "-machine".into(),
            "virt,gic-version=2".into(),
            "-nographic".into(),
            "-bios".into(),
            value(&out.join("u-boot/u-boot.bin"))?,
            "-drive".into(),
            format!("if=none,id=boot,format=raw,file={image}"),
            "-device".into(),
            "virtio-blk-pci,drive=boot".into(),
            "-drive".into(),
            format!(
                "if=none,id=blk0,format=raw,file={}",
                value(&rootfs::path(root, &args.common))?
            ),
            "-device".into(),
            "virtio-blk-device,drive=blk0".into(),
            "-global".into(),
            "virtio-mmio.force-legacy=false".into(),
        ]);
    }
    network(args, host, &mut result)?;
    if args.snapshot {
        result.push("-snapshot".into());
    }
    if args.gdb {
        result.extend(["-s".into(), "-S".into()]);
    }
    result.extend(args.extra.iter().cloned());
    Ok(result)
}

pub fn run(root: &Path, args: &Run) -> Result<()> {
    let argv = arguments(root, args, host())?;
    let qemu = args
        .qemu
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("qemu-system-{}", args.common.arch.name())));
    if args.print_command {
        println!(
            "{}",
            serde_json::to_string(
                &std::iter::once(qemu.to_string_lossy().into_owned())
                    .chain(argv)
                    .collect::<Vec<_>>()
            )?
        );
        return Ok(());
    }
    let out = out_dir(root, args)?;
    if !args.common.dry_run {
        let image = boot_image(root, args)?;
        ensure!(
            image.is_file(),
            "boot image is missing; run cargo xtask build or cargo xtask run --build"
        );
        if args.common.arch == Arch::X86_64 {
            let vars = out.join("OVMF_VARS.xtask.fd");
            if !vars.exists() {
                util::write(
                    &vars,
                    &fs::read(root.join("spencer/a9nloader-rs/tools/OVMF_VARS.fd"))?,
                    false,
                )?;
            }
        } else {
            ensure!(
                out.join("u-boot/u-boot.bin").is_file()
                    && rootfs::path(root, &args.common).is_file(),
                "AArch64 U-Boot/rootfs artifacts are missing"
            );
        }
        if let Some(pcap) = &args.pcap {
            if let Some(parent) = pcap.parent().filter(|p| !p.as_os_str().is_empty()) {
                fs::create_dir_all(parent)?;
            }
        }
    }
    let mut cmd = if args.sudo {
        let mut c = Command::new("sudo");
        c.arg(&qemu);
        c
    } else {
        Command::new(&qemu)
    };
    if host() == Host::Mac && args.network() == Network::Bridged && !args.sudo {
        eprintln!("[xtask] macOS vmnet may require --sudo or an entitled QEMU binary");
    }
    cmd.args(argv);
    util::command(cmd, args.common.dry_run)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    fn parse(tail: &[&str]) -> Run {
        let cli = crate::cli::Cli::try_parse_from(
            ["xtask", "run"].into_iter().chain(tail.iter().copied()),
        )
        .unwrap();
        let crate::cli::Task::Run(args) = cli.task else {
            panic!()
        };
        args
    }
    #[test]
    fn xhci_is_created_before_usb_storage() {
        let args = arguments(Path::new("/a repo with spaces"), &parse(&[]), Host::Other).unwrap();
        let controller = args
            .iter()
            .position(|a| a.starts_with("qemu-xhci"))
            .unwrap();
        let disk = args
            .iter()
            .position(|a| a.starts_with("usb-storage"))
            .unwrap();
        assert!(controller < disk);
        assert!(args.iter().any(|a| a.contains("file=/a repo with spaces/")));
    }
    #[test]
    fn bridged_backends_match_the_host() {
        let input = parse(&["--bridge-if", "bridge0"]);
        for (h, expected) in [(Host::Mac, "vmnet-bridged"), (Host::Linux, "bridge,")] {
            assert!(arguments(Path::new("."), &input, h)
                .unwrap()
                .iter()
                .any(|s| s.starts_with(expected)));
        }
    }
    #[test]
    fn commas_in_image_paths_are_escaped_for_qemu() {
        assert_eq!(value(Path::new("/a,b/c.img")).unwrap(), "/a,,b/c.img");
    }
    #[test]
    fn validates_supported_accelerators() {
        for accelerator in ["auto", "none", "tcg", "kvm", "hvf"] {
            assert!(validate(&parse(&["--accel", accelerator])).is_ok());
        }
        assert!(validate(&parse(&["--accel", "whpx"])).is_err());
    }
    #[test]
    fn arm_uses_separate_root_disk_and_gic_v2() {
        let args = arguments(
            Path::new("."),
            &parse(&["--arch", "aarch64", "--net", "none"]),
            Host::Mac,
        )
        .unwrap();
        assert!(args.contains(&"virt,gic-version=2".into()));
        assert!(args.contains(&"virtio-blk-device,drive=blk0".into()));
        assert!(args.contains(&"tcg,thread=multi".into()));
    }
}
