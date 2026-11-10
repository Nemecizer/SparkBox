//! Paths the Files window acts on: lexical clean-up and the rule for what may never be
//! deleted. The same rule runs on the nodes (sparkjob.py `protected`), after `realpath`.

use std::path::{Path, PathBuf};

/// The system's own folders. Anything directly inside "/" is refused as well.
const PROTECTED: &[&str] = &[
    "/", "/bin", "/boot", "/dev", "/etc", "/home", "/lib", "/lib64", "/opt", "/proc", "/root", "/run", "/sbin", "/snap",
    "/srv", "/sys", "/tmp", "/usr", "/var", "/mnt", "/media", "/Applications", "/Library", "/System", "/Users", "/Volumes",
    "/private/etc", "/private/var", "/private/tmp",
];
/// Everything inside these is part of the OS (except /usr/local).
const SYSTEM_TREES: &[&str] = &["/bin", "/boot", "/dev", "/lib", "/lib64", "/proc", "/sbin", "/sys", "/usr", "/snap", "/System"];
/// Their direct children are system folders too (/var/lib, /opt/nvidia, /Library/Preferences).
const SYSTEM_PARENTS: &[&str] = &["/usr", "/var", "/opt", "/Library", "/private", "/private/var"];

/// "//etc", "/etc/." and "/usr/../etc" all become "/etc"; a trailing slash goes. Purely lexical:
/// symlinks are resolved where the file lives (canonicalize on the Mac, realpath on a node).
pub fn clean(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    format!("/{}", parts.join("/"))
}

pub fn is_protected(real: &str, home: &str) -> bool {
    let real = clean(real);
    let home = clean(home);
    let parent = match real.rfind('/') {
        Some(0) | None => "/",
        Some(i) => &real[..i],
    };
    if PROTECTED.contains(&real.as_str()) || parent == "/" || SYSTEM_PARENTS.contains(&parent) {
        return true;
    }
    // The home folder and everything above it.
    if real == home || home.starts_with(&format!("{}/", real.trim_end_matches('/'))) {
        return true;
    }
    if real.starts_with("/usr/local/") {
        return false;
    }
    SYSTEM_TREES.iter().any(|t| real.starts_with(&format!("{t}/")))
}

/// The real path of an item on this Mac, without following a symlink at the end (deleting a
/// link removes the link, not what it points to).
pub fn mac_real(path: &Path) -> Result<PathBuf, String> {
    let shown = path.display().to_string();
    if !path.is_absolute() {
        return Err(format!("SparkBox won't delete {shown}: it isn't an absolute path."));
    }
    let cleaned = PathBuf::from(clean(&shown));
    match (cleaned.parent(), cleaned.file_name()) {
        (Some(parent), Some(name)) => {
            let parent = parent.canonicalize().map_err(|e| format!("can't find {}: {e}", parent.display()))?;
            Ok(parent.join(name))
        }
        _ => Ok(PathBuf::from("/")),
    }
}

/// Checks a Mac item may be deleted or trashed, returning its real path.
pub fn mac_removable(path: &Path, home: &Path) -> Result<PathBuf, String> {
    let real = mac_real(path)?;
    let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    if is_protected(&real.display().to_string(), &home.display().to_string()) {
        return Err(format!("SparkBox won't delete {}.", real.display()));
    }
    if real.symlink_metadata().is_err() {
        return Err(format!("{} doesn't exist.", real.display()));
    }
    Ok(real)
}

/// Checks a node path before it's sent (the node checks again after realpath).
pub fn node_removable(path: &str, home: Option<&str>) -> Result<String, String> {
    if !path.starts_with('/') && !path.starts_with("~/") {
        return Err(format!("SparkBox won't delete {path}: it isn't an absolute path."));
    }
    let expanded = match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(h)) => format!("{h}/{rest}"),
        _ => path.to_string(),
    };
    let real = clean(&expanded);
    if is_protected(&real, home.unwrap_or("/home")) {
        return Err(format!("SparkBox won't delete {real}."));
    }
    Ok(real)
}
