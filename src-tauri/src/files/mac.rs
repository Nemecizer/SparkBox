//! This Mac's side of the Files window: real files, always (Demo mode only simulates the nodes).

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use sha2::{Digest, Sha256};

use super::transfer::{Placed, Policy, Side};
use super::{paths, Backup, Entry, Listing, SaveError, Saved, TextFile, Trashed, TEXT_LIMIT};

const PEEK: usize = 64 * 1024;
const BACKUPS_KEPT: usize = 5;

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

pub fn path(p: &str) -> PathBuf {
    match p {
        "" | "~" => home(),
        p if p.starts_with("~/") => home().join(&p[2..]),
        p => PathBuf::from(p),
    }
}

fn sha_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn mtime(meta: &fs::Metadata) -> u64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn mode_string(meta: &fs::Metadata) -> String {
    let m = meta.permissions().mode();
    let kind = if meta.file_type().is_symlink() { 'l' } else if meta.is_dir() { 'd' } else { '-' };
    let bits: String = (0..9).map(|i| if m & (0o400 >> i) != 0 { ['r', 'w', 'x'][i % 3] } else { '-' }).collect();
    format!("{kind}{bits}")
}

/// A user or group name for an id, falling back to the number.
fn user_name(uid: u32) -> String {
    let mut buf = vec![0i8; 1024];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe { libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut out) };
    if rc == 0 && !out.is_null() {
        return unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) }.to_string_lossy().into_owned();
    }
    uid.to_string()
}

fn group_name(gid: u32) -> String {
    let mut buf = vec![0i8; 1024];
    let mut grp: libc::group = unsafe { std::mem::zeroed() };
    let mut out: *mut libc::group = std::ptr::null_mut();
    let rc = unsafe { libc::getgrgid_r(gid, &mut grp, buf.as_mut_ptr(), buf.len(), &mut out) };
    if rc == 0 && !out.is_null() {
        return unsafe { std::ffi::CStr::from_ptr(grp.gr_name) }.to_string_lossy().into_owned();
    }
    gid.to_string()
}

/// Free and total bytes of the volume holding `p`.
pub fn space(p: &Path) -> (Option<u64>, Option<u64>) {
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()) else { return (None, None) };
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return (None, None);
    }
    let frag = st.f_frsize as u64;
    (Some(st.f_bavail as u64 * frag), Some(st.f_blocks as u64 * frag))
}

pub fn list(p: &str) -> Result<Listing, String> {
    let dir = path(p).canonicalize().map_err(|e| format!("can't open {p}: {e}"))?;
    let mut entries = Vec::new();
    let mut names = std::collections::HashMap::new();
    for e in fs::read_dir(&dir).map_err(|e| format!("can't open {}: {e}", dir.display()))?.flatten() {
        let Ok(meta) = e.path().symlink_metadata() else { continue };
        let link = meta.file_type().is_symlink();
        let owner = names.entry(meta.uid()).or_insert_with(|| user_name(meta.uid())).clone();
        entries.push(Entry {
            name: e.file_name().to_string_lossy().into(),
            kind: if link { "link" } else if meta.is_dir() { "dir" } else { "file" }.into(),
            size: meta.len(),
            mtime: mtime(&meta),
            mode: mode_string(&meta),
            target: link.then(|| fs::read_link(e.path()).ok().map(|t| t.display().to_string())).flatten(),
            link_dir: link && e.path().is_dir(),
            owner: Some(owner),
        });
    }
    let (free, capacity) = space(&dir);
    Ok(Listing { place: super::MAC.into(), path: dir.display().to_string(), entries, free, capacity })
}

pub fn size(p: &Path) -> u64 {
    match p.symlink_metadata() {
        Ok(m) if m.is_dir() => fs::read_dir(p).map(|rd| rd.flatten().map(|e| size(&e.path())).sum()).unwrap_or(0),
        Ok(m) if m.is_file() => m.len(),
        _ => 0,
    }
}

