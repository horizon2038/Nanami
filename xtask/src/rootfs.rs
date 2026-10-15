use crate::{cli::Common, payload, util};
use anyhow::{ensure, Context, Result};
use ext2_image::ImageBuilder;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn path(root: &Path, args: &Common) -> PathBuf {
    args.rootfs
        .as_ref()
        .map(|p| util::absolute(root, p))
        .unwrap_or_else(|| {
            root.join("out")
                .join(if args.arch == crate::cli::Arch::X86_64 {
                    "ext2.img"
                } else {
                    "ext2-aarch64.img"
                })
        })
}

pub fn create(root: &Path, args: &Common) -> Result<PathBuf> {
    args.platform()?;
    let out = path(root, args);
    ensure!(
        !out.exists() || args.rebuild_rootfs,
        "rootfs exists: {}. Reuse it, or pass --rebuild-rootfs to discard its guest data",
        out.display()
    );
    let mut builder = ImageBuilder::new(args.size_mib as u64 * 1024 * 1024)?;
    builder.set_label("Nanami ext2")?;
    for dir in [
        "/nanami",
        "/bin",
        "/.honoka",
        "/alter",
        "/alter/linux",
        "/alter/freebsd",
    ] {
        builder.add_directory(dir, 0o755)?;
    }
    for guest in ["linux", "freebsd"] {
        for dir in [
            "bin", "etc", "dev", "tmp", "usr", "usr/bin", "sbin", "usr/sbin",
        ] {
            builder.add_directory(
                &format!("/alter/{guest}/{dir}"),
                if dir == "tmp" { 0o777 } else { 0o755 },
            )?;
        }
        crate::terminfo::install(&mut builder, guest)?;
    }
    let system = payload::manifest(root, "system-list", args.arch)?;
    let session = payload::manifest(root, "session-list", args.arch)?;
    builder.add_file("/nanami/system-list", 0o644, system.clone())?;
    builder.add_file("/nanami/session-list", 0o644, session.clone())?;
    let theme = root.join("nanami/servers/apps/honoka/assets/themes");
    let config = fs::read(theme.join("config")).context("Honoka theme config")?;
    ensure!(!config.is_empty(), "Honoka theme config is empty");
    builder.add_file("/.honoka/config", 0o644, config)?;
    let mut themes = util::regular_files(&theme)?
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "theme"))
        .collect::<Vec<_>>();
    themes.sort();
    ensure!(!themes.is_empty(), "no Honoka themes");
    for file in themes {
        builder.add_file(
            &format!("/.honoka/{}", util::name(&file)?),
            0o644,
            fs::read(&file)?,
        )?;
    }

    // Put startup applications first to preserve efficient sequential reading.
    let order = [system, session]
        .iter()
        .flat_map(|b| {
            String::from_utf8_lossy(b)
                .lines()
                .filter_map(|line| {
                    if line.trim_start().starts_with('#') {
                        return None;
                    }
                    line.split_whitespace()
                        .nth(2)
                        .and_then(|p| p.strip_prefix("/bin/"))
                        .map(str::to_owned)
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut apps = payload::discover(root, args)?
        .into_iter()
        .filter(|a| !a.core && (args.rootfs_app.is_empty() || args.rootfs_app.contains(&a.name)))
        .collect::<Vec<_>>();
    for wanted in &args.rootfs_app {
        ensure!(
            apps.iter().any(|a| &a.name == wanted),
            "unknown rootfs app: {wanted}"
        );
    }
    apps.sort_by_key(|a| {
        (
            order
                .iter()
                .position(|n| n == &a.name)
                .unwrap_or(usize::MAX),
            a.name.clone(),
        )
    });
    for app in apps {
        let source = payload::artifact(root, args, &app);
        let data = fs::read(&source).with_context(|| {
            format!(
                "missing app {}; run cargo xtask servers first",
                source.display()
            )
        })?;
        payload::validate_elf(&data, args.arch, &source)?;
        if app.name.starts_with("_linux-") {
            add_guest(&mut builder, "linux", &app.name[1..], data)?;
        } else {
            builder.add_file(&format!("/bin/{}", app.name), 0o755, data)?;
        }
    }
    let mut linux = args
        .linux_bin
        .iter()
        .map(|p| util::absolute(root, p))
        .collect::<Vec<_>>();
    if !args.no_linux_bins {
        let directory = args
            .linux_bin_dir
            .as_ref()
            .map(|p| util::absolute(root, p))
            .unwrap_or_else(|| root.join("bin"));
        linux.extend(util::regular_files(&directory).with_context(|| {
            format!(
                "Linux payload directory {}; use --no-linux-bins to omit",
                directory.display()
            )
        })?);
    }
    for (guest, files) in [
        ("linux", linux),
        (
            "freebsd",
            args.freebsd_bin
                .iter()
                .map(|p| util::absolute(root, p))
                .collect(),
        ),
    ] {
        for file in files {
            ensure!(
                fs::symlink_metadata(&file)?.file_type().is_file(),
                "not a regular payload file: {}",
                file.display()
            );
            let data = fs::read(&file)?;
            payload::validate_elf(&data, args.arch, &file)?;
            let name = util::name(&file)?;
            add_guest(
                &mut builder,
                guest,
                name.strip_suffix(".elf").unwrap_or(name),
                data,
            )?;
        }
    }
    if let Some(overlay) = &args.linux_rootfs_dir {
        overlay_tree(
            &mut builder,
            &util::absolute(root, overlay),
            "/alter/linux",
            args,
        )?;
    }
    if args.dry_run {
        eprintln!(
            "[dry-run] generate ext2 {} ({} MiB); no output written",
            out.display(),
            args.size_mib
        );
    } else {
        util::write(&out, &builder.build()?, args.rebuild_rootfs)?;
        eprintln!("[xtask] rootfs ready: {}", out.display());
    }
    Ok(out)
}

fn add_guest(builder: &mut ImageBuilder, guest: &str, name: &str, data: Vec<u8>) -> Result<()> {
    let path = format!("/alter/{guest}/bin/{name}");
    builder
        .add_file(&path, 0o755, data)
        .with_context(|| format!("guest payload {path}"))?;
    builder.add_hard_link(&format!("/alter/{guest}/usr/bin/{name}"), &path)?;
    Ok(())
}
fn overlay_tree(builder: &mut ImageBuilder, host: &Path, guest: &str, args: &Common) -> Result<()> {
    ensure!(
        fs::symlink_metadata(host)?.file_type().is_dir(),
        "overlay must be a regular directory"
    );
    let mut entries = fs::read_dir(host)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let source = entry.path();
        let destination = format!("{guest}/{}", util::name(&source)?);
        let kind = entry.file_type()?;
        if kind.is_dir() {
            if !builder.contains(&destination) {
                builder.add_directory(&destination, permissions(&source, 0o755)?)?;
            }
            ensure!(
                builder.is_directory(&destination),
                "overlay directory collides with file: {destination}"
            );
            overlay_tree(builder, &source, &destination, args)?;
        } else if kind.is_file() {
            let data = fs::read(&source)?;
            payload::validate_elf(&data, args.arch, &source)?;
            builder.add_file(&destination, permissions(&source, 0o644)?, data)?;
        } else {
            anyhow::bail!(
                "overlay links/special files are not supported: {}",
                source.display()
            );
        }
    }
    Ok(())
}
fn permissions(path: &Path, fallback: u16) -> Result<u16> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fallback;
        Ok((fs::metadata(path)?.permissions().mode() & 0o7777) as u16)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(fallback)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn fixture() -> (tempfile::TempDir, Common) {
        let root = tempfile::tempdir().unwrap();
        for directory in [
            "nanami/servers/core-services",
            "nanami/servers/apps/sample/src",
            "nanami/servers/apps/honoka/assets/themes",
            "bin/nested",
        ] {
            fs::create_dir_all(root.path().join(directory)).unwrap();
        }
        for (file, contents) in [
            (
                "nanami/servers/apps/sample/Cargo.toml",
                "[package]\nname='sample_binary'\n",
            ),
            ("nanami/servers/apps/sample/src/main.rs", ""),
            ("nanami/servers/system-list", "sample 20 /bin/sample\n"),
            ("nanami/servers/session-list", ""),
            ("nanami/servers/apps/honoka/assets/themes/config", "dark"),
            (
                "nanami/servers/apps/honoka/assets/themes/dark.theme",
                "theme-payload",
            ),
            ("bin/nested/game.elf", "guest-executable"),
            ("bin/.hidden", "hidden-payload"),
            ("bin/data.wad", "game-data-payload"),
        ] {
            fs::write(root.path().join(file), contents).unwrap();
        }
        let crate::cli::Task::Rootfs(mut args) =
            crate::cli::Cli::try_parse_from(["xtask", "rootfs"])
                .unwrap()
                .task
        else {
            panic!()
        };
        args.rootfs = Some("fixture.img".into());
        args.linux_bin_dir = Some(root.path().join("bin"));
        args.linux_rootfs_dir = None;
        args.size_mib = 2;
        let artifact =
            payload::target_dir(root.path()).join("x86_64-unknown-a9n/release/sample_binary");
        fs::create_dir_all(artifact.parent().unwrap()).unwrap();
        fs::write(artifact, "native-payload").unwrap();
        (root, args)
    }

    #[test]
    fn packages_all_nested_hidden_and_data_payloads_deterministically() {
        let (root, mut args) = fixture();
        let image = fs::read(create(root.path(), &args).unwrap()).unwrap();
        for contents in [
            b"guest-executable".as_slice(),
            b"hidden-payload",
            b"game-data-payload",
            b"native-payload",
            b"theme-payload",
            b"vt100|Nanami VT100-compatible terminal\0",
        ] {
            assert!(image.windows(contents.len()).any(|w| w == contents));
        }
        assert_eq!(image.len(), 2 * 1024 * 1024);
        args.rootfs = Some("second.img".into());
        assert_eq!(
            image,
            fs::read(create(root.path(), &args).unwrap()).unwrap()
        );
    }

    #[test]
    fn existing_images_require_explicit_replacement_and_errors_preserve_them() {
        let (root, mut args) = fixture();
        let out = root.path().join("fixture.img");
        fs::write(&out, b"saved guest data").unwrap();
        assert!(create(root.path(), &args).is_err());
        assert_eq!(fs::read(&out).unwrap(), b"saved guest data");
        args.rebuild_rootfs = true;
        // Flattened basenames cannot silently replace a different executable.
        fs::write(root.path().join("bin/game"), b"duplicate").unwrap();
        assert!(create(root.path(), &args).is_err());
        assert_eq!(fs::read(&out).unwrap(), b"saved guest data");
    }

    #[test]
    fn overlay_conflicts_fail_without_creating_an_image() {
        let (root, mut args) = fixture();
        fs::create_dir_all(root.path().join("overlay/bin")).unwrap();
        fs::write(root.path().join("overlay/bin/game"), b"collision").unwrap();
        args.linux_rootfs_dir = Some("overlay".into());
        assert!(create(root.path(), &args).is_err());
        assert!(!root.path().join("fixture.img").exists());
    }
}
