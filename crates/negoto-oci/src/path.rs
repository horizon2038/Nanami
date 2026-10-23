use crate::{Error, Result};
use alloc::{format, string::String, vec::Vec};

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
        && id != "."
        && id != ".."
}

/// Normalize in the guest namespace before adding a host prefix. Guest `..`
/// cannot remove a component of the backing root.
pub fn guest_path(cwd: &str, path: &str) -> Result<String> {
    if !cwd.starts_with('/') || cwd.contains('\0') || path.contains('\0') {
        return Err(Error("invalid pathname".into()));
    }
    let input = if path.starts_with('/') {
        String::from(path)
    } else {
        format!("{cwd}/{path}")
    };
    let mut components = Vec::new();
    for component in input.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            _ => components.push(component),
        }
    }
    Ok(format!("/{}", components.join("/")))
}

pub fn host_path(root: &str, guest: &str) -> Result<String> {
    if !root.starts_with('/') || !guest.starts_with('/') {
        return Err(Error("root and guest pathname must be absolute".into()));
    }
    let guest = guest_path("/", guest)?;
    let root = guest_path("/", root)?;
    if root == "/" {
        return Err(Error("the native root cannot be a container rootfs".into()));
    }
    if guest == "/" {
        return Ok(root);
    }
    Ok(format!("{root}{guest}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotdot_never_escapes_the_root() {
        assert_eq!(
            host_path("/bundles/a/rootfs", "/../../etc/passwd").unwrap(),
            "/bundles/a/rootfs/etc/passwd"
        );
        assert_eq!(
            guest_path("/work", "../bin/busybox").unwrap(),
            "/bin/busybox"
        );
        assert_eq!(
            host_path("/bundles/a/rootfs", "/bundles/a/rootfs/secret").unwrap(),
            "/bundles/a/rootfs/bundles/a/rootfs/secret"
        );
        assert!(host_path("/", "/bin/sh").is_err());
        assert!(guest_path("/", "a\0b").is_err());
    }

    #[test]
    fn ids_cannot_be_pathnames() {
        for id in ["", ".", "..", "../a", "a/b", "a\0b"] {
            assert!(!valid_id(id));
        }
        assert!(valid_id("busybox-test.1"));
    }
}