pub fn read(p: &str) -> Result<TextFile, String> {
    let real = path(p).canonicalize().map_err(|e| format!("can't read {p}: {e}"))?;
    let meta = real.metadata().map_err(|e| format!("can't read {p}: {e}"))?;
    if meta.is_dir() {
        return Err(format!("{} is a folder", real.display()));
    }
    let shown = real.display().to_string();
    let writable = fs::OpenOptions::new().append(true).open(&real).is_ok();
    let info = TextFile {
        path: shown.clone(),
        content: String::new(),
        size: meta.len(),
        mtime: mtime(&meta),
        sha256: String::new(),
        mode: mode_string(&meta),
        owner: format!("{}:{}", user_name(meta.uid()), group_name(meta.gid())),
        writable,
        large: false,
        head: None,
        tail: None,
    };
    let mut f = fs::File::open(&real).map_err(|e| format!("can't read {shown}: {e}"))?;
    if meta.len() > TEXT_LIMIT {
        let mut head = vec![0u8; PEEK];
        let n = f.read(&mut head).map_err(|e| e.to_string())?;
        head.truncate(n);
        if head.contains(&0) {
            return Err(format!("{shown} looks like a binary file"));
        }
        f.seek(SeekFrom::Start(meta.len().saturating_sub(PEEK as u64).max(PEEK as u64))).map_err(|e| e.to_string())?;
        let mut tail = Vec::new();
        f.read_to_end(&mut tail).map_err(|e| e.to_string())?;
        return Ok(TextFile {
            large: true,
            head: Some(String::from_utf8_lossy(&head).into_owned()),
            tail: Some(String::from_utf8_lossy(&tail).into_owned()),
            ..info
        });
    }
    let mut data = Vec::new();
    f.read_to_end(&mut data).map_err(|e| format!("can't read {shown}: {e}"))?;
    if data.contains(&0) {
        return Err(format!("{shown} looks like a binary file"));
    }
    let sha256 = sha_hex(&data);
    let content = String::from_utf8(data).map_err(|_| format!("{shown} isn't UTF-8 text"))?;
    Ok(TextFile { content, sha256, ..info })
}

fn backup_dir(backups: &Path, real: &Path) -> PathBuf {
    let h = sha_hex(real.display().to_string().as_bytes());
    backups.join(&h[..16])
}

pub fn backups(backups: &Path, p: &str) -> Vec<Backup> {
    let Ok(real) = path(p).canonicalize() else { return Vec::new() };
    let name = real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = backup_dir(backups, &real);
    let mut found: Vec<Backup> = fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with(&format!("{name}.")))
                .filter_map(|e| {
                    let m = e.metadata().ok()?;
                    Some(Backup { path: e.path().display().to_string(), mtime: mtime(&m), size: m.len() })
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort_by(|a, b| b.path.cmp(&a.path));
    found
}

fn keep_backup(backups: &Path, real: &Path) -> Result<PathBuf, String> {
    let dir = backup_dir(backups, real);
    fs::create_dir_all(&dir).map_err(|e| format!("can't keep a backup: {e}"))?;
    let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    let _ = fs::write(dir.join("path"), format!("{}\n", real.display()));
    let name = real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let ms = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let dest = dir.join(format!("{name}.{ms:015}"));
    fs::copy(real, &dest).map_err(|e| format!("can't keep a backup: {e}"))?;
    let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(0o600));
    let mut old: Vec<PathBuf> = fs::read_dir(&dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&format!("{name}.")))).collect())
        .unwrap_or_default();
    old.sort();
    for p in old.iter().take(old.len().saturating_sub(BACKUPS_KEPT)) {
        let _ = fs::remove_file(p);
    }
    Ok(dest)
}

