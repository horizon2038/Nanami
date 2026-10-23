use crate::{guest_path, Error, Result};
use alloc::{collections::BTreeMap, format, string::String, vec::Vec};
use serde_json::{Map, Value};

pub const CONFIG_MAX_BYTES: usize = 32 * 1024;
pub const STRING_COUNT_MAX: usize = 64;
// Leave room for argv/envp, auxv and alignment in Alter's 16-KiB initial stack.
pub const LAUNCH_STRING_MAX_BYTES: usize = 12 * 1024;
pub const ANNOTATION_MAX_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug)]
pub struct Config {
    pub root: String,
    pub readonly: bool,
    pub cwd: String,
    pub args: Vec<String>,
    pub env: Vec<String>,
    pub annotations: BTreeMap<String, String>,
}

impl Config {
    /// Unsupported isolation requirements are errors, never silently ignored.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > CONFIG_MAX_BYTES {
            return Err(Error("config.json exceeds 32 KiB".into()));
        }
        let json: Value =
            serde_json::from_slice(bytes).map_err(|e| Error(format!("config.json: {e}")))?;
        let top = object(&json, "config")?;
        fields(
            top,
            &[
                "ociVersion",
                "root",
                "process",
                "annotations",
                "mounts",
                "hooks",
                "linux",
                "hostname",
                "domainname",
            ],
        )?;
        version(text(top, "ociVersion")?)?;
        let root = object(required(top, "root")?, "root")?;
        fields(root, &["path", "readonly"])?;
        let root_path = text(root, "path")?;
        if root_path.is_empty() || root_path.contains('\0') {
            return Err(Error("root.path must be a nonempty pathname".into()));
        }
        let process = object(required(top, "process")?, "process")?;
        fields(
            process,
            &[
                "terminal",
                "consoleSize",
                "user",
                "args",
                "env",
                "cwd",
                "noNewPrivileges",
                "capabilities",
                "rlimits",
            ],
        )?;
        if boolean(process, "terminal", false)? {
            return Err(Error(
                "process.terminal: PTY allocation is not supported".into(),
            ));
        }
        // OCI requires consoleSize to be ignored when terminal is false.
        boolean(process, "noNewPrivileges", false)?;
        let user = object(required(process, "user")?, "process.user")?;
        fields(user, &["uid", "gid", "additionalGids", "umask"])?;
        for key in ["uid", "gid"] {
            if required(user, key)?.as_u64() != Some(0) {
                return Err(Error(format!(
                    "process.user.{key}: only uid/gid 0 are supported"
                )));
            }
        }
        empty_array(user, "additionalGids")?;
        if let Some(mask) = user.get("umask") {
            if mask.as_u64() != Some(0o022) {
                return Err(Error("process.user.umask: only 022 is supported".into()));
            }
        }
        if let Some(capabilities) = process.get("capabilities") {
            let capabilities = object(capabilities, "process.capabilities")?;
            fields(
                capabilities,
                &[
                    "bounding",
                    "effective",
                    "inheritable",
                    "permitted",
                    "ambient",
                ],
            )?;
            for key in capabilities.keys() {
                empty_array(capabilities, key)?;
            }
        }
        empty_array(process, "rlimits")?;
        let cwd = text(process, "cwd")?;
        if !cwd.starts_with('/') {
            return Err(Error("process.cwd must be absolute".into()));
        }
        let cwd = guest_path("/", cwd)?;
        let args = strings(process, "args", true)?;
        let env = strings(process, "env", false)?;
        if args
            .iter()
            .chain(&env)
            .map(|value| value.len() + 1)
            .sum::<usize>()
            > LAUNCH_STRING_MAX_BYTES
        {
            return Err(Error("process args/env exceed 12 KiB".into()));
        }
        if env
            .iter()
            .any(|entry| !entry.contains('=') || entry.starts_with('='))
        {
            return Err(Error("process.env entries must be NAME=value".into()));
        }
        empty_array(top, "mounts")?;
        for key in ["hostname", "domainname"] {
            if let Some(value) = top.get(key) {
                if value.as_str() != Some("") {
                    return Err(Error(format!("{key}: UTS namespaces are not supported")));
                }
            }
        }
        if let Some(hooks) = top.get("hooks") {
            let hooks = object(hooks, "hooks")?;
            fields(
                hooks,
                &[
                    "prestart",
                    "createRuntime",
                    "createContainer",
                    "startContainer",
                    "poststart",
                    "poststop",
                ],
            )?;
            for key in hooks.keys() {
                empty_array(hooks, key)?;
            }
        }
        if let Some(linux) = top.get("linux") {
            let linux = object(linux, "linux")?;
            fields(
                linux,
                &["namespaces", "devices", "maskedPaths", "readonlyPaths"],
            )?;
            for key in linux.keys() {
                empty_array(linux, key)?;
            }
        }
        let mut annotations = BTreeMap::new();
        if let Some(value) = top.get("annotations") {
            let values = object(value, "annotations")?;
            if values.len() > STRING_COUNT_MAX {
                return Err(Error("annotations exceed 64 entries".into()));
            }
            let mut bytes = 0usize;
            for (key, value) in values {
                let value = value
                    .as_str()
                    .ok_or_else(|| Error("annotations must contain strings".into()))?;
                bytes += key.len() + value.len();
                if bytes > ANNOTATION_MAX_BYTES {
                    return Err(Error("annotations exceed 8 KiB".into()));
                }
                annotations.insert(key.clone(), value.into());
            }
        }
        Ok(Self {
            root: root_path.into(),
            readonly: boolean(root, "readonly", false)?,
            cwd,
            args,
            env,
            annotations,
        })
    }
}

