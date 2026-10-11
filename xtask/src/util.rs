use anyhow::{ensure, Context, Result};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn absolute(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.into()
    } else {
        root.join(path)
    }
}
pub fn path_text(path: &Path) -> Result<String> {
    Ok(path.to_str().context("non-UTF8 build path")?.to_owned())
}
pub fn command(mut cmd: Command, dry: bool) -> Result<()> {
    eprintln!("[xtask] {:?}", cmd);
    if !dry {
        ensure!(
            cmd.status()
                .with_context(|| format!("start {:?}", cmd.get_program()))?
                .success(),
            "command failed: {:?}",
            cmd
        );
    }
    Ok(())
}
pub fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

/// Same-directory temporary output: a failed build leaves the old image intact.
/// persist_noclobber protects an existing image unless overwrite was requested.
pub fn write(path: &Path, bytes: &[u8], overwrite: bool) -> Result<()> {
    use std::io::Write;
    let parent = path.parent().context("output has no parent")?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    if overwrite {
        temporary.persist(path).map_err(|e| e.error)?;
    } else {
        temporary.persist_noclobber(path).map_err(|e| e.error)?;
    }
    Ok(())
}

/// Return sorted regular files, including hidden files. Reject links and special
/// files instead of following a host path outside the supplied tree.
pub fn regular_files(root: &Path) -> Result<Vec<PathBuf>> {
    ensure!(
        fs::symlink_metadata(root)?.file_type().is_dir(),
        "not a regular directory: {}",
        root.display()
    );
    let mut result = Vec::new();
    fn walk(dir: &Path, result: &mut Vec<PathBuf>) -> Result<()> {
        let mut entries = fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let kind = entry.file_type()?;
            if kind.is_dir() {
                walk(&entry.path(), result)?;
            } else if kind.is_file() {
                result.push(entry.path());
            } else {
                anyhow::bail!(
                    "symlink or special file in payload: {}",
                    entry.path().display()
                );
            }
        }
        Ok(())
    }
    walk(root, &mut result)?;
    Ok(result)
}
pub fn name(path: &Path) -> Result<&str> {
    path.file_name()
        .and_then(OsStr::to_str)
        .context("payload name must be valid UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_literal_backslashes_in_unix_paths() {
        assert_eq!(
            path_text(Path::new("/a\\b/file.img")).unwrap(),
            "/a\\b/file.img"
        );
    }
    #[test]
    fn failed_noclobber_write_preserves_existing_data() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("saved.img");
        write(&p, b"saved guest data", false).unwrap();
        assert!(write(&p, b"replacement", false).is_err());
        assert_eq!(fs::read(&p).unwrap(), b"saved guest data");
        write(&p, b"explicit replacement", true).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"explicit replacement");
    }
    #[test]
    fn collects_nested_and_hidden_files() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join("nested")).unwrap();
        fs::write(d.path().join(".hidden"), b"").unwrap();
        fs::write(d.path().join("nested/data.wad"), b"").unwrap();
        assert_eq!(regular_files(d.path()).unwrap().len(), 2);
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlinks() {
        let d = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/", d.path().join("escape")).unwrap();
        assert!(regular_files(d.path()).is_err());
    }
}