/// Saves a text file atomically: a temporary file next to it with the same permissions,
/// flushed to disk, renamed over the original (symlinks are followed to the real file).
/// A file that changed since it was read (`expect`) is left alone and reported.
pub fn commit(p: &str, content: &str, expect: Option<&str>, backups: &Path) -> Result<Saved, SaveError> {
    let target = path(p);
    let real = match target.canonicalize() {
        Ok(r) => r,
        Err(_) => paths::mac_real(&target).map_err(|message| SaveError::Failed { message })?,
    };
    let exists = real.symlink_metadata().is_ok();
    if let Some(expect) = expect.filter(|e| !e.is_empty() && *e != "-") {
        let current = fs::read(&real).ok();
        if current.as_deref().map(sha_hex).as_deref() != Some(expect) {
            let meta = real.metadata().ok();
            return Err(SaveError::ChangedOnDisk {
                message: format!("{} changed on disk after you opened it.", real.display()),
                sha256: current.as_deref().map(sha_hex),
                mtime: meta.as_ref().map(mtime),
                size: meta.map(|m| m.len()),
            });
        }
    }
    let dir = real.parent().ok_or_else(|| SaveError::Failed { message: format!("can't save {}", real.display()) })?;
    let name = real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut nonce = [0u8; 4];
    let _ = getrandom::fill(&mut nonce);
    let tmp = dir.join(format!(".{name}.sparkbox-{:08x}", u32::from_le_bytes(nonce)));
    let mode = real.metadata().map(|m| m.permissions().mode() & 0o7777).unwrap_or(0o644);
    let fail = |e: std::io::Error| SaveError::Failed { message: format!("can't save {}: {e}", real.display()) };
    let result = (|| {
        let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
        drop(f);
        fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
        Ok::<(), std::io::Error>(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(fail(e));
    }
    let backup = if exists { keep_backup(backups, &real).ok() } else { None };
    if let Err(e) = fs::rename(&tmp, &real) {
        let _ = fs::remove_file(&tmp);
        return Err(fail(e));
    }
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    let meta = real.metadata().map_err(fail)?;
    Ok(Saved {
        path: real.display().to_string(),
        sha256: sha_hex(content.as_bytes()),
        mtime: mtime(&meta),
        size: meta.len(),
        backup: backup.map(|b| b.display().to_string()),
    })
}

/// Where the Mac pane's Delete puts things: the real Trash, or a folder in tests (the real one
/// can take minutes on a synced folder).
pub trait Trash: Send + Sync {
    /// Moves `real` to the Trash; where it went, when known.
    fn put(&self, real: &Path) -> Result<Option<PathBuf>, String>;
}

/// The user's Trash: NSFileManager (Finder's Put Back works), or, if that refuses, Finder
/// itself through the `trash` crate.
pub struct SystemTrash;

impl Trash for SystemTrash {
    fn put(&self, real: &Path) -> Result<Option<PathBuf>, String> {
        match ns_trash(real) {
            Ok(dest) => Ok(Some(dest)),
            Err(first) => {
                let ctx = trash::TrashContext::default();
                ctx.delete(real).map_err(|e| format!("can't move {} to the Trash: {first} ({e})", real.display()))?;
                Ok(None)
            }
        }
    }
}

/// A Trash that's a plain folder (named .Trash, as the user's is), for tests.
#[cfg(test)]
pub struct FolderTrash(pub PathBuf);

#[cfg(test)]
impl Trash for FolderTrash {
    fn put(&self, real: &Path) -> Result<Option<PathBuf>, String> {
        fs::create_dir_all(&self.0).map_err(|e| e.to_string())?;
        let name = real.file_name().ok_or("no name")?.to_string_lossy().into_owned();
        let taken = self.0.join(&name).symlink_metadata().is_ok();
        let dest = self.0.join(if taken { free_name(&self.0, &name, real.is_dir()) } else { name });
        fs::rename(real, &dest).map_err(|e| e.to_string())?;
        Ok(Some(dest))
    }
}

/// Moves an item to the Trash (`bin`); says where it went when known.
pub fn trash(p: &str, bin: &dyn Trash) -> Result<Trashed, String> {
    let real = paths::mac_removable(&path(p), &home())?;
    let dest = bin.put(&real)?;
    Ok(Trashed { path: real.display().to_string(), trashed: dest.map(|d| d.display().to_string()) })
}

#[cfg(target_os = "macos")]
fn ns_trash(real: &Path) -> Result<PathBuf, String> {
    use objc2_foundation::{NSFileManager, NSString, NSURL};
    objc2::rc::autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&real.display().to_string()));
        let manager = NSFileManager::defaultManager();
        let mut out = None;
        manager.trashItemAtURL_resultingItemURL_error(&url, Some(&mut out)).map_err(|e| e.localizedDescription().to_string())?;
        out.and_then(|u| u.path()).map(|s| PathBuf::from(s.to_string())).ok_or_else(|| "the Trash didn't say where the item went".into())
    })
}