fn object<'a>(value: &'a Value, name: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| Error(format!("{name} must be an object")))
}

fn fields(object: &Map<String, Value>, allowed: &[&str]) -> Result<()> {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(Error(format!("unsupported configuration field: {key}")));
        }
    }
    Ok(())
}

fn required<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a Value> {
    object
        .get(key)
        .ok_or_else(|| Error(format!("missing required field: {key}")))
}

fn text<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    required(object, key)?
        .as_str()
        .ok_or_else(|| Error(format!("{key} must be a string")))
}

fn boolean(object: &Map<String, Value>, key: &str, default: bool) -> Result<bool> {
    match object.get(key) {
        Some(value) => value
            .as_bool()
            .ok_or_else(|| Error(format!("{key} must be a boolean"))),
        None => Ok(default),
    }
}

fn strings(object: &Map<String, Value>, key: &str, nonempty: bool) -> Result<Vec<String>> {
    let Some(value) = object.get(key) else {
        if nonempty {
            return Err(Error(format!("missing required field: {key}")));
        }
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .ok_or_else(|| Error(format!("{key} must be an array")))?;
    if array.len() > STRING_COUNT_MAX || (nonempty && array.is_empty()) {
        return Err(Error(format!(
            "{key}: expected {}..64 strings",
            usize::from(nonempty)
        )));
    }
    let mut result = Vec::new();
    for value in array {
        let value = value
            .as_str()
            .ok_or_else(|| Error(format!("{key} must contain strings")))?;
        if value.contains('\0') || (nonempty && result.is_empty() && value.is_empty()) {
            return Err(Error(format!("{key}: invalid string")));
        }
        result.push(value.into());
    }
    Ok(result)
}

fn empty_array(object: &Map<String, Value>, key: &str) -> Result<()> {
    if let Some(value) = object.get(key) {
        if !value.as_array().is_some_and(|array| array.is_empty()) {
            return Err(Error(format!("{key}: requested feature is not supported")));
        }
    }
    Ok(())
}

fn version(version: &str) -> Result<()> {
    let parts: Vec<_> = version.split('.').collect();
    if parts.len() != 3
        || parts.iter().any(|part| {
            part.is_empty()
                || !part.bytes().all(|b| b.is_ascii_digit())
                || (part.len() > 1 && part.starts_with('0'))
        })
    {
        return Err(Error(
            "ociVersion must be a supported SemVer release".into(),
        ));
    }
    if parts[0] != "1"
        || !matches!(parts[1], "0" | "1" | "2" | "3")
        || parts[2].parse::<u32>().is_err()
    {
        return Err(Error("unsupported OCI specification version".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn minimal() -> Value {
        serde_json::json!({"ociVersion":"1.3.0", "root":{"path":"rootfs"}, "process":{"user":{"uid":0,"gid":0},"cwd":"/work","args":["/bin/busybox","echo","hello"],"env":["PATH=/bin"]}})
    }
    #[test]
    fn preserves_guest_argv_environment_and_defaults() {
        let config = Config::parse(&serde_json::to_vec(&minimal()).unwrap()).unwrap();
        assert_eq!(config.args[0], "/bin/busybox");
        assert_eq!(config.cwd, "/work");
        assert!(!config.readonly);
        assert_eq!(config.env, ["PATH=/bin"]);
    }
    #[test]
    fn checked_in_busybox_sample_matches_the_supported_profile() {
        let config = Config::parse(include_bytes!(
            "../../../samples/negoto/bundles/busybox/config.json"
        ))
        .unwrap();
        assert_eq!(config.root, "rootfs");
        assert!(config.readonly);
        assert_eq!(config.cwd, "/");
        assert_eq!(config.args, ["/bin/busybox", "sh", "-i"]);
        assert_eq!(config.env, ["PATH=/bin:/usr/bin"]);
    }
    #[test]
    fn refuses_requested_isolation_instead_of_ignoring_it() {
        for field in ["resources", "seccomp", "uidMappings", "rootfsPropagation"] {
            let mut json = minimal();
            json["linux"] = serde_json::json!({field: {}});
            assert!(
                Config::parse(&serde_json::to_vec(&json).unwrap()).is_err(),
                "{field}"
            );
        }
        let mut json = minimal();
        json["linux"] = serde_json::json!({"namespaces":[{"type":"network"}]});
        assert!(Config::parse(&serde_json::to_vec(&json).unwrap()).is_err());
        json = minimal();
        json["process"]["capabilities"] = serde_json::json!({"effective":["CAP_SYS_ADMIN"]});
        assert!(Config::parse(&serde_json::to_vec(&json).unwrap()).is_err());
    }
    #[test]
    fn checks_types_paths_credentials_and_required_values() {
        for (key, value) in [
            ("args", serde_json::json!([])),
            ("cwd", serde_json::json!("relative")),
            ("env", serde_json::json!(["BAD"])),
        ] {
            let mut json = minimal();
            json["process"][key] = value;
            assert!(Config::parse(&serde_json::to_vec(&json).unwrap()).is_err());
        }
        let mut json = minimal();
        json["root"]["readonly"] = Value::String("false".into());
        assert!(Config::parse(&serde_json::to_vec(&json).unwrap()).is_err());
    }
    #[test]
    fn bounds_stack_and_persistent_metadata_before_preparing_a_process() {
        let mut json = minimal();
        json["process"]["args"] =
            serde_json::json!(["/bin/busybox", "x".repeat(LAUNCH_STRING_MAX_BYTES)]);
        assert!(Config::parse(&serde_json::to_vec(&json).unwrap()).is_err());
        let mut json = minimal();
        json["annotations"] = serde_json::json!({"large":"x".repeat(ANNOTATION_MAX_BYTES)});
        assert!(Config::parse(&serde_json::to_vec(&json).unwrap()).is_err());
        let mut json = minimal();
        json["process"]["args"] = serde_json::json!(["/bin/busybox", ""]);
        assert_eq!(
            Config::parse(&serde_json::to_vec(&json).unwrap())
                .unwrap()
                .args[1],
            ""
        );
    }
}
