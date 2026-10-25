use crate::cli::{Arch, Common};
use anyhow::{ensure, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct App {
    pub name: String,
    pub binary: String,
    pub manifest: PathBuf,
    pub core: bool,
    pub cpp: bool,
}

pub fn target_dir(root: &Path) -> PathBuf {
    root.join("nanami/servers/target")
}
pub fn artifact(root: &Path, args: &Common, app: &App) -> PathBuf {
    if app.cpp {
        app.manifest
            .parent()
            .unwrap()
            .join("build")
            .join(format!("{}.elf", app.binary))
    } else {
        target_dir(root)
            .join(format!("{}-unknown-a9n", args.arch.name()))
            .join(args.profile.name())
            .join(&app.binary)
    }
}

pub fn discover(root: &Path, args: &Common) -> Result<Vec<App>> {
    args.platform()?;
    let mut apps = Vec::new();
    for (directory, core) in [("core-services", true), ("apps", false)] {
        discover_at(
            &root.join("nanami/servers").join(directory),
            "",
            core,
            args,
            &mut apps,
        )?;
    }
    apps.sort_by(|a, b| a.name.cmp(&b.name));
    ensure!(!apps.is_empty(), "no native applications found");
    Ok(apps)
}

fn discover_at(
    dir: &Path,
    prefix: &str,
    core: bool,
    args: &Common,
    apps: &mut Vec<App>,
) -> Result<()> {
    let mut children = fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
    children.sort_by_key(|e| e.file_name());
    for entry in children {
        let kind = entry.file_type()?;
        if !kind.is_dir() {
            continue;
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("non-UTF8 application name"))?;
        if matches!(
            name.as_str(),
            "target" | "build" | ".git" | "src" | "assets" | ".cargo"
        ) {
            continue;
        }
        let installed = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}-{name}")
        };
        if args.arch == Arch::Aarch64
            && matches!(
                installed.as_str(),
                "acpi-server" | "ahci-server" | "hpet-server" | "timer-server" | "pci-server" | "intel-graphics-server" | "ps2-server" | "virtio-net"
            )
        {
            continue;
        }
        let path = entry.path();
        let manifest = path.join("Cargo.toml");
        if manifest.is_file() {
            let value: toml::Value = toml::from_str(&fs::read_to_string(&manifest)?)?;
            if let Some(binary) = value
                .get("package")
                .and_then(|p| p.get("name"))
                .and_then(toml::Value::as_str)
            {
                if path.join("src/main.rs").is_file() {
                    apps.push(App {
                        name: installed.clone(),
                        binary: binary.into(),
                        manifest,
                        core,
                        cpp: false,
                    });
                } else if let Some(bins) = value.get("bin").and_then(toml::Value::as_array) {
                    for bin in bins {
                        let binary = bin
                            .get("name")
                            .and_then(toml::Value::as_str)
                            .context("[[bin]] must have a name")?;
                        // Shared-source bins such as HPET install by component
                        // directory, not Cargo's underscore-separated bin name.
                        let name = if bins.len() == 1 {
                            installed.clone()
                        } else {
                            binary.replace('_', "-")
                        };
                        apps.push(App {
                            name,
                            binary: binary.into(),
                            manifest: manifest.clone(),
                            core,
                            cpp: false,
                        });
                    }
                }
            }
        }
        let cpp = path.join("nanami-app.toml");
        if args.arch == Arch::X86_64 && !args.skip_cpp && cpp.is_file() {
            let value: toml::Value = toml::from_str(&fs::read_to_string(&cpp)?)?;
            let binary = value
                .get("name")
                .and_then(toml::Value::as_str)
                .context("C++ app must have a name")?;
            ensure!(
                binary == installed,
                "C++ app name must match its directory path"
            );
            apps.push(App {
                name: installed.clone(),
                binary: binary.into(),
                manifest: cpp,
                core,
                cpp: true,
            });
        }
        discover_at(&path, &installed, core, args, apps)?;
    }
    Ok(())
}

pub fn manifest(root: &Path, name: &str, arch: Arch) -> Result<Vec<u8>> {
    let dir = root.join("nanami/servers");
    let specific = dir.join(format!("{name}.{}", arch.name()));
    fs::read(if specific.is_file() {
        specific
    } else {
        dir.join(name)
    })
    .with_context(|| format!("read {name} manifest"))
}

pub fn validate_elf(data: &[u8], arch: Arch, label: &Path) -> Result<()> {
    if !data.starts_with(b"\x7fELF") {
        return Ok(());
    }
    ensure!(
        data.len() >= 20 && &data[..6] == b"\x7fELF\x02\x01",
        "not a little-endian ELF64 file: {}",
        label.display()
    );
    let machine = u16::from_le_bytes([data[18], data[19]]);
    let expected = if arch == Arch::X86_64 { 0x3e } else { 0xb7 };
    ensure!(
        machine == expected,
        "wrong ELF machine {machine:#x} (expected {expected:#x}): {}",
        label.display()
    );
    Ok(())
}