#[cfg(not(target_os = "macos"))]
fn ns_trash(_real: &Path) -> Result<PathBuf, String> {
    Err("no NSFileManager".into())
}

/// Puts a trashed item back where it was (Undo).
pub fn untrash(item: &Trashed) -> Result<(), String> {
    let trashed = PathBuf::from(item.trashed.as_deref().ok_or("SparkBox doesn't know where that item went; use Finder's Put Back.")?);
    let original = PathBuf::from(&item.path);
    if !trashed.components().any(|c| matches!(c.as_os_str().to_str(), Some(".Trash") | Some(".Trashes"))) {
        return Err(format!("{} isn't in the Trash.", trashed.display()));
    }
    if original.symlink_metadata().is_ok() {
        return Err(format!("{} already exists, so the trashed copy was left in the Trash.", original.display()));
    }
    if trashed.symlink_metadata().is_err() {
        return Err(format!("{} is no longer in the Trash.", trashed.display()));
    }
    if let Some(parent) = original.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("can't recreate {}: {e}", parent.display()))?;
    }
    fs::rename(&trashed, &original).map_err(|e| format!("can't put {} back: {e}", original.display()))
}

pub fn remove(p: &str) -> Result<(), String> {
    let real = paths::mac_removable(&path(p), &home())?;
    let meta = real.symlink_metadata().map_err(|e| format!("can't delete {}: {e}", real.display()))?;
    if meta.is_dir() { fs::remove_dir_all(&real) } else { fs::remove_file(&real) }.map_err(|e| format!("can't delete {}: {e}", real.display()))
}

/// Facts about `dir/name` for a transfer's pre-flight check.
pub fn side(dir: &Path, name: &str) -> Option<Side> {
    let p = dir.join(name);
    let meta = p.symlink_metadata().ok()?;
    let (mut bytes, mut entries, mut tar) = (0u64, 0u64, 0u64);
    fn walk(p: &Path, rel_len: usize, bytes: &mut u64, entries: &mut u64, tar: &mut u64) {
        let Ok(m) = p.symlink_metadata() else { return };
        *entries += 1;
        *tar += 512 + if rel_len > 99 { 1024 } else { 0 };
        if m.is_file() {
            *bytes += m.len();
            *tar += m.len().div_ceil(512) * 512;
        }
        if m.is_dir() {
            for e in fs::read_dir(p).into_iter().flatten().flatten() {
                walk(&e.path(), rel_len + 1 + e.file_name().len(), bytes, entries, tar);
            }
        }
    }
    walk(&p, name.len(), &mut bytes, &mut entries, &mut tar);
    Some(Side {
        kind: if meta.file_type().is_symlink() { "link" } else if meta.is_dir() { "dir" } else { "file" }.into(),
        bytes,
        entries,
        mtime: mtime(&meta),
        tar_bytes: tar,
    })
}

fn move_item(src: &Path, dst: &Path) -> std::io::Result<()> {
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            copy_tree(src, dst)?;
            remove_any(src)
        }
        Err(e) => Err(e),
    }
}

fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    let meta = src.symlink_metadata()?;
    if meta.file_type().is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(src)?, dst)
    } else if meta.is_dir() {
        fs::create_dir(dst)?;
        for e in fs::read_dir(src)? {
            let e = e?;
            copy_tree(&e.path(), &dst.join(e.file_name()))?;
        }
        fs::set_permissions(dst, meta.permissions())
    } else {
        fs::copy(src, dst).map(|_| ())
    }
}

fn remove_any(p: &Path) -> std::io::Result<()> {
    let meta = p.symlink_metadata()?;
    if meta.is_dir() { fs::remove_dir_all(p) } else { fs::remove_file(p) }
}

fn is_real_dir(p: &Path) -> bool {
    p.symlink_metadata().is_ok_and(|m| m.is_dir())
}

fn merge(src: &Path, dst: &Path, newer_only: bool) -> std::io::Result<()> {
    let (sd, dd) = (is_real_dir(src), is_real_dir(dst));
    if sd && dd {
        for e in fs::read_dir(src)? {
            let e = e?;
            merge(&e.path(), &dst.join(e.file_name()), newer_only)?;
        }
        return Ok(());
    }
    if let Ok(dm) = dst.symlink_metadata() {
        let sm = src.symlink_metadata()?;
        if newer_only && sm.modified()? <= dm.modified()? {
            return Ok(());
        }
        if dd || sd {
            remove_any(dst)?;
        }
    }
    move_item(src, dst)
}

