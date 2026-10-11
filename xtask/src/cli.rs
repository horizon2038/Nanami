use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Build and boot Nanami without host shell scripts")]
pub struct Cli {
    #[command(subcommand)]
    pub task: Task,
}

#[derive(Subcommand)]
pub enum Task {
    /// Build user space, initramfs, rootfs, A9N and the boot disk.
    #[command(alias = "image")]
    Build(Common),
    /// Build all native services/apps and the boot initramfs.
    Servers(Common),
    /// Package previously built applications into a Rust-generated ext2 image.
    Rootfs(Common),
    /// Boot the existing image (add --build to build first).
    Run(Run),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Arch {
    #[value(alias = "x86_64")]
    X86_64,
    Aarch64,
}
impl Arch {
    pub fn name(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Platform {
    Pc99,
    Qemu,
    Rpi4b,
}
impl Platform {
    pub fn name(self) -> &'static str {
        match self {
            Self::Pc99 => "pc99",
            Self::Qemu => "qemu",
            Self::Rpi4b => "rpi4b",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Profile {
    Release,
    Debug,
}
impl Profile {
    pub fn name(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Debug => "debug",
        }
    }
}

#[derive(Clone, Debug, Args)]
pub struct Common {
    #[arg(long, value_enum, default_value = "x86-64")]
    pub arch: Arch,
    #[arg(long, value_enum)]
    pub platform: Option<Platform>,
    #[arg(long, value_enum, default_value = "release")]
    pub profile: Profile,
    /// Rootfs staging image; defaults to out/ext2[-aarch64].img.
    #[arg(long)]
    pub rootfs: Option<PathBuf>,
    /// Boot disk output (build/run); defaults to spencer/out/<target>/spencer.img.
    #[arg(long)]
    pub output_image: Option<PathBuf>,
    #[arg(long, default_value = "64", value_parser = clap::value_parser!(u32).range(1..))]
    pub size_mib: u32,
    /// Replace an existing rootfs explicitly. Guest data will be lost.
    #[arg(long)]
    pub rebuild_rootfs: bool,
    /// Recursively collect ALL regular files, including hidden files, by basename.
    #[arg(long, env = "LINUX_BIN_DIR")]
    pub linux_bin_dir: Option<PathBuf>,
    /// Do not collect the default bin/ directory.
    #[arg(long, conflicts_with = "linux_bin_dir")]
    pub no_linux_bins: bool,
    /// Additional payload file, repeatable; accepts paths containing spaces.
    #[arg(long)]
    pub linux_bin: Vec<PathBuf>,
    #[arg(long)]
    pub freebsd_bin: Vec<PathBuf>,
    /// Additional regular-file directory tree for /alter/linux; links are rejected.
    #[arg(long, env = "LINUX_ROOTFS_DIR")]
    pub linux_rootfs_dir: Option<PathBuf>,
    /// Select rootfs apps by their installed name; repeatable.
    #[arg(long)]
    pub rootfs_app: Vec<String>,
    #[arg(long)]
    pub skip_cpp: bool,
    #[arg(long)]
    pub verbose: bool,
    /// Print actions without building, writing files, or starting QEMU.
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub offline: bool,
}
impl Common {
    pub fn platform(&self) -> anyhow::Result<Platform> {
        let p = self.platform.unwrap_or(match self.arch {
            Arch::X86_64 => Platform::Pc99,
            Arch::Aarch64 => Platform::Qemu,
        });
        anyhow::ensure!(
            matches!(
                (self.arch, p),
                (Arch::X86_64, Platform::Pc99) | (Arch::Aarch64, Platform::Qemu | Platform::Rpi4b)
            ),
            "unsupported architecture/platform pair"
        );
        Ok(p)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Network {
    User,
    Bridged,
    None,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Storage {
    Usb,
    Ahci,
    Virtio,
}
#[derive(Clone, Debug, Args)]
pub struct Run {
    #[command(flatten)]
    pub common: Common,
    #[arg(long, conflicts_with = "print_command")]
    pub build: bool,
    #[arg(long)]
    pub qemu: Option<PathBuf>,
    /// Boot another existing disk image without replacing the build artifact.
    #[arg(long, conflicts_with_all = ["build", "output_image"])]
    pub image: Option<PathBuf>,
    /// Print the QEMU argument vector as JSON and do not launch/write firmware.
    #[arg(long)]
    pub print_command: bool,
    #[arg(long, default_value = "4G")]
    pub memory: String,
    #[arg(long, default_value = "4", value_parser = clap::value_parser!(u16).range(1..=64))]
    pub smp: u16,
    #[arg(long, default_value = "auto")]
    pub accel: String,
    #[arg(long)]
    pub cpu: Option<String>,
    #[arg(long)]
    pub no_hpet: bool,
    #[arg(long)]
    pub no_usb_input: bool,
    #[arg(long, value_enum, default_value = "usb")]
    pub storage: Storage,
    /// Defaults to user; BRIDGE_IF/--bridge-if selects bridged unless overridden.
    #[arg(long, value_enum, env = "NET_MODE")]
    pub net: Option<Network>,
    /// macOS vmnet interface or Linux bridge.
    #[arg(long, env = "BRIDGE_IF")]
    pub bridge_if: Option<String>,
    #[arg(long, default_value = "tcp:127.0.0.1:1234-:80")]
    pub hostfwd: String,
    #[arg(long)]
    pub pcap: Option<PathBuf>,
    /// Launch QEMU with sudo (never runs build steps as root).
    #[arg(long)]
    pub sudo: bool,
    /// Use a snapshot so guest writes do not alter the boot disk.
    #[arg(long)]
    pub snapshot: bool,
    #[arg(long)]
    pub headless: bool,
    #[arg(long)]
    pub gdb: bool,
    /// Extra QEMU arguments after --, passed as separate arguments without a shell.
    #[arg(last = true)]
    pub extra: Vec<String>,
}
impl Run {
    pub fn network(&self) -> Network {
        self.net.unwrap_or(if self.bridge_if.is_some() {
            Network::Bridged
        } else if self.common.arch == Arch::Aarch64 {
            Network::None
        } else {
            Network::User
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_with_spaces_and_repeatable_payloads_are_not_shell_parsed() {
        let cli = Cli::try_parse_from([
            "xtask",
            "build",
            "--linux-bin",
            "C:\\my files\\doom.elf",
            "--linux-bin",
            "lua",
        ])
        .unwrap();
        let Task::Build(args) = cli.task else {
            panic!()
        };
        assert_eq!(args.linux_bin.len(), 2);
        assert_eq!(args.linux_bin[0], PathBuf::from("C:\\my files\\doom.elf"));
    }
    #[test]
    fn run_does_not_build_unless_requested() {
        let cli = Cli::try_parse_from(["xtask", "run"]).unwrap();
        let Task::Run(args) = cli.task else { panic!() };
        assert!(!args.build);
        assert!(!args.common.rebuild_rootfs);
    }
    #[test]
    fn bridge_if_selects_bridged_but_explicit_mode_wins() {
        for (tail, expected) in [
            (vec!["--bridge-if", "en0"], Network::Bridged),
            (vec!["--bridge-if", "en0", "--net", "none"], Network::None),
        ] {
            let cli = Cli::try_parse_from(["xtask", "run"].into_iter().chain(tail)).unwrap();
            let Task::Run(args) = cli.task else { panic!() };
            assert_eq!(args.network(), expected);
        }
    }
    #[test]
    fn rejects_invalid_cpu_counts_and_platforms() {
        assert!(Cli::try_parse_from(["xtask", "run", "--smp", "0"]).is_err());
        let cli =
            Cli::try_parse_from(["xtask", "build", "--arch", "aarch64", "--platform", "pc99"])
                .unwrap();
        let Task::Build(args) = cli.task else {
            panic!()
        };
        assert!(args.platform().is_err());
    }
}