/// Catch linker layout regressions before embedding an unbootable Alpha.
/// Unlike ordinary payload entries this input must be an executable ELF.
pub fn validate_alpha_elf(data: &[u8], arch: Arch, label: &Path) -> Result<()> {
    validate_elf(data, arch, label)?;
    ensure!(
        data.len() >= 64 && data.starts_with(b"\x7fELF"),
        "invalid Alpha ELF"
    );
    let word = |offset| u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
    let short = |offset| u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap());
    ensure!(short(16) == 2, "Alpha must be a fixed-address executable");
    let offset = usize::try_from(word(32))?;
    let stride = usize::from(short(54));
    let count = usize::from(short(56));
    let end = count
        .checked_mul(stride)
        .and_then(|bytes| offset.checked_add(bytes));
    ensure!(
        stride >= 56 && count > 0 && end.is_some_and(|end| end <= data.len()),
        "invalid Alpha program headers"
    );
    let mut executable_entry = false;
    for index in 0..count {
        let header = offset + index * stride;
        let kind = u32::from_le_bytes(data[header..header + 4].try_into().unwrap());
        if kind != 1 {
            continue;
        }
        let flags = u32::from_le_bytes(data[header + 4..header + 8].try_into().unwrap());
        let file = word(header + 8);
        let address = word(header + 16);
        let file_bytes = word(header + 32);
        let memory_bytes = word(header + 40);
        ensure!(
            file_bytes <= memory_bytes
                && file
                    .checked_add(file_bytes)
                    .is_some_and(|end| end <= data.len() as u64),
            "invalid Alpha segment contents"
        );
        if memory_bytes == 0 {
            continue;
        }
        ensure!(
            address >= 4096
                && address
                    .checked_add(memory_bytes)
                    .is_some_and(|end| end <= 64 * 1024 * 1024),
            "Alpha segment is outside the launcher image range"
        );
        if flags & 1 != 0 && word(24) >= address && word(24) < address + memory_bytes {
            executable_entry = true;
        }
    }
    ensure!(
        executable_entry,
        "Alpha entry is not in an executable segment"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn alpha_layout_rejects_zero_header_mapping_on_both_architectures() {
        for arch in [Arch::X86_64, Arch::Aarch64] {
            let mut elf = vec![0u8; 4096];
            elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
            elf[16..18].copy_from_slice(&2u16.to_le_bytes());
            let machine = if arch == Arch::X86_64 {
                0x3eu16
            } else {
                0xb7u16
            };
            elf[18..20].copy_from_slice(&machine.to_le_bytes());
            elf[24..32].copy_from_slice(&4096u64.to_le_bytes());
            elf[32..40].copy_from_slice(&64u64.to_le_bytes());
            elf[54..56].copy_from_slice(&56u16.to_le_bytes());
            elf[56..58].copy_from_slice(&1u16.to_le_bytes());
            elf[64..68].copy_from_slice(&1u32.to_le_bytes());
            elf[68..72].copy_from_slice(&5u32.to_le_bytes());
            elf[80..88].copy_from_slice(&4096u64.to_le_bytes());
            elf[96..104].copy_from_slice(&128u64.to_le_bytes());
            elf[104..112].copy_from_slice(&4096u64.to_le_bytes());
            let label = Path::new("alpha");
            validate_alpha_elf(&elf, arch, label).unwrap();
            elf[80..88].copy_from_slice(&0u64.to_le_bytes());
            assert!(validate_alpha_elf(&elf, arch, label).is_err());
            elf[80..88].copy_from_slice(&4096u64.to_le_bytes());
            elf[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
            assert!(validate_alpha_elf(&elf, arch, label).is_err());
            assert!(validate_alpha_elf(b"not an executable", arch, label).is_err());
        }
    }
    #[test]
    fn shared_source_binary_keeps_its_installed_component_name() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("nanami/servers/core-services/hpet-server")).unwrap();
        fs::create_dir_all(root.path().join("nanami/servers/apps/alter/linux/src")).unwrap();
        fs::write(root.path().join("nanami/servers/core-services/hpet-server/Cargo.toml"),
            "[package]\nname='hpet_server'\n[[bin]]\nname='hpet_server'\npath='../timer-server/src/main.rs'\n").unwrap();
        fs::write(
            root.path()
                .join("nanami/servers/apps/alter/linux/Cargo.toml"),
            "[package]\nname='alter-linux'\n",
        )
        .unwrap();
        fs::write(
            root.path()
                .join("nanami/servers/apps/alter/linux/src/main.rs"),
            "",
        )
        .unwrap();
        let crate::cli::Task::Servers(args) = crate::cli::Cli::try_parse_from(["xtask", "servers"])
            .unwrap()
            .task
        else {
            panic!()
        };
        let apps = discover(root.path(), &args).unwrap();
        assert_eq!(
            apps.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["alter-linux", "hpet-server"]
        );
        assert_eq!(apps[1].binary, "hpet_server");
        assert!(apps[1].core);
        let arm = Common {
            arch: Arch::Aarch64,
            ..args
        };
        assert_eq!(discover(root.path(), &arm).unwrap().len(), 1);
    }
    #[test]
    fn validates_architecture_but_keeps_non_elf_payloads() {
        let mut elf = vec![0; 20];
        elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
        elf[18] = 0x3e;
        assert!(validate_elf(&elf, Arch::X86_64, Path::new("doom")).is_ok());
        assert!(validate_elf(&elf, Arch::Aarch64, Path::new("doom")).is_err());
        assert!(validate_elf(b"\x7fELF", Arch::X86_64, Path::new("truncated")).is_err());
        assert!(validate_elf(b"IWAD data", Arch::X86_64, Path::new("doom.wad")).is_ok());
    }
}