/// A free "name 2" (or "name 2.ext") in `dir`, Finder style.
pub fn free_name(dir: &Path, name: &str, is_dir: bool) -> String {
    let (stem, ext) = match (is_dir, name.rfind('.')) {
        (false, Some(i)) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (2..).map(|n| format!("{stem} {n}{ext}")).find(|c| dir.join(c).symlink_metadata().is_err()).unwrap()
}

/// Puts items from `source` into `dest` by policy; the Mac twin of sparkjob.py `place`.
pub fn place(source: &Path, dest: &Path, items: &[(String, Option<Policy>)], nonce: &str, moving: bool) -> Result<Vec<Placed>, String> {
    fs::create_dir_all(dest).map_err(|e| format!("can't create {}: {e}", dest.display()))?;
    let mut results = Vec::new();
    for (name, policy) in items {
        let (src, dst) = (source.join(name), dest.join(name));
        let placed = |action: &str, as_name: &str| Placed { name: name.clone(), action: action.into(), as_name: as_name.into() };
        if src.symlink_metadata().is_err() || *policy == Some(Policy::Skip) || src == dst {
            results.push(placed("skipped", name));
            continue;
        }
        let exists = dst.symlink_metadata().is_ok();
        let policy = match policy {
            None if exists => Some(Policy::KeepBoth),
            p => *p,
        };
        let err = |e: std::io::Error| format!("can't put {name} into {}: {e}", dest.display());
        if !exists {
            move_item(&src, &dst).map_err(err)?;
            results.push(placed("copied", name));
            continue;
        }
        match policy {
            Some(Policy::KeepBoth) | None => {
                let other = free_name(dest, name, is_real_dir(&src));
                move_item(&src, &dest.join(&other)).map_err(err)?;
                results.push(placed("keptBoth", &other));
            }
            Some(Policy::Replace) => {
                let old = dest.join(format!(".{name}.sparkbox-old-{nonce}"));
                fs::rename(&dst, &old).map_err(err)?;
                if let Err(e) = move_item(&src, &dst) {
                    let _ = fs::rename(&old, &dst);
                    return Err(err(e));
                }
                let _ = remove_any(&old);
                results.push(placed("replaced", name));
            }
            Some(p @ (Policy::Merge | Policy::ReplaceIfNewer)) => {
                merge(&src, &dst, p == Policy::ReplaceIfNewer).map_err(err)?;
                if moving && src.symlink_metadata().is_ok() {
                    let _ = remove_any(&src);
                }
                results.push(placed("merged", name));
            }
            Some(Policy::Skip) => unreachable!(),
        }
    }
    Ok(results)
}

// ---- Where copies may land on this Mac, and which folders hold secrets

/// What a copy into a folder on this Mac is refused with when it would land in a login item,
/// a shell startup file or ~/.ssh, Mac scope or not.
pub const PERSISTENCE: &str = "SparkBox doesn't copy into login items, shell startup files or ~/.ssh.";
/// Copies land in the home folder or on a volume in /Volumes.
pub const OUTSIDE: &str = "Copy into a folder in your home folder or on a volume in /Volumes.";
/// Shell startup files at the top of the home folder: what zsh and bash read at login, in
/// every shell and at logout.
const STARTUP_FILES: [&str; 10] = [".zshenv", ".zshrc", ".zprofile", ".zlogin", ".zlogout", ".bash_profile", ".bash_login", ".bash_logout", ".bashrc", ".profile"];
/// Paths in the home folder that hold keys or credentials: listing and reading them needs Mac
/// scope, and du/preflight/backups refuse them (P1.46). Folders cover everything under them;
/// the files cover just themselves.
const SECRET_DIRS: &[&str] = &[
    ".ssh", "Library/Keychains", ".gnupg", ".aws", ".config/gh", ".kube", ".config/gcloud", ".azure", ".netrc",
    ".git-credentials", ".cache/huggingface/token", ".docker/config.json",
];
/// The same keys-and-credentials paths on a node (home-relative), plus the two the owner keeps
/// there (~/hf_token.txt, ~/glm53/api_key). Keychains is Mac-only.
const NODE_SECRETS: &[&str] = &[
    ".ssh", ".gnupg", ".aws", ".config/gh", ".config/gcloud", ".azure", ".kube", ".netrc", ".git-credentials",
    ".cache/huggingface/token", ".docker/config.json", "hf_token.txt", "glm53/api_key",
];
/// What du, preflight and backups refuse a keys-and-credentials path with (P1.46).
pub const SECRET_PATH: &str = "SparkBox won't measure, preview or list versions of a path that may hold keys or credentials.";

/// Whether `path` on `place` is a keys-and-credentials path or inside one (P1.46): on this Mac
/// a key folder or file, on a node a node secret. Listing and reading it needs Mac scope, and
/// du and backups refuse it. A folder that only holds one (~, ~/.cache) is not: its size or its
/// listing shows no secret.
pub fn is_secret_at(place: &str, path: &str, mac_home: &Path) -> bool {
    if place == super::MAC {
        is_secret_path(mac_home, path)
    } else {
        is_node_secret(path)
    }
}

/// Whether reading `path` on `place` whole, as a copy's preflight does, takes keys or
/// credentials along: a secret path, or on this Mac a folder that holds one (copying ~ takes
/// ~/.ssh with it). Preflight refuses these (P1.46).
pub fn secret_read(place: &str, path: &str, mac_home: &Path) -> bool {
    is_secret_at(place, path, mac_home) || (place == super::MAC && holds_secrets(mac_home, path))
}

/// A node path (under ~, /root or /home/<user>) that holds keys or credentials: listing and
/// reading it needs Mac scope, and du, preflight and backups refuse it. Lexical, like the
/// node's own protected-path rule.
pub fn is_node_secret(path: &str) -> bool {
    let c = super::paths::clean(path.trim());
    let rel = if let Some(r) = c.strip_prefix("/~/") {
        r.to_string()
    } else if c == "/~" {
        String::new()
    } else if let Some(r) = c.strip_prefix("/root/") {
        r.to_string()
    } else if let Some((_, r)) = c.strip_prefix("/home/").and_then(|rest| rest.split_once('/')) {
        r.to_string()
    } else {
        return false;
    };
    NODE_SECRETS.iter().any(|s| rel == *s || rel.starts_with(&format!("{s}/")))
}

/// Where a copy lands: ~/Downloads (and below) is a download, anywhere else a change on the Mac.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Landing {
    Downloads,
    Elsewhere,
}

