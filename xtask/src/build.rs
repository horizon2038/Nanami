use crate::{
    cli::{Arch, Common, Profile},
    cpio, payload, rootfs, util,
};
use anyhow::{ensure, Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub fn servers(root: &Path, args: &Common) -> Result<()> {
    let apps = payload::discover(root, args)?;
    let arch = args.arch.name();
    let target = root.join(format!(
        "nanami/servers/targets/{arch}/{arch}-unknown-a9n.json"
    ));
    let linker = root.join(format!(
        "nanami/servers/targets/{arch}/linker/nanami_user.ld"
    ));
    let target_dir = payload::target_dir(root);
    for app in &apps {
        if app.cpp {
            build_cpp(root, app, args)?;
            continue;
        }
        let mut cmd = util::cargo();
        cmd.current_dir(root.join("nanami/servers"))
            .args(["build", "--manifest-path"])
            .arg(&app.manifest)
            .arg("--target")
            .arg(&target)
            .args([
                "-Z",
                "json-target-spec",
                "-Z",
                "build-std=core,alloc,compiler_builtins",
                "-Z",
                "build-std-features=compiler-builtins-mem",
            ])
            .env("CARGO_TARGET_DIR", &target_dir)
            // Encoded flags preserve linker-script paths containing spaces.
            .env_remove("RUSTFLAGS")
            .env(
                "CARGO_ENCODED_RUSTFLAGS",
                format!("-C\x1flink-arg=-T{}", util::path_text(&linker)?),
            );
        if args.profile == Profile::Release {
            cmd.arg("--release");
        }
        if args.verbose {
            cmd.arg("--verbose");
        }
        util::command(cmd, args.dry_run)?;
    }
    package_boot_archive(root, args, &apps, false)?;
    Ok(())
}

fn package_boot_archive(
    root: &Path,
    args: &Common,
    apps: &[payload::App],
    include_alpha: bool,
) -> Result<()> {
    if !args.dry_run {
        let configuration = if let Some(path) = &args.boot_list {
            let path = root.join(path);
            ensure!(
                fs::symlink_metadata(&path)?.is_file(),
                "boot list must be a regular file"
            );
            fs::read(path)?
        } else {
            payload::manifest(root, "boot-list", args.arch)?
        };
        let mut records = vec![
            (".".into(), 0o40755, vec![]),
            ("./bin".into(), 0o40755, vec![]),
            ("./nanami".into(), 0o40755, vec![]),
            ("./nanami/boot-list".into(), 0o100644, configuration),
        ];
        for app in apps.iter().filter(|a| a.core) {
            let source = payload::artifact(root, args, app);
            let data = fs::read(&source).with_context(|| format!("read {}", source.display()))?;
            payload::validate_elf(&data, args.arch, &source)?;
            records.push((format!("./bin/{}", app.name), 0o100755, data));
        }
        if include_alpha {
            let path = root.join("nanami/alpha.elf");
            let data = fs::read(&path)?;
            payload::validate_alpha_elf(&data, args.arch, &path)?;
            records.push(("./nanami/alpha".into(), 0o100755, data));
        }
        if !args.boot_module.is_empty() {
            records.push(("./modules".into(), 0o40755, vec![]));
        }
        for path in &args.boot_module {
            let path = root.join(path);
            ensure!(
                fs::symlink_metadata(&path)?.is_file(),
                "boot module must be a regular file"
            );
            let basename = path
                .file_name()
                .and_then(|name| name.to_str())
                .context("invalid boot module name")?;
            let name = format!("./modules/{basename}");
            ensure!(
                !records.iter().any(|(existing, _, _)| *existing == name),
                "duplicate boot module: {basename}"
            );
            let data = fs::read(&path)?;
            payload::validate_elf(&data, args.arch, &path)?;
            records.push((name, 0o100755, data));
        }
        records.sort_by(|a, b| a.0.cmp(&b.0));
        util::write(
            &root.join("nanami/servers/initramfs.cpio"),
            &cpio::archive(&records)?,
            true,
        )?;
    } else {
        eprintln!("[dry-run] generate initramfs.cpio from boot-list and core services");
    }
    Ok(())
}

pub fn image(root: &Path, args: &Common) -> Result<PathBuf> {
    let platform = args.platform()?;
    servers(root, args)?;
    let filesystem = rootfs::path(root, args);
    if args.rebuild_rootfs || !filesystem.exists() {
        if args.dry_run {
            eprintln!("[dry-run] generate rootfs {}", filesystem.display());
        } else {
            rootfs::create(root, args)?;
        }
    } else {
        eprintln!(
            "[xtask] reusing existing rootfs {}; pass --rebuild-rootfs to regenerate",
            filesystem.display()
        );
    }
    let target = root.join(format!("out/targets/{}-unknown-a9n.json", args.arch.name()));
    if !args.dry_run {
        let mut value: serde_json::Value = serde_json::from_slice(&fs::read(
            root.join(format!("targets/{}-unknown-a9n.json", args.arch.name())),
        )?)?;
        value["pre-link-args"]["ld.lld"][1] =
            util::path_text(&root.join(format!("spencer/Nun/arch/{}/user.ld", args.arch.name())))?
                .into();
        util::write(
            &target,
            serde_json::to_string_pretty(&value)?.as_bytes(),
            true,
        )?;
    }
    alpha(root, args)?;
    package_boot_archive(root, args, &payload::discover(root, args)?, true)?;
    let common = spencer_build::cli::CommonArgs {
        arch: match args.arch {
            Arch::X86_64 => spencer_build::cli::Arch::X86_64,
            Arch::Aarch64 => spencer_build::cli::Arch::Aarch64,
        },
        platform: match platform {
            crate::cli::Platform::Pc99 => spencer_build::cli::Platform::Pc99,
            crate::cli::Platform::Qemu => spencer_build::cli::Platform::Qemu,
            crate::cli::Platform::Rpi4b => spencer_build::cli::Platform::Rpi4b,
        },
        release: args.profile == Profile::Release,
        verbose: args.verbose,
        dry_run: args.dry_run,
        os_manifest: Some(
            camino::Utf8PathBuf::from_path_buf(root.join("nanami/Cargo.toml"))
                .map_err(|_| anyhow::anyhow!("non-UTF8 manifest"))?,
        ),
        os_target_json: Some(
            camino::Utf8PathBuf::from_path_buf(target)
                .map_err(|_| anyhow::anyhow!("non-UTF8 target path"))?,
        ),
        os_binary: "nanami".into(),
        rootfs_image: if args.arch == Arch::X86_64 {
            Some(
                camino::Utf8PathBuf::from_path_buf(filesystem)
                    .map_err(|_| anyhow::anyhow!("non-UTF8 rootfs path"))?,
            )
        } else {
            None
        },
    };
    let spencer = root.join("spencer");
    let spencer = camino::Utf8Path::from_path(&spencer).context("non-UTF8 Spencer path")?;
    let out = args
        .output_image
        .as_ref()
        .map(|p| util::absolute(root, p))
        .unwrap_or_else(|| {
            root.join(format!(
                "spencer/out/{}-{}-{}/spencer.img",
                args.arch.name(),
                platform.name(),
                args.profile.name()
            ))
        });
    let out_utf8 = camino::Utf8Path::from_path(&out).context("non-UTF8 image path")?;
    spencer_build::run_build_pipeline_to_image(spencer, &common, true, Some(out_utf8))?;
    eprintln!("[xtask] image ready: {}", out.display());
    Ok(out)
}

/// Build the common Alpha executable before compiling the init Booter that
/// embeds it. Neither the kernel nor Spencer's dependency revision is changed.
fn alpha(root: &Path, args: &Common) -> Result<()> {
    let name = format!("alpha-{}-unknown-a9n", args.arch.name());
    let target = root.join(format!("out/targets/{name}.json"));
    let directory = root.join("out/alpha-target");
    if !args.dry_run {
        let mut value: serde_json::Value = serde_json::from_slice(&fs::read(
            root.join(format!("targets/{}-unknown-a9n.json", args.arch.name())),
        )?)?;
        value["pre-link-args"]["ld.lld"][1] =
            util::path_text(&root.join(format!("targets/{}/alpha.ld", args.arch.name())))?.into();
        util::write(
            &target,
            serde_json::to_string_pretty(&value)?.as_bytes(),
            true,
        )?;
    }
    let mut command = util::cargo();
    command
        .current_dir(root)
        .args(["build", "--locked", "--manifest-path"])
        .arg(root.join("nanami/Cargo.toml"))
        .args(["--bin", "alpha", "--features", "alpha-entry", "--target"])
        .arg(&target)
        .args([
            "-Z",
            "json-target-spec",
            "-Z",
            "build-std=core,alloc,compiler_builtins",
            "-Z",
            "build-std-features=compiler-builtins-mem",
        ])
        .env("CARGO_TARGET_DIR", &directory);
    if args.profile == Profile::Release {
        command.arg("--release");
    }
    util::command(command, args.dry_run)?;
    if !args.dry_run {
        let executable = directory.join(name).join(args.profile.name()).join("alpha");
        let bytes =
            fs::read(&executable).with_context(|| format!("read {}", executable.display()))?;
        payload::validate_alpha_elf(&bytes, args.arch, &executable)?;
        util::write(&root.join("nanami/alpha.elf"), &bytes, true)?;
    }
    Ok(())
}

fn build_cpp(root: &Path, app: &payload::App, args: &Common) -> Result<()> {
    let directory = app.manifest.parent().unwrap();
    let output = directory.join("build");
    if !args.dry_run {
        fs::create_dir_all(&output)?;
    }
    let value: toml::Value = toml::from_str(&fs::read_to_string(&app.manifest)?)?;
    let sources = value
        .get("sources")
        .and_then(toml::Value::as_array)
        .context("C++ app sources must be an array")?;
    let runtime = root.join("nanami/servers/libs/nanami-runtime");
    let mut inputs = vec![
        runtime.join("arch/x86_64/runtime/start.S"),
        runtime.join("cpp/src/nanami/nanami.cpp"),
    ];
    for file in ["debug_call.cpp", "ipc_port.cpp", "processor.cpp"] {
        inputs.push(runtime.join("arch/x86_64/a9n_abi").join(file));
    }
    for source in sources {
        inputs.push(
            directory.join(
                source
                    .as_str()
                    .context("C++ source must be a path string")?,
            ),
        );
    }
    let mut objects = Vec::new();
    for (i, source) in inputs.iter().enumerate() {
        let object = output.join(format!("{i}.o"));
        let cpp = source.extension().is_some_and(|e| e == "cpp");
        let mut cmd = Command::new(if cpp { "clang++" } else { "clang" });
        cmd.args([
            "--target=x86_64-unknown-elf",
            "-ffreestanding",
            "-fno-stack-protector",
            "-mno-red-zone",
            "-nostdlib",
        ])
        .arg(if args.profile == Profile::Release {
            "-O2"
        } else {
            "-O0"
        })
        .arg("-I")
        .arg(root.join("nanami/servers/uapi/include"));
        if cpp {
            cmd.args(["-std=c++20", "-fno-exceptions", "-fno-rtti"]);
        }
        cmd.arg("-c").arg(source).arg("-o").arg(&object);
        util::command(cmd, args.dry_run)?;
        objects.push(object);
    }
    let mut cmd = Command::new("ld.lld");
    cmd.args(["-static", "-no-pie", "-T"])
        .arg(root.join("nanami/servers/targets/x86_64/linker/nanami_user.ld"))
        .arg("-o")
        .arg(output.join(format!("{}.elf", app.binary)))
        .args(objects);
    util::command(cmd, args.dry_run)
}