/// `p` with symlinks resolved as far as it exists (the rest appended as written).
fn resolved(p: &Path) -> PathBuf {
    let mut tail = Vec::new();
    let mut at = p.to_path_buf();
    loop {
        if let Ok(real) = at.canonicalize() {
            return tail.into_iter().rev().fold(real, |acc: PathBuf, part| acc.join(part));
        }
        match (at.file_name().map(|n| n.to_os_string()), at.parent().map(Path::to_path_buf)) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                at = parent;
            }
            _ => return p.to_path_buf(),
        }
    }
}

/// The places nothing is ever copied into: login items, the system Library, ~/.ssh, fish's
/// config folder and the shell startup files at the top of `home`.
fn persistence_paths(home: &Path) -> Vec<PathBuf> {
    let mut out = vec![home.join("Library/LaunchAgents"), home.join("Library/LaunchDaemons"), PathBuf::from("/Library"), home.join(".ssh"), home.join(".config/fish")];
    out.extend(STARTUP_FILES.iter().map(|f| home.join(f)));
    out
}

/// `a` is `b` or inside it, ignoring case: APFS on a Mac is case-insensitive, so ~/.ZSHRC is
/// ~/.zshrc. Both are compared after `resolved` canonicalised what exists of them.
fn under(a: &Path, b: &Path) -> bool {
    let lower = |p: &Path| p.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect::<Vec<_>>();
    let (a, b) = (lower(a), lower(b));
    a.len() >= b.len() && a[..b.len()] == b[..]
}

/// Checks a copy of `names` into `to_dir` on this Mac: the folder must be absolute and in
/// `home` or /Volumes (symlinks resolved), and no item may land on, in or above a login item,
/// a shell startup file or ~/.ssh (copying a folder called Library into ~ would merge into
/// ~/Library/LaunchAgents). Returns whether it's a download.
pub fn landing(home: &Path, to_dir: &str, names: &[String]) -> Result<Landing, String> {
    let dir = match to_dir {
        "~" => home.to_path_buf(),
        d if d.starts_with("~/") => home.join(&d[2..]),
        d if d.starts_with('/') => PathBuf::from(d),
        _ => return Err(OUTSIDE.into()),
    };
    if dir.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(OUTSIDE.into());
    }
    let real = resolved(&dir);
    let home = resolved(home);
    if !(under(&real, &home) || real.starts_with("/Volumes/")) {
        return Err(OUTSIDE.into());
    }
    let banned: Vec<PathBuf> = persistence_paths(&home);
    for name in names {
        let dest = resolved(&real.join(name));
        if banned.iter().any(|b| under(&dest, b) || under(b, &dest)) {
            return Err(PERSISTENCE.into());
        }
    }
    if banned.iter().any(|b| under(&real, b)) {
        return Err(PERSISTENCE.into());
    }
    Ok(if under(&real, &home.join("Downloads")) { Landing::Downloads } else { Landing::Elsewhere })
}

/// Whether `p` is in (or is) a folder of keys and credentials in `home` (~/.ssh, the
/// Keychains, ~/.gnupg, ~/.aws, ~/.config/gh, ~/.kube), symlinks resolved.
pub fn is_secret_path(home: &Path, p: &str) -> bool {
    let (real, home) = (resolved(&expand_home(home, p)), resolved(home));
    SECRET_DIRS.iter().any(|d| under(&real, &home.join(d)))
}

/// Whether copying `p` off this Mac reads keys or credentials: it is in a key folder, or one is
/// inside it (copying ~, ~/Library, ~/.config or /Users takes ~/.ssh along). Case is ignored
/// and symlinks are resolved.
pub fn holds_secrets(home: &Path, p: &str) -> bool {
    let (real, home) = (resolved(&expand_home(home, p)), resolved(home));
    SECRET_DIRS.iter().any(|d| {
        let secret = home.join(d);
        under(&real, &secret) || under(&secret, &real)
    })
}

fn expand_home(home: &Path, p: &str) -> PathBuf {
    match p {
        "" | "~" => home.to_path_buf(),
        p if p.starts_with("~/") => home.join(&p[2..]),
        p => PathBuf::from(p),
    }
}

/// A copy that arrived in ~/Downloads is treated like a browser download: every file and
/// folder gets the com.apple.quarantine flag (Gatekeeper asks before anything in it runs) and
/// files lose their execute bits. Symlinks are left alone, never followed.
pub fn quarantine(root: &Path) -> Result<(), String> {
    let stamp = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let value = format!("0083;{stamp:08x};SparkBox;");
    fn walk(p: &Path, value: &str) -> Result<(), String> {
        let meta = p.symlink_metadata().map_err(|e| format!("can't read {}: {e}", p.display()))?;
        if meta.file_type().is_symlink() {
            return Ok(());
        }
        if meta.is_file() {
            let mode = meta.permissions().mode() & 0o7777 & !0o111 & !0o6000;
            fs::set_permissions(p, fs::Permissions::from_mode(mode)).map_err(|e| format!("can't change {}: {e}", p.display()))?;
        }
        set_quarantine(p, value)?;
        if meta.is_dir() {
            for e in fs::read_dir(p).map_err(|e| format!("can't open {}: {e}", p.display()))?.flatten() {
                walk(&e.path(), value)?;
            }
        }
        Ok(())
    }
    walk(root, &value)
}

fn set_quarantine(p: &Path, value: &str) -> Result<(), String> {
    let path = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).map_err(|_| format!("{} has a NUL in its name", p.display()))?;
    let name = c"com.apple.quarantine";
    // SAFETY: both strings are NUL-terminated and live across the call; value has the given length.
    let rc = unsafe { libc::setxattr(path.as_ptr(), name.as_ptr(), value.as_ptr().cast(), value.len(), 0, libc::XATTR_NOFOLLOW) };
    if rc != 0 {
        return Err(format!("can't mark {} as downloaded: {}", p.display(), std::io::Error::last_os_error()));
    }
    Ok(())
}

/// The quarantine flag on `p`, if any.
#[cfg(test)]
pub fn quarantine_of(p: &Path) -> Option<String> {
    let path = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).ok()?;
    let name = c"com.apple.quarantine";
    let mut buf = [0u8; 256];
    // SAFETY: buf is writable for its length; the strings are NUL-terminated.
    let n = unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), buf.as_mut_ptr().cast(), buf.len(), 0, libc::XATTR_NOFOLLOW) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

/// What the activity log records about an item placed on this Mac: its path, size and
/// SHA-256. A folder's hash covers every file in it ("path\0size\0sha256\n", sorted).
pub fn fingerprint(p: &Path) -> (u64, u64, String) {
    fn file_sha(p: &Path) -> String {
        let Ok(mut f) = fs::File::open(p) else { return String::new() };
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        while let Ok(n) = f.read(&mut buf) {
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }
    let Ok(meta) = p.symlink_metadata() else { return (0, 0, String::new()) };
    if meta.is_file() {
        return (meta.len(), 1, file_sha(p));
    }
    let mut lines = Vec::new();
    fn walk(p: &Path, rel: &str, lines: &mut Vec<(String, u64, String)>) {
        let Ok(m) = p.symlink_metadata() else { return };
        if m.is_file() {
            lines.push((rel.to_string(), m.len(), file_sha(p)));
        } else if m.is_dir() {
            for e in fs::read_dir(p).into_iter().flatten().flatten() {
                walk(&e.path(), &format!("{rel}/{}", e.file_name().to_string_lossy()), lines);
            }
        }
    }
    walk(p, "", &mut lines);
    lines.sort();
    let mut h = Sha256::new();
    for (rel, size, sha) in &lines {
        h.update(format!("{rel}\0{size}\0{sha}\n").as_bytes());
    }
    (lines.iter().map(|l| l.1).sum(), lines.len() as u64, h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
