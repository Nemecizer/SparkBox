//! Files against fake nodes that run the real commands (sparkjob.py, tar, mv…) in temporary
//! homes, plus the Mac side on temporary folders.

use std::collections::HashMap;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::transfer::{Op, Policy, TransferSpec};
use super::*;
use crate::fake_node::{connect, fake_node, real_shell, temp, Script};
use super::transfer;

/// Runs a command like the fake would, with the given stdin (so a script can cut it short).
fn run_real(cmd: &str, stdin: &[u8], home: &Path, env: &[(&str, &str)]) -> (String, u32) {
    use std::io::Write;
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .env("HOME", home)
        .envs(env.iter().copied())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(stdin);
    let out = child.wait_with_output().unwrap();
    let code = out.status.code().unwrap_or(137) as u32;
    if code == 0 {
        (String::from_utf8_lossy(&out.stdout).into(), 0)
    } else {
        (format!("!stderr:{}", String::from_utf8_lossy(&out.stderr)), code)
    }
}

/// Nodes that run everything for real (sudo is the fake node's own shim: password checked, no root).
async fn setup(name: &str, nodes: usize) -> (Arc<Files>, Vec<crate::fake_node::Node>, Vec<PathBuf>) {
    setup_with(name, nodes, |_| real_shell()).await
}

async fn setup_with(name: &str, nodes: usize, script: impl Fn(PathBuf) -> Script) -> (Arc<Files>, Vec<crate::fake_node::Node>, Vec<PathBuf>) {
    let mut fakes = Vec::new();
    let mut homes = Vec::new();
    let mut ports = Vec::new();
    for i in 0..nodes {
        let home = temp(&format!("files-{name}-home{i}"));
        let (node, port) = fake_node(&home, if i == 0 { script(home.clone()) } else { real_shell() }).await;
        fakes.push(node);
        homes.push(home);
        ports.push(port);
    }
    let ids = ["cell1", "cell2"];
    let named: Vec<(&str, u16)> = ports.iter().enumerate().map(|(i, p)| (ids[i], *p)).collect();
    let app = temp(&format!("files-{name}-app"));
    let (live, _) = connect(&app, &named).await;
    let files = Files::new(live, app.join("backups"));
    // Copies onto "this Mac" land in temp folders: count those as the home folder.
    files.set_mac_home(std::env::temp_dir().canonicalize().unwrap());
    (files, fakes, homes)
}

fn collector() -> (Emit, Arc<Mutex<Vec<TransferEvent>>>) {
    let seen: Arc<Mutex<Vec<TransferEvent>>> = Arc::default();
    let s = seen.clone();
    (Box::new(move |e| s.lock().unwrap().push(e)), seen)
}

async fn finished(seen: &Arc<Mutex<Vec<TransferEvent>>>) -> TransferEvent {
    for _ in 0..1500 {
        if let Some(e) = seen.lock().unwrap().iter().find(|e| !matches!(e, TransferEvent::Progress { .. })) {
            return e.clone();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("transfer never finished: {:?}", seen.lock().unwrap());
}

fn spec(from: &str, dir: &Path, names: &[&str], to: &str, to_dir: &Path) -> TransferSpec {
    TransferSpec {
        from: from.into(),
        dir: dir.display().to_string(),
        names: names.iter().map(|s| s.to_string()).collect(),
        to: to.into(),
        to_dir: to_dir.display().to_string(),
        op: Op::Copy,
        policies: HashMap::new(),
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn age(path: &Path, secs: u64) {
    let t = std::time::SystemTime::now() - Duration::from_secs(secs);
    std::fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
}

fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.contains(".sparkbox-")).collect()
}

#[tokio::test]
async fn reading_and_saving_on_a_node_is_atomic_and_conflict_aware() {
    let (files, fakes, homes) = setup("save", 1).await;
    let home = &homes[0];
    write(&home.join("a.conf"), "x = 1\n");
    std::fs::set_permissions(home.join("a.conf"), std::fs::Permissions::from_mode(0o640)).unwrap();
    let path = home.join("a.conf").display().to_string();

    let opened = files.read(false, "cell1", &path).await.unwrap();
    assert_eq!(opened.content, "x = 1\n");
    assert_eq!(opened.mode, "-rw-r-----");
    assert!(opened.writable && !opened.large);

    let saved = files.write(false, "cell1", &path, "x = 2 # ünïcode\n", false, Some(&opened.sha256)).await.unwrap();
    assert_eq!(std::fs::read_to_string(home.join("a.conf")).unwrap(), "x = 2 # ünïcode\n");
    assert_eq!(std::fs::metadata(home.join("a.conf")).unwrap().permissions().mode() & 0o777, 0o640, "mode kept");
    let backup = saved.backup.expect("previous version kept");
    assert!(backup.starts_with(&home.join(".sparkbox/backups").display().to_string()), "{backup}");
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), "x = 1\n");
    let versions = files.backups(false, "cell1", &path).await.unwrap();
    assert_eq!(versions.len(), 1);
    assert!(leftovers(home).is_empty(), "no temporary files: {:?}", leftovers(home));

    // Someone else changes it: the stale save is refused and nothing is touched.
    write(&home.join("a.conf"), "x = 9 # edited on the node\n");
    match files.write(false, "cell1", &path, "x = 3\n", false, Some(&opened.sha256)).await {
        Err(SaveError::ChangedOnDisk { sha256, .. }) => assert_eq!(sha256.as_deref(), Some(sha_hex(b"x = 9 # edited on the node\n").as_str())),
        other => panic!("{other:?}"),
    }
    assert_eq!(std::fs::read_to_string(home.join("a.conf")).unwrap(), "x = 9 # edited on the node\n");
    // Overwrite (no expected checksum) goes through.
    files.write(false, "cell1", &path, "x = 3\n", false, None).await.unwrap();

    // Owner and mode survive a sudo save; only the password goes to sudo's stdin.
    std::fs::set_permissions(home.join("a.conf"), std::fs::Permissions::from_mode(0o604)).unwrap();
    let before = std::fs::metadata(home.join("a.conf")).unwrap();
    let cur = files.read(false, "cell1", &path).await.unwrap();
    files.write(false, "cell1", &path, "x = 4\n", true, Some(&cur.sha256)).await.unwrap();
    let after = std::fs::metadata(home.join("a.conf")).unwrap();
    assert_eq!(std::fs::read_to_string(home.join("a.conf")).unwrap(), "x = 4\n");
    assert_eq!((after.uid(), after.gid(), after.permissions().mode() & 0o7777), (before.uid(), before.gid(), 0o604));
    assert_ne!(after.ino(), before.ino(), "replaced by rename, not rewritten in place");
    let stdins = fakes[0].stdin.lock().unwrap().clone();
    let (cmd, password) = stdins.iter().rev().find(|(c, _)| c.starts_with("sudo") && c.contains("commit")).unwrap();
    assert!(cmd.contains("exec python3 -I ") && cmd.contains("sparkjob.py' commit") && cmd.contains("--from"), "root runs its own verified copy: {cmd}");
    assert!(!cmd.contains(".sparkbox/sparkjob.py"), "never the one in the user's home");
    assert_eq!(password, b"pw\n", "only the password goes to sudo's stdin");
    assert!(std::fs::read_dir(home.join(".sparkbox/tmp")).unwrap().next().is_none(), "staged copy removed");
    assert_eq!(files.backups(false, "cell1", &path).await.unwrap().len(), 3);

    let bin = home.join("b.bin");
    std::fs::write(&bin, [0u8, 1, 2]).unwrap();
    assert!(files.read(false, "cell1", &bin.display().to_string()).await.unwrap_err().contains("binary"));
    let big = home.join("big.log");
    std::fs::write(&big, format!("first\n{}\nlast\n", "y".repeat(3 * 1024 * 1024))).unwrap();
    let peek = files.read(false, "cell1", &big.display().to_string()).await.unwrap();
    assert!(peek.large && peek.head.unwrap().starts_with("first") && peek.tail.unwrap().ends_with("last\n"));
}

#[tokio::test]
async fn a_save_cut_off_mid_way_leaves_the_original_intact() {
    // cell1's fake cuts the text short (the connection dropped while it was being sent), or kills
    // sparkjob just before the rename (the session died at the worst moment). A script decides
    // when the command starts, so the saves name what should happen to them in their checksums.
    let cut_text = format!("TRUNCATE {}", "z".repeat(4000));
    let cut_sha = sha_hex(cut_text.as_bytes());
    let die_sha = sha_hex(b"DIE\n");
    let (files, _fakes, homes) = setup_with("cut", 1, |home| {
        let (cut_text, cut_sha, die_sha) = (cut_text.clone(), cut_sha.clone(), die_sha.clone());
        Arc::new(move |cmd: &str, _: &[u8]| {
            if !cmd.contains("sparkjob.py commit") {
                return None;
            }
            if cmd.contains(&cut_sha) {
                return Some(run_real(cmd, &cut_text.as_bytes()[..cut_text.len() / 2], &home, &[]));
            }
            if cmd.contains(&die_sha) {
                return Some(run_real(cmd, b"DIE\n", &home, &[("SPARKJOB_FAULT", "before-replace")]));
            }
            None
        })
    })
    .await;
    let home = &homes[0];
    let original = "network:\n  version: 2\n".repeat(50);
    write(&home.join("40-cx7.yaml"), &original);
    let path = home.join("40-cx7.yaml").display().to_string();
    let sha = files.read(false, "cell1", &path).await.unwrap().sha256;

    let err = files.write(false, "cell1", &path, &cut_text, false, Some(&sha)).await.unwrap_err();
    assert!(err.to_string().contains("incomplete"), "{err}");
    assert_eq!(std::fs::read_to_string(home.join("40-cx7.yaml")).unwrap(), original);
    assert!(leftovers(home).is_empty());

    let err = files.write(false, "cell1", &path, "DIE\n", false, Some(&sha)).await.unwrap_err();
    assert!(matches!(err, SaveError::Failed { .. }), "{err:?}");
    assert_eq!(std::fs::read_to_string(home.join("40-cx7.yaml")).unwrap(), original, "the original is untouched");

    // The next save works (and its checksum still matches: nothing changed).
    files.write(false, "cell1", &path, "network: {}\n", false, Some(&sha)).await.unwrap();
    assert_eq!(std::fs::read_to_string(home.join("40-cx7.yaml")).unwrap(), "network: {}\n");
}

#[tokio::test]
async fn the_mac_saves_atomically_with_backups_and_conflicts() {
    let dir = temp("files-mac-save");
    let files = Files::new(Live::new(crate::vpn::Vpn::new(dir.join("v.json"), dir.join("no-helper"), |_| {}), dir.join("kh.json"), || Ok(None)), dir.join("backups"));
    let p = dir.join("run.sh");
    write(&p, "echo 1\n");
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o750)).unwrap();
    // Saving through a symlink writes the real file and keeps the link.
    std::os::unix::fs::symlink(&p, dir.join("link.sh")).unwrap();
    let link = dir.join("link.sh").display().to_string();
    let opened = files.read(false, MAC, &link).await.unwrap();
    files.write(false, MAC, &link, "echo 2\n", false, Some(&opened.sha256)).await.unwrap();
    assert!(std::fs::symlink_metadata(dir.join("link.sh")).unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "echo 2\n");
    assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o750);
    assert!(matches!(files.write(false, MAC, &link, "echo 3\n", false, Some(&opened.sha256)).await, Err(SaveError::ChangedOnDisk { .. })));
    let versions = files.backups(false, MAC, &p.display().to_string()).await.unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(std::fs::read_to_string(&versions[0].path).unwrap(), "echo 1\n");
    assert!(leftovers(&dir).is_empty());
}

#[test]
fn protected_paths_are_normalised_before_the_check() {
    use super::paths::{clean, is_protected, node_removable};
    assert_eq!(clean("//etc"), "/etc");
    assert_eq!(clean("/etc/."), "/etc");
    assert_eq!(clean("/usr/../etc"), "/etc");
    assert_eq!(clean("/home/u/models/"), "/home/u/models");
    for p in ["/", "//etc", "/etc/.", "/usr/../etc", "/usr/bin/python3", "/var/lib", "/opt/nvidia", "/home", "/home/u", "/home/u/", "/Users", "/System/Library/Fonts"] {
        assert!(is_protected(&clean(p), "/home/u"), "{p}");
        assert!(node_removable(p, Some("/home/u")).is_err(), "{p}");
    }
    for p in ["/home/u/notes.txt", "/usr/local/bin/tool", "/etc/netplan/50-old.yaml", "/home/u/.cache/huggingface/hub"] {
        assert!(!is_protected(p, "/home/u"), "{p}");
    }
    assert_eq!(node_removable("~/models/x", Some("/home/u")).unwrap(), "/home/u/models/x");
    assert!(node_removable("relative/path", Some("/home/u")).is_err());
}

/// The real Trash, through NSFileManager. It can take minutes when the temp folder is synced,
/// so it's run on demand: `cargo test -- --ignored the_real_mac_trash`.
#[tokio::test]
#[ignore]
async fn the_real_mac_trash_takes_an_item_and_undo_puts_it_back() {
    let dir = temp("files-mac-real-trash");
    let files = Files::new(Live::new(crate::vpn::Vpn::new(dir.join("v.json"), dir.join("no-helper"), |_| {}), dir.join("kh.json"), || Ok(None)), dir.join("backups"));
    let p = dir.join(format!("sparkbox-trash-test-{}.txt", std::process::id()));
    write(&p, "keep me");
    let trashed = files.trash(false, MAC, &[p.display().to_string()]).await.unwrap();
    let went = trashed[0].trashed.clone().expect("NSFileManager says where it went");
    assert!(went.contains("/.Trash") && Path::new(&went).exists(), "{went}");
    files.untrash(false, MAC, &trashed).await.unwrap();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "keep me");
}

#[tokio::test]
async fn on_the_mac_delete_moves_to_the_trash_and_undo_puts_it_back() {
    let dir = temp("files-mac-trash");
    let files = Files::new(Live::new(crate::vpn::Vpn::new(dir.join("v.json"), dir.join("no-helper"), |_| {}), dir.join("kh.json"), || Ok(None)), dir.join("backups"));
    files.set_mac_trash(Arc::new(mac::FolderTrash(dir.join(".Trash"))));
    let name = format!("sparkbox-trash-test-{}.txt", std::process::id());
    let p = dir.join(&name);
    write(&p, "keep me");
    let trashed = files.trash(false, MAC, &[p.display().to_string()]).await.unwrap();
    assert!(!p.exists(), "moved away");
    let went = trashed[0].trashed.clone().expect("the Trash says where it went");
    assert!(went.contains("/.Trash"), "{went}");
    assert!(Path::new(&went).exists());
    files.untrash(false, MAC, &trashed).await.unwrap();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "keep me", "Undo restored it");
    assert!(!Path::new(&went).exists());

    let home = mac::home().display().to_string();
    for bad in ["/", "//etc", "/etc/.", "/usr/../etc", "/Users", "/System/Library", home.as_str(), "relative.txt"] {
        let err = files.trash(false, MAC, &[bad.to_string()]).await.unwrap_err();
        assert!(err.contains("won't delete") || err.contains("absolute"), "{bad}: {err}");
        assert!(files.delete(false, MAC, &[bad.to_string()]).await.is_err(), "{bad}");
    }
    files.delete(false, MAC, &[p.display().to_string()]).await.unwrap();
    assert!(!p.exists());
}

#[tokio::test]
async fn on_a_node_delete_uses_the_freedesktop_trash_and_undo_puts_it_back() {
    let (files, _fakes, homes) = setup("trash", 1).await;
    let home = &homes[0];
    write(&home.join("work/a b.txt"), "one");
    write(&home.join("work/sub/x.txt"), "two");
    let paths = vec![home.join("work/a b.txt").display().to_string(), home.join("work/sub").display().to_string()];
    let trashed = files.trash(false, "cell1", &paths).await.unwrap();
    assert_eq!(trashed.len(), 2);
    assert!(!home.join("work/a b.txt").exists() && !home.join("work/sub").exists());
    let files_dir = home.join(".local/share/Trash/files");
    assert!(files_dir.join("a b.txt").exists() && files_dir.join("sub/x.txt").exists());
    let info = std::fs::read_to_string(home.join(".local/share/Trash/info/a b.txt.trashinfo")).unwrap();
    assert!(info.starts_with("[Trash Info]\nPath=") && info.contains("a%20b.txt") && info.contains("DeletionDate="), "{info}");

    files.untrash(false, "cell1", &trashed).await.unwrap();
    assert_eq!(std::fs::read_to_string(home.join("work/a b.txt")).unwrap(), "one");
    assert_eq!(std::fs::read_to_string(home.join("work/sub/x.txt")).unwrap(), "two");
    assert!(!home.join(".local/share/Trash/info/a b.txt.trashinfo").exists());

    let home_s = home.display().to_string();
    for bad in ["/", "//etc", "/etc/.", "/usr/../etc", "/var/lib", home_s.as_str(), "relative"] {
        let err = files.trash(false, "cell1", &[bad.to_string()]).await.unwrap_err();
        assert!(err.contains("won't delete") || err.contains("absolute"), "{bad}: {err}");
        assert!(files.delete(false, "cell1", &[bad.to_string()]).await.is_err(), "{bad}");
    }
    // The node checks again after realpath: a symlink that leads to a system folder is the link.
    std::os::unix::fs::symlink("/etc", home.join("etc-link")).unwrap();
    files.delete(false, "cell1", &[home.join("etc-link").display().to_string()]).await.unwrap();
    assert!(Path::new("/etc").exists() && !home.join("etc-link").exists());
}

#[tokio::test]
async fn folders_rename_and_permissions_on_a_node_and_the_mac() {
    let (files, _fakes, homes) = setup("ops", 1).await;
    let mac = temp("files-ops-mac");
    for (place, root) in [("cell1", homes[0].clone()), (MAC, mac.clone())] {
        let dir = root.display().to_string();
        files.mkdir(false, place, &dir, "new").await.unwrap();
        assert!(root.join("new").is_dir(), "{place}");
        assert!(files.mkdir(false, place, &dir, "../escape").await.is_err());
        std::fs::write(root.join("new/f.txt"), "hi").unwrap();

        files.rename(false, place, &format!("{dir}/new"), "renamed").await.unwrap();
        assert!(root.join("renamed/f.txt").exists(), "{place}");
        std::fs::create_dir(root.join("taken")).unwrap();
        assert!(files.rename(false, place, &format!("{dir}/renamed"), "taken").await.unwrap_err().contains("already exists"));

        files.chmod(false, place, &format!("{dir}/renamed/f.txt"), 0o600).await.unwrap();
        assert_eq!(std::fs::metadata(root.join("renamed/f.txt")).unwrap().permissions().mode() & 0o777, 0o600, "{place}");
        let usage = files.du(false, place, &[format!("{dir}/renamed")]).await.unwrap();
        assert_eq!(usage[0].bytes, 2, "{place}");

        files.delete(false, place, &[format!("{dir}/renamed"), format!("{dir}/taken")]).await.unwrap();
        assert!(!root.join("renamed").exists() && !root.join("taken").exists(), "{place}");
        let listing = files.list(false, place, &dir).await.unwrap();
        assert!(listing.free.unwrap_or(0) > 0 && listing.capacity.unwrap_or(0) > 0, "{place}: free space shown");
    }
}

/// Files › New file (T2's "Create a file"): an empty file, made once. The second time, and over
/// a link that leads nowhere, it's "There's already something called … there." and nothing
/// changes. On a node it's sparkjob's commit with `--expect absent` and empty stdin (C8), as a
/// titled row; a Demo node says it needs Live data.
#[tokio::test]
async fn new_file_makes_an_empty_file_once_and_never_over_anything() {
    let (files, fakes, homes) = setup("newfile", 1).await;
    let mac = temp("files-newfile-mac");
    for (place, root) in [("cell1", homes[0].clone()), (MAC, mac.clone())] {
        let dir = root.display().to_string();
        assert_eq!(files.new_file(false, place, &dir, "notes.txt").await.unwrap(), format!("{dir}/notes.txt"), "{place}");
        assert_eq!(std::fs::read(root.join("notes.txt")).unwrap(), b"", "{place}");
        std::fs::write(root.join("notes.txt"), "kept").unwrap();
        assert_eq!(files.new_file(false, place, &dir, "notes.txt").await.unwrap_err(), "There's already something called notes.txt there.", "{place}");
        assert_eq!(std::fs::read_to_string(root.join("notes.txt")).unwrap(), "kept", "{place}: left as it was");
        std::os::unix::fs::symlink(root.join("nowhere"), root.join("link")).unwrap();
        assert_eq!(files.new_file(false, place, &dir, "link").await.unwrap_err(), "There's already something called link there.", "{place}");
        assert!(!root.join("nowhere").exists(), "{place}: nothing made through the link");
        assert!(files.new_file(false, place, &dir, "../escape").await.is_err(), "{place}");
    }
    let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    let sent = fakes[0].ran("sparkjob.py commit");
    let made = format!("python3 ~/.sparkbox/sparkjob.py commit '{}/notes.txt' --sha256 {empty} --expect absent", homes[0].display());
    assert_eq!(sent.iter().filter(|c| c.ends_with(&made)).count(), 2, "{sent:?}");
    let stdin: Vec<Vec<u8>> = fakes[0].stdin.lock().unwrap().iter().filter(|(c, _)| c.ends_with(&made)).map(|(_, i)| i.clone()).collect();
    assert!(stdin.len() == 2 && stdin.iter().all(|i| i.is_empty()), "empty stdin: {stdin:?}");
    let rows = files.live.audit.entries();
    let row = rows.iter().find(|e| e.command.ends_with(&made)).expect("its row");
    assert_eq!((row.title.as_str(), row.node.as_str()), ("New file notes.txt on cell1", "cell1"));
    assert!(rows.iter().any(|e| e.title == format!("New file {}/notes.txt", mac.display())), "the Mac's in the activity log too");
    assert_eq!(files.new_file(true, "cell1", "~", "x.txt").await.unwrap_err(), "Available with Live data");
}

/// New file into a folder that isn't there: sparkjob.py says which folder, with no traceback, and
/// the Files window's error is the plain sentence. Nothing is made.
#[tokio::test]
async fn new_file_into_a_missing_folder_says_so_plainly() {
    let (files, fakes, homes) = setup("newfile-missing", 1).await;
    std::fs::create_dir_all(homes[0].join("sparkbox-test")).unwrap();
    for dir in ["~/sparkbox-test/x".to_string(), homes[0].join("sparkbox-test/x/y").display().to_string()] {
        let e = files.new_file(false, "cell1", &dir, "notes.txt").await.unwrap_err();
        assert_eq!(e, format!("cell1: {{\"error\": \"The folder {dir} doesn't exist.\"}}"));
        assert_eq!(super::commands::new_file_error(e, "notes.txt"), format!("Can't create notes.txt: the folder {dir} doesn't exist."));
    }
    assert!(std::fs::read_dir(homes[0].join("sparkbox-test")).unwrap().next().is_none(), "nothing made");
    assert_eq!(fakes[0].ran("sparkjob.py commit").len(), 2);
    // Any other error stays as it was.
    for other in ["There's already something called notes.txt there.", "cell1: sparkjob was killed (SIGKILL)", "cell1: {\"error\": \"something else\"}"] {
        assert_eq!(super::commands::new_file_error(other.to_string(), "notes.txt"), other);
    }
}

/// Under the T2 preset (cell2: test containers, process signals, files) New file is admitted as
/// files-test in ~/sparkbox-test and refused anywhere else; without files-test it's refused there
/// too, and look-only refuses it before any fence is asked. Nothing refused reaches the node.
#[tokio::test]
async fn new_file_under_the_t2_preset_is_files_test_in_sparkbox_test_only() {
    use crate::live::fence_class::FenceClass::{ContainerTest, FilesTest, ProcessTest};
    let (files, fakes, homes) = setup("newfile-t2", 2).await;
    let (node, home) = (&fakes[1], &homes[1]);
    std::fs::create_dir_all(home.join("sparkbox-test")).unwrap();
    let test_dir = home.join("sparkbox-test").display().to_string();
    let fences = temp("files-newfile-t2-fence");
    let message = |e: String| crate::errors::SbError::classify(&e).message;
    let commits = || node.ran("sparkjob.py commit").len();

    crate::live::tests::open_fence(&files.live, &fences, &["cell2"], &[ContainerTest, ProcessTest, FilesTest]);
    files.new_file(false, "cell2", &test_dir, "made.txt").await.unwrap();
    assert!(home.join("sparkbox-test/made.txt").is_file());
    let row = files.live.audit.entries().into_iter().find(|e| e.node == "cell2" && e.command.contains("made.txt' --sha256")).expect("its row");
    assert_eq!((row.fence.as_deref(), row.title.as_str()), (Some("files-test"), "New file made.txt on cell2"));
    let before = commits();
    let e = files.new_file(false, "cell2", &home.display().to_string(), "outside.txt").await.unwrap_err();
    assert_eq!(message(e), "The fence doesn't allow this change.");
    assert!(!home.join("outside.txt").exists());

    crate::live::tests::open_fence(&files.live, &fences, &["cell2"], &[ContainerTest, ProcessTest]);
    assert_eq!(message(files.new_file(false, "cell2", &test_dir, "other.txt").await.unwrap_err()), "The fence doesn't allow file changes.");
    crate::live::tests::open_fence(&files.live, &fences, &["cell2"], &[ContainerTest, ProcessTest, FilesTest]);
    files.live.set_write_policy(crate::live::tests::TestPolicy::look_only());
    assert!(message(files.new_file(false, "cell2", &test_dir, "other.txt").await.unwrap_err()).contains("look-only mode"));
    assert_eq!(commits(), before, "nothing refused reached the node");
    assert!(!home.join("sparkbox-test/other.txt").exists());
    // Each refusal is a row of its own titled operation.
    let refused: Vec<_> = files.live.audit.entries().into_iter().filter(|e| e.title == "Refused" || e.state == "failed").filter(|e| e.node == "cell2").collect();
    assert_eq!(refused.len(), 3, "{refused:?}");
    assert!(refused.iter().all(|e| e.op_id.as_deref().is_some_and(|o| o.starts_with("files-"))), "{refused:?}");
}

#[tokio::test]
async fn transfers_stream_batches_between_the_mac_and_nodes() {
    let (files, _fakes, homes) = setup("copy", 2).await;
    let mac = temp("files-copy-mac");
    write(&mac.join("project/sub/data.bin"), &"7".repeat(300_000));
    write(&mac.join("project/run.sh"), "#!/bin/sh\necho hi\n");
    write(&mac.join("readme.txt"), "read me\n");
    std::fs::set_permissions(mac.join("project/run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();

    // Mac -> cell1, two items in one tar, into a folder that doesn't exist yet.
    let dest = homes[0].join("incoming");
    let pre = files.preflight(false, &spec(MAC, &mac, &["project", "readme.txt"], "cell1", &dest)).await.unwrap();
    assert_eq!(pre.bytes, 300_000 + 18 + 8);
    assert!(pre.conflicts.is_empty() && pre.route == "direct");
    let (emit, seen) = collector();
    files.transfer(false, spec(MAC, &mac, &["project", "readme.txt"], "cell1", &dest), emit).await.unwrap();
    match finished(&seen).await {
        TransferEvent::Done { files: n, placed, .. } => {
            assert_eq!(n, 5, "project, sub, data.bin, run.sh and readme.txt, counted from tar -v");
            assert!(placed.iter().all(|p| p.action == "copied"), "{placed:?}");
        }
        e => panic!("{e:?}"),
    }
    let landed = dest.join("project");
    assert_eq!(std::fs::read(landed.join("sub/data.bin")).unwrap().len(), 300_000);
    assert_eq!(std::fs::metadata(landed.join("run.sh")).unwrap().permissions().mode() & 0o777, 0o755, "permissions survive");
    assert!(!landed.join("._run.sh").exists(), "no macOS metadata files");
    assert!(leftovers(&dest).is_empty(), "staging folder removed");
    let current = seen.lock().unwrap().iter().filter_map(|e| match e {
        TransferEvent::Progress { current, .. } => current.clone(),
        _ => None,
    }).next_back();
    assert!(current.is_some(), "the file in flight is named");

    // cell1 -> cell2 (through this Mac), then cell2 -> this Mac.
    let (emit, seen) = collector();
    let pre = files.preflight(false, &spec("cell1", &dest, &["project"], "cell2", &homes[1])).await.unwrap();
    assert_eq!(pre.route, "viaMac");
    files.transfer(false, spec("cell1", &dest, &["project"], "cell2", &homes[1]), emit).await.unwrap();
    assert!(matches!(finished(&seen).await, TransferEvent::Done { .. }));
    assert_eq!(std::fs::read_to_string(homes[1].join("project/run.sh")).unwrap(), "#!/bin/sh\necho hi\n");
    let back = temp("files-copy-back");
    let (emit, seen) = collector();
    files.transfer(false, spec("cell2", &homes[1].join("project"), &["run.sh"], MAC, &back), emit).await.unwrap();
    assert!(matches!(finished(&seen).await, TransferEvent::Done { .. }));
    assert_eq!(std::fs::read_to_string(back.join("run.sh")).unwrap(), "#!/bin/sh\necho hi\n");

    // Within one node: copy on the node, and move by renaming.
    let (emit, seen) = collector();
    files.transfer(false, spec("cell1", &dest, &["project"], "cell1", &homes[0]), emit).await.unwrap();
    assert!(matches!(finished(&seen).await, TransferEvent::Done { .. }), "{:?}", seen.lock().unwrap());
    assert!(homes[0].join("project/sub/data.bin").exists() && dest.join("project").exists());
    let (emit, seen) = collector();
    files.transfer(false, TransferSpec { op: Op::Move, ..spec("cell1", &dest, &["readme.txt"], "cell1", &homes[0]) }, emit).await.unwrap();
    assert!(matches!(finished(&seen).await, TransferEvent::Done { .. }));
    assert!(homes[0].join("readme.txt").exists() && !dest.join("readme.txt").exists());

    // A move between places removes the originals only after the copy landed.
    let (emit, seen) = collector();
    files.transfer(false, TransferSpec { op: Op::Move, ..spec(MAC, &mac, &["readme.txt"], "cell2", &homes[1]) }, emit).await.unwrap();
    assert!(matches!(finished(&seen).await, TransferEvent::Done { .. }));
    assert!(homes[1].join("readme.txt").exists() && !mac.join("readme.txt").exists());

    // A source that vanished reports the failure instead of hanging.
    let (emit, seen) = collector();
    files.transfer(false, spec("cell2", Path::new("/no/such"), &["thing"], MAC, &back), emit).await.unwrap();
    match finished(&seen).await {
        TransferEvent::Failed { message } => assert!(!message.is_empty()),
        e => panic!("{e:?}"),
    }
}

#[tokio::test]
async fn each_conflict_policy_does_what_it_says() {
    let (files, _fakes, homes) = setup("policies", 1).await;
    let (mac, node) = (temp("files-policies-mac"), homes[0].join("dest"));
    // Destination as it was.
    write(&node.join("cfg/a.txt"), "old a");
    write(&node.join("cfg/only-old.txt"), "old only");
    write(&node.join("merge/a.txt"), "old a");
    write(&node.join("merge/keep.txt"), "keep");
    write(&node.join("notes.txt"), "old notes");
    write(&node.join("skip.txt"), "untouched");
    write(&node.join("newer/stale.txt"), "old");
    age(&node.join("newer/stale.txt"), 3600);
    write(&node.join("newer/fresh.txt"), "dest is newer");
    // What's coming.
    for (rel, text) in [("cfg/a.txt", "new a"), ("merge/a.txt", "new a"), ("merge/b.txt", "new b"), ("notes.txt", "new notes"), ("skip.txt", "no"), ("newer/stale.txt", "newer copy"), ("newer/fresh.txt", "older copy")] {
        write(&mac.join(rel), text);
    }
    age(&mac.join("newer/fresh.txt"), 7200);
    let names = ["cfg", "merge", "notes.txt", "skip.txt", "newer"];

    let pre = files.preflight(false, &spec(MAC, &mac, &names, "cell1", &node)).await.unwrap();
    let by: HashMap<&str, _> = pre.conflicts.iter().map(|c| (c.name.as_str(), c)).collect();
    assert_eq!(by.len(), 5, "every name is taken");
    assert_eq!((by["notes.txt"].source.bytes, by["notes.txt"].dest.bytes), (9, 9));
    assert!(by["cfg"].dest.entries == 3 && by["cfg"].dest.kind == "dir");
    assert!(by["notes.txt"].source.mtime > 0 && by["notes.txt"].dest.mtime > 0);

    let mut s = spec(MAC, &mac, &names, "cell1", &node);
    s.policies = HashMap::from([
        ("cfg".to_string(), Policy::Replace),
        ("merge".to_string(), Policy::Merge),
        ("notes.txt".to_string(), Policy::KeepBoth),
        ("skip.txt".to_string(), Policy::Skip),
        ("newer".to_string(), Policy::ReplaceIfNewer),
    ]);
    let (emit, seen) = collector();
    files.transfer(false, s, emit).await.unwrap();
    let placed = match finished(&seen).await {
        TransferEvent::Done { placed, .. } => placed,
        e => panic!("{e:?}"),
    };
    let read = |rel: &str| std::fs::read_to_string(node.join(rel)).unwrap();
    assert_eq!(read("cfg/a.txt"), "new a");
    assert!(!node.join("cfg/only-old.txt").exists(), "replace swaps the whole folder");
    assert_eq!((read("merge/a.txt"), read("merge/b.txt"), read("merge/keep.txt")), ("new a".into(), "new b".into(), "keep".into()));
    assert_eq!((read("notes.txt"), read("notes 2.txt")), ("old notes".into(), "new notes".into()));
    assert_eq!(placed.iter().find(|p| p.name == "notes.txt").unwrap().as_name, "notes 2.txt");
    assert_eq!(read("skip.txt"), "untouched");
    assert_eq!((read("newer/stale.txt"), read("newer/fresh.txt")), ("newer copy".into(), "dest is newer".into()));
    assert!(leftovers(&node).is_empty(), "{:?}", leftovers(&node));

    // The same policies into this Mac (node -> Mac).
    let back = temp("files-policies-back");
    write(&back.join("cfg/stale.txt"), "x");
    write(&back.join("notes.txt"), "mac notes");
    let mut s = spec("cell1", &node, &["cfg", "notes.txt"], MAC, &back);
    s.policies = HashMap::from([("cfg".to_string(), Policy::Replace), ("notes.txt".to_string(), Policy::KeepBoth)]);
    let (emit, seen) = collector();
    files.transfer(false, s, emit).await.unwrap();
    assert!(matches!(finished(&seen).await, TransferEvent::Done { .. }));
    assert!(!back.join("cfg/stale.txt").exists() && back.join("cfg/a.txt").exists());
    assert_eq!(std::fs::read_to_string(back.join("notes 2.txt")).unwrap(), "old notes");
}

#[tokio::test]
async fn stopping_a_transfer_leaves_nothing_behind() {
    let (files, _fakes, homes) = setup("cancel", 1).await;
    let mac = temp("files-cancel-mac");
    // Real data (a sparse file would make a tiny tar stream).
    let big = mac.join("big.img");
    let block: Vec<u8> = (0..1024 * 1024).map(|i| (i % 251) as u8).collect();
    let mut f = std::fs::File::create(&big).unwrap();
    for _ in 0..96 {
        std::io::Write::write_all(&mut f, &block).unwrap();
    }
    drop(f);
    let (emit, seen) = collector();
    let id = files.transfer(false, spec(MAC, &mac, &["big.img"], "cell1", &homes[0]), emit).await.unwrap();
    for _ in 0..500 {
        if seen.lock().unwrap().iter().any(|e| matches!(e, TransferEvent::Progress { done, .. } if *done > 0)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    files.cancel(&id);
    assert!(matches!(finished(&seen).await, TransferEvent::Cancelled), "{:?}", seen.lock().unwrap().last());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!homes[0].join("big.img").exists(), "no partial file");
    assert!(leftovers(&homes[0]).is_empty(), "staging folder removed: {:?}", leftovers(&homes[0]));
}

#[tokio::test]
async fn demo_nodes_follow_the_demo_world() {
    let dir = temp("files-demo");
    let files = Files::new(Live::new(crate::vpn::Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {}), dir.join("kh.json"), || Ok(None)), dir.join("backups"));
    let home = crate::demo::world::home();

    let listing = files.list(true, "cell3", "~").await.unwrap();
    assert_eq!(listing.path, home);
    assert!(listing.entries.iter().any(|e| e.name == "notes.txt"));
    assert!(listing.free.unwrap() > 1_000_000_000_000);
    let netplan = files.list(true, "cell3", "/etc/netplan").await.unwrap();
    assert!(netplan.entries.iter().all(|e| e.owner.as_deref() == Some("root")));
    // Root-only files can't be read without sudo; the fabric netplan and /etc/hosts follow the IP plan.
    // r4 F5: as one refusal the editor can explain (root, 600), with Read with sudo… behind it.
    let refused = files.read(true, "cell3", "/etc/netplan/50-cloud-init.yaml").await.unwrap_err();
    let v: serde_json::Value = serde_json::from_str(&refused).unwrap();
    assert_eq!(v["kind"], "permission", "{refused}");
    assert!(v["message"].as_str().unwrap().starts_with("Only root can read 50-cloud-init.yaml (root, 600)."), "{refused}");
    let as_root = files.read_sudo(true, "cell3", "/etc/netplan/50-cloud-init.yaml").await.unwrap();
    assert!(!as_root.content.is_empty() && !as_root.writable, "read as root, saved only with sudo");
    let fabric = files.read(true, "cell3", "/etc/netplan/40-cx7-fabric.yaml").await.unwrap().content;
    assert!(fabric.contains("enp1s0f1np1:") && fabric.contains("addresses: [192.168.100.3/24]") && fabric.contains("addresses: [192.168.101.3/24]") && fabric.contains("mtu: 9000"), "{fabric}");
    let hosts = files.read(true, "cell2", "/etc/hosts").await.unwrap();
    assert!(hosts.content.contains("192.168.100.3   cell3-fabric") && hosts.content.contains("192.168.101.4   gx10-0000-fabric2"), "{}", hosts.content);
    assert!(!hosts.writable && hosts.owner == "root:root");
    assert!(matches!(files.write(true, "cell2", "/etc/hosts", "x", false, Some(&hosts.sha256)).await, Err(SaveError::NeedsSudo { .. })));
    files.write(true, "cell2", "/etc/hosts", &format!("{}10.0.0.9 extra\n", hosts.content), true, Some(&hosts.sha256)).await.unwrap();
    assert_eq!(files.backups(true, "cell2", "/etc/hosts").await.unwrap().len(), 1);

    // Model folders as the Models window lists them, with real-sized shards.
    let llama = crate::demo::world::MODEL_FOLDERS[0];
    let folder = files.list(true, "cell1", &llama.path(&home)).await.unwrap();
    let shards: Vec<_> = folder.entries.iter().filter(|e| e.name.ends_with(".safetensors")).collect();
    assert_eq!(shards.len(), llama.files as usize);
    assert_eq!(shards[0].name, format!("model-00001-of-{:05}.safetensors", llama.files));
    assert_eq!(files.du(true, "cell1", &[llama.path(&home)]).await.unwrap()[0].bytes, llama.bytes);
    assert!(files.read(true, "cell1", &format!("{}/tokenizer.json", llama.path(&home))).await.unwrap().large);
    assert!(files.list(true, "cell1", "~/models").await.unwrap().entries.iter().any(|e| e.name == "latest" && e.kind == "link" && e.link_dir));

    // Saves notice changes made meanwhile (e.g. by the demo Terminal).
    let notes = files.read(true, "cell3", "~/notes.txt").await.unwrap();
    files.demo_fs().lock().unwrap().write_text("cell3", "~/notes.txt", "appended\n", true).unwrap();
    assert!(matches!(files.write(true, "cell3", "~/notes.txt", "mine", false, Some(&notes.sha256)).await, Err(SaveError::ChangedOnDisk { .. })));

    // Trash and Undo.
    let t = files.trash(true, "cell3", &[format!("{home}/notes.txt")]).await.unwrap();
    assert!(t[0].trashed.as_deref().unwrap().ends_with(".local/share/Trash/files/notes.txt"));
    assert!(files.read(true, "cell3", "~/notes.txt").await.is_err());
    files.untrash(true, "cell3", &t).await.unwrap();
    assert!(files.read(true, "cell3", "~/notes.txt").await.unwrap().content.contains("appended"));
    assert!(files.trash(true, "cell3", &["/etc/.".into()]).await.is_err());

    // Node -> node, with the policies; then node -> this Mac (real text, sparse weights).
    let (emit, seen) = collector();
    let mut s = spec("cell3", Path::new(&home), &["notes.txt"], "cell1", Path::new(&home));
    s.policies.insert("notes.txt".into(), Policy::KeepBoth);
    files.transfer(true, s, emit).await.unwrap();
    assert!(matches!(finished(&seen).await, TransferEvent::Done { .. }));
    assert!(files.read(true, "cell1", "~/notes 2.txt").await.unwrap().content.contains("appended"));
    // A Demo node's files never land on this Mac (nothing is written there).
    files.set_mac_home(std::env::temp_dir().canonicalize().unwrap());
    let (emit, _) = collector();
    let refused = files.transfer(true, spec("cell1", Path::new(&format!("{home}/projects")), &["bench"], MAC, &dir), emit).await.unwrap_err();
    assert_eq!(refused, DEMO_TO_MAC);
    assert!(!dir.join("bench").exists());
}

// ---- Copies onto this Mac (B6)

/// A Files over no nodes whose "home folder" is a temp folder with a Downloads in it.
fn mac_only(name: &str) -> (Arc<Files>, PathBuf) {
    let dir = temp(&format!("files-{name}"));
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("Downloads")).unwrap();
    let files = Files::new(Live::new(crate::vpn::Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {}), dir.join("kh.json"), || Ok(None)), dir.join("backups"));
    files.set_mac_home(home.clone());
    (files, home)
}

#[tokio::test]
async fn demo_node_to_mac_is_refused() {
    let (files, home) = mac_only("demo-to-mac");
    let demo_home = crate::demo::world::home();
    let to_mac = spec("cell3", Path::new(&demo_home), &["notes.txt"], MAC, &home.join("Downloads"));
    assert_eq!(files.preflight(true, &to_mac).await.unwrap_err(), DEMO_TO_MAC, "said before anything starts");
    let (emit, seen) = collector();
    assert_eq!(files.transfer(true, to_mac, emit).await.unwrap_err(), DEMO_TO_MAC);
    assert!(seen.lock().unwrap().is_empty());
    assert_eq!(std::fs::read_dir(home.join("Downloads")).unwrap().count(), 0, "nothing written on the Mac");

    // This Mac -> a Demo node still works, from names and sizes only: Demo never reads what's
    // in a file on this Mac (r4 F2), so the copy is there but says it has no contents.
    write(&home.join("Documents/plan.md"), "# plan\n");
    let (emit, seen) = collector();
    files.transfer(true, spec(MAC, &home.join("Documents"), &["plan.md"], "cell3", Path::new(&demo_home)), emit).await.unwrap();
    assert!(matches!(finished(&seen).await, TransferEvent::Done { .. }));
    assert_eq!(files.read(true, "cell3", "~/plan.md").await.unwrap_err(), super::DEMO_MAC_COPY);
    let listed = files.list(true, "cell3", "~").await.unwrap();
    assert_eq!(listed.entries.iter().find(|e| e.name == "plan.md").map(|e| e.size), Some(7), "the size came along");
    let log = files.live.audit.entries();
    assert!(log.iter().any(|e| e.simulated && e.node == "cell3" && e.title.starts_with("Copy plan.md from this Mac to cell3")), "{log:?}");
}

#[test]
fn node_to_mac_needs_mac_scope() {
    use super::commands::{gate_place, transfer_changes};
    let home = temp("files-scope-home");
    std::fs::create_dir_all(home.join("Downloads/models")).unwrap();
    std::fs::create_dir_all(home.join("Documents")).unwrap();
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    let node = Path::new("/home/sparkuser");
    // Anywhere on this Mac but Downloads: a change on the Mac, Mac scope.
    let docs = spec("cell1", node, &["run.sh"], MAC, &home.join("Documents"));
    assert!(transfer_changes(&docs, &home));
    assert_eq!(gate_place(&docs, &home), MAC);
    // ~/Downloads and below, new names only: a download, no scope.
    let dl = spec("cell1", node, &["run.sh"], MAC, &home.join("Downloads/models"));
    assert!(!transfer_changes(&dl, &home));
    let mut replacing = dl.clone();
    replacing.policies.insert("run.sh".into(), Policy::Replace);
    assert!(transfer_changes(&replacing, &home), "replacing a file in Downloads changes it");
    assert_eq!(gate_place(&replacing, &home), MAC);
    assert!(transfer_changes(&TransferSpec { op: Op::Move, ..dl.clone() }, &home), "a move takes the node's originals");
    // Mac -> node is node scope, unless it reads ~/.ssh.
    let up = spec(MAC, &home.join("Documents"), &["a.txt"], "cell1", node);
    assert_eq!(gate_place(&up, &home), "cell1");
    assert_eq!(gate_place(&spec(MAC, &home.join(".ssh"), &["id_ed25519"], "cell1", node), &home), MAC);
    assert_eq!(gate_place(&TransferSpec { op: Op::Move, ..up }, &home), MAC);
}

#[tokio::test]
async fn downloads_copy_is_quarantined_without_exec() {
    let (files, _fakes, homes) = setup("quarantine", 1).await;
    let mac_home = temp("files-quarantine-mac");
    std::fs::create_dir_all(mac_home.join("Downloads")).unwrap();
    files.set_mac_home(mac_home.clone());
    write(&homes[0].join("tools/run.sh"), "#!/bin/sh\necho hi\n");
    write(&homes[0].join("tools/lib/helper"), "x\n");
    std::fs::set_permissions(homes[0].join("tools/run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(homes[0].join("tools/lib/helper"), std::fs::Permissions::from_mode(0o4755)).unwrap();

    let (emit, seen) = collector();
    files.transfer(false, spec("cell1", &homes[0], &["tools"], MAC, &mac_home.join("Downloads")), emit).await.unwrap();
    assert!(matches!(finished(&seen).await, TransferEvent::Done { .. }), "{:?}", seen.lock().unwrap().last());
    let landed = mac_home.join("Downloads/tools");
    for f in ["run.sh", "lib/helper"] {
        let mode = std::fs::metadata(landed.join(f)).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode & 0o7111, 0, "{f}: no execute, setuid or setgid bits left ({mode:o})");
        assert!(mac::quarantine_of(&landed.join(f)).is_some_and(|q| q.starts_with("0083;") && q.ends_with(";SparkBox;")), "{f} is quarantined");
    }
    assert!(mac::quarantine_of(&landed).is_some(), "the folder too");
    assert!(std::fs::metadata(landed.join("lib")).unwrap().permissions().mode() & 0o100 != 0, "folders stay openable");

    // The same copy outside Downloads keeps its modes and isn't quarantined.
    let docs = mac_home.join("Documents");
    let (emit, seen) = collector();
    files.transfer(false, spec("cell1", &homes[0].join("tools"), &["run.sh"], MAC, &docs), emit).await.unwrap();
    assert!(matches!(finished(&seen).await, TransferEvent::Done { .. }));
    assert_eq!(std::fs::metadata(docs.join("run.sh")).unwrap().permissions().mode() & 0o777, 0o755);
    assert!(mac::quarantine_of(&docs.join("run.sh")).is_none());
}

#[tokio::test]
async fn launch_agents_refused() {
    let (files, home) = mac_only("persist");
    std::fs::create_dir_all(home.join("Library/LaunchAgents")).unwrap();
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::os::unix::fs::symlink(home.join("Library/LaunchAgents"), home.join("Downloads/agents")).unwrap();
    let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let check = |dir: &str, n: &[&str]| mac::landing(&home, dir, &names(n));
    let h = home.display().to_string();
    for (dir, n) in [
        (format!("{h}/Library/LaunchAgents"), vec!["com.x.plist"]),
        (format!("{h}/Library/LaunchDaemons"), vec!["com.x.plist"]),
        (format!("{h}/.ssh"), vec!["authorized_keys"]),
        (h.clone(), vec![".zshrc"]),
        (h.clone(), vec![".bash_profile"]),
        (h.clone(), vec![".ssh"]),
        (h.clone(), vec!["Library"]),
        (format!("{h}/Library"), vec!["LaunchAgents"]),
        (format!("{h}/Downloads/agents"), vec!["com.x.plist"]),
    ] {
        assert_eq!(check(&dir, &n).unwrap_err(), mac::PERSISTENCE, "{dir} {n:?}");
    }
    for dir in ["/Library/LaunchDaemons", "/etc", "relative/folder", "/tmp"] {
        assert_eq!(check(dir, &["x"]).unwrap_err(), mac::OUTSIDE, "{dir}");
    }
    assert_eq!(check(&format!("{h}/Downloads/../Library/LaunchAgents"), &["x"]).unwrap_err(), mac::OUTSIDE);
    assert_eq!(check(&format!("{h}/Downloads"), &["x.plist"]).unwrap(), mac::Landing::Downloads);
    assert_eq!(check("~/Documents", &[".zshrc"]).unwrap(), mac::Landing::Elsewhere, "a .zshrc anywhere but ~ is only a file");
    assert_eq!(check("/Volumes/Backup/models", &["x"]).unwrap(), mac::Landing::Elsewhere);
    // And the transfer itself refuses before anything moves, with Mac scope or not.
    let (emit, _) = collector();
    let err = files.transfer(false, spec("cell1", Path::new("/home/sparkuser"), &["evil.plist"], MAC, &home.join("Library/LaunchAgents")), emit).await.unwrap_err();
    assert_eq!(err, mac::PERSISTENCE);
}

#[tokio::test]
async fn mac_placement_is_audited_with_sha() {
    let (files, _fakes, homes) = setup("audit-sha", 1).await;
    let mac_dir = temp("files-audit-sha-mac");
    write(&homes[0].join("weights.bin"), "abc");
    write(&homes[0].join("cfg/a.json"), "{}");
    let (emit, seen) = collector();
    files.transfer(false, spec("cell1", &homes[0], &["weights.bin", "cfg"], MAC, &mac_dir), emit).await.unwrap();
    assert!(matches!(finished(&seen).await, TransferEvent::Done { .. }));
    // The hashes are written just after Done, off the async workers.
    let mut log = Vec::new();
    for _ in 0..100 {
        log = files.live.audit.entries().into_iter().filter(|e| e.node == "mac" && e.kind == "app").collect();
        if log.len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let weights = log.iter().find(|e| e.title == "Copied weights.bin from cell1 to this Mac").expect("one line per item");
    // sha256("abc")
    assert!(weights.command.contains("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"), "{}", weights.command);
    assert!(weights.command.contains(&mac_dir.join("weights.bin").display().to_string()) && weights.command.contains("3 bytes"), "{}", weights.command);
    assert!(!weights.simulated && weights.class == "app" && weights.changes);
    let folder = log.iter().find(|e| e.title == "Copied cfg from cell1 to this Mac").unwrap();
    assert!(folder.command.contains("1 files, 2 bytes · sha256 "), "{}", folder.command);
}

#[test]
fn key_folders_on_the_mac_need_mac_scope_to_read() {
    let home = temp("files-secret-home");
    for d in [".ssh", "Library/Keychains", ".gnupg", ".aws", ".config/gh", ".kube", "Documents"] {
        std::fs::create_dir_all(home.join(d)).unwrap();
    }
    std::os::unix::fs::symlink(home.join(".ssh"), home.join("Documents/keys")).unwrap();
    let h = home.display().to_string();
    for p in [format!("{h}/.ssh"), format!("{h}/.ssh/id_ed25519"), format!("{h}/Library/Keychains/login.keychain-db"), format!("{h}/.gnupg"), format!("{h}/.aws/credentials"), format!("{h}/.config/gh/hosts.yml"), format!("{h}/.kube/config"), format!("{h}/Documents/keys/id_rsa")] {
        assert!(mac::is_secret_path(&home, &p), "{p}");
    }
    for p in [h.clone(), format!("{h}/Documents"), format!("{h}/.config"), format!("{h}/.sshx")] {
        assert!(!mac::is_secret_path(&home, &p), "{p}");
    }
}

/// r4 F5 on a fake node running the real sparkjob.py: a file the cluster user can't read
/// comes back as the permission refusal (no Python traceback), and Read with sudo… runs root's
/// own verified sparkjob.py with only the password on sudo's stdin.
#[tokio::test]
async fn a_root_only_file_is_explained_and_read_with_sudo() {
    let (files, fakes, homes) = setup("rootonly", 1).await;
    let home = &homes[0];
    write(&home.join("etc/netplan/40-cx7.yaml"), "network:\n  version: 2\n");
    let path = home.join("etc/netplan/40-cx7.yaml").display().to_string();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    let refused = files.read(false, "cell1", &path).await.unwrap_err();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let v: serde_json::Value = serde_json::from_str(&refused).unwrap_or_else(|e| panic!("{e}: {refused}"));
    assert_eq!((v["kind"].as_str(), v["node"].as_str()), (Some("permission"), Some("cell1")), "{refused}");
    assert!(refused.starts_with(r#"{"kind":"permission""#), "kind first, as the window reads every refusal: {refused}");
    let message = v["message"].as_str().unwrap();
    assert!(message.contains("can read 40-cx7.yaml (") && message.ends_with("Reading it needs sudo, which needs Admin."), "{message}");
    let facts: serde_json::Value = serde_json::from_str(v["detail"].as_str().unwrap()).unwrap();
    assert_eq!(facts["mode"], "0");
    assert!(!refused.contains("PermissionError") && !refused.contains("Errno"), "no traceback: {refused}");

    // The fake's sudo has no real privilege, so root's read is of a file the user can read too.
    let file = files.read_sudo(false, "cell1", &path).await.unwrap();
    assert_eq!(file.content, "network:\n  version: 2\n");
    assert!(!file.writable, "saving it goes through Save with sudo");
    let stdins = fakes[0].stdin.lock().unwrap().clone();
    let (cmd, password) = stdins.iter().rev().find(|(c, _)| c.starts_with("sudo") && c.contains("sparkjob.py' read")).expect("read through sudo");
    assert!(cmd.contains("exec python3 -I ") && !cmd.contains(".sparkbox/sparkjob.py"), "root's own verified copy: {cmd}");
    assert_eq!(password, b"pw\n", "only the password goes to sudo's stdin");
}

/// r4 F2: copying a key folder off this Mac, or a folder that holds one, is a Mac-scope read
/// for every item (.ssh from ~, Keychains from ~/Library, gh from ~/.config, ~ from /Users),
/// in Demo and Live alike, whatever the case of the name.
#[test]
fn copying_a_key_folder_or_one_that_holds_it_needs_mac_scope() {
    use super::commands::gate_place;
    let base = temp("files-keys-base");
    let home = base.join("Users/sparkuser");
    for d in [".ssh", "Library/Keychains", ".config/gh", ".config/nvim", "Documents/notes"] {
        std::fs::create_dir_all(home.join(d)).unwrap();
    }
    let h = home.display().to_string();
    let users = base.join("Users");
    let node = Path::new("/tmp");
    for (dir, name) in [(home.clone(), ".ssh"), (home.join("Library"), "Keychains"), (home.join(".config"), "gh"), (users.clone(), "sparkuser"), (home.clone(), "Library"), (home.clone(), ".config"), (home.clone(), ".SSH")] {
        let up = spec(MAC, &dir, &[name], "cell1", node);
        assert_eq!(gate_place(&up, &home), MAC, "{}/{name}", dir.display());
        assert!(mac::holds_secrets(&home, &format!("{}/{name}", dir.display())), "{}/{name}", dir.display());
    }
    // One key folder among ordinary items is enough; ordinary items alone stay node scope.
    assert_eq!(gate_place(&spec(MAC, &home, &["Documents", ".ssh"], "cell1", node), &home), MAC);
    assert_eq!(gate_place(&spec(MAC, &home, &["Documents"], "cell1", node), &home), "cell1");
    assert_eq!(gate_place(&spec(MAC, &home.join(".config"), &["nvim"], "cell1", node), &home), "cell1");
    assert!(!mac::holds_secrets(&home, &format!("{h}/Documents/notes")));
}

/// r4 F2: a Mac → Demo-node copy carries names, sizes and modes only; no byte of a real file on
/// this Mac reaches the Demo tree, key folders included.
#[tokio::test]
async fn no_bytes_of_a_mac_file_reach_the_demo_tree() {
    let (files, home) = mac_only("demo-no-bytes");
    let demo_home = crate::demo::world::home();
    write(&home.join(".ssh/id_ed25519"), "-----BEGIN OPENSSH PRIVATE KEY-----\nsecret\n");
    write(&home.join(".ssh/config"), "Host *\n");
    let (emit, seen) = collector();
    files.transfer(true, spec(MAC, &home, &[".ssh"], "cell3", Path::new(&demo_home)), emit).await.unwrap();
    assert!(matches!(finished(&seen).await, TransferEvent::Done { .. }));
    let fs = files.demo_fs();
    let items = fs.lock().unwrap().export("cell3", &demo_home, ".ssh").unwrap();
    assert_eq!(items.len(), 3, "the folder and its two files");
    assert!(items.iter().all(|(_, i)| i.content.is_none() && i.peek.is_none()), "names and sizes only");
    assert_eq!(items.iter().find(|(r, _)| r.ends_with("id_ed25519")).map(|(_, i)| i.size), Some(43));
    for f in ["~/.ssh/id_ed25519", "~/.ssh/config"] {
        assert_eq!(files.read(true, "cell3", f).await.unwrap_err(), super::DEMO_MAC_COPY, "{f}");
    }
}

/// r4 F10: shell startup files zsh and bash read at login and logout, and fish's config, are
/// never landed on, in any case; Downloads copies are quarantined in staging, before they're
/// in place.
#[tokio::test]
async fn startup_files_are_refused_in_any_case() {
    let (_files, home) = mac_only("startup");
    std::fs::create_dir_all(home.join(".config/fish")).unwrap();
    let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let h = home.display().to_string();
    for n in [".zlogin", ".zlogout", ".bash_login", ".bash_logout", ".ZSHRC", ".Bash_Profile", ".zshenv"] {
        assert_eq!(mac::landing(&home, &h, &names(&[n])).unwrap_err(), mac::PERSISTENCE, "{n}");
    }
    assert_eq!(mac::landing(&home, &format!("{h}/.config/fish"), &names(&["config.fish"])).unwrap_err(), mac::PERSISTENCE);
    assert_eq!(mac::landing(&home, &format!("{h}/.config"), &names(&["fish"])).unwrap_err(), mac::PERSISTENCE);
    assert_eq!(mac::landing(&home, &format!("{h}/.CONFIG/Fish"), &names(&["x"])).unwrap_err(), mac::PERSISTENCE, "case doesn't matter on APFS");
    assert_eq!(mac::landing(&home, &format!("{h}/Library/launchagents"), &names(&["x.plist"])).unwrap_err(), mac::PERSISTENCE);
    assert_eq!(mac::landing(&home, &format!("{h}/.config"), &names(&["nvim"])).unwrap(), mac::Landing::Elsewhere);
}

// ---- Rename runs exactly as written, whatever the name

#[tokio::test]
async fn rename_handles_awkward_names_and_refuses_to_replace() {
    let (files, fakes, homes) = setup("rename", 1).await;
    let home = &homes[0];
    let awkward = ["has \"quote\".txt", "$(touch pwned)", "`touch pwned2`", "two  spaces .txt", "it's.txt", "-rf"];
    for (i, name) in awkward.iter().enumerate() {
        let from = home.join(format!("plain{i}.txt"));
        write(&from, "x");
        files.rename(false, "cell1", &from.display().to_string(), name).await.unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(home.join(name).exists() && !from.exists(), "{name}");
        // And back again.
        files.rename(false, "cell1", &home.join(name).display().to_string(), &format!("back{i}.txt")).await.unwrap();
        assert!(home.join(format!("back{i}.txt")).exists());
    }
    assert!(!home.join("pwned").exists() && !home.join("pwned2").exists(), "nothing in a name ran");
    // The target-exists branch: refused, both kept.
    write(&home.join("a.txt"), "a");
    write(&home.join("b \"x\".txt"), "b");
    let e = files.rename(false, "cell1", &home.join("a.txt").display().to_string(), "b \"x\".txt").await.unwrap_err();
    assert!(e.contains("already exists"), "{e}");
    assert_eq!(std::fs::read_to_string(home.join("b \"x\".txt")).unwrap(), "b");
    assert!(home.join("a.txt").exists());
    let ran = fakes[0].raw.lock().unwrap().iter().filter(|c| c.contains("mv --")).count();
    assert_eq!(ran, awkward.len() * 2 + 1, "every rename reached the node as one exact command");
}

// ---- Verified transfers: nothing goes in place unless both tars succeeded and it all arrived

#[tokio::test]
async fn a_killed_receiver_places_nothing_and_keeps_the_source() {
    let (files, fakes, homes) = setup("killed", 1).await;
    let mac = temp("files-killed-mac");
    write(&mac.join("model/weights.bin"), &"w".repeat(40_000_000));
    fakes[0].kill_child_with("KILL", "tar -C");
    let (emit, seen) = collector();
    files.transfer(false, TransferSpec { op: Op::Move, ..spec(MAC, &mac, &["model"], "cell1", &homes[0]) }, emit).await.unwrap();
    match finished(&seen).await {
        TransferEvent::Failed { message } => assert!(message.contains("tar on cell1 was killed (SIGKILL)"), "{message}"),
        e => panic!("{e:?}"),
    }
    assert!(!homes[0].join("model").exists(), "nothing put in place");
    assert!(mac.join("model/weights.bin").exists(), "the move kept its source");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(leftovers(&homes[0]).is_empty(), "staging removed: {:?}", leftovers(&homes[0]));
}

#[tokio::test]
async fn a_receiver_that_ends_without_a_status_is_a_failure() {
    let (files, fakes, homes) = setup("nostatus", 1).await;
    let mac = temp("files-nostatus-mac");
    write(&mac.join("notes.txt"), "hello\n");
    fakes[0].close_without_status("tar -C");
    let (emit, seen) = collector();
    files.transfer(false, spec(MAC, &mac, &["notes.txt"], "cell1", &homes[0]), emit).await.unwrap();
    match finished(&seen).await {
        TransferEvent::Failed { message } => assert!(message.contains("ended without reporting success"), "{message}"),
        e => panic!("{e:?}"),
    }
    assert!(!homes[0].join("notes.txt").exists());
}

#[tokio::test]
async fn a_short_copy_is_caught_before_it_goes_in_place() {
    let (files, fakes, homes) = setup("short", 1).await;
    let mac = temp("files-short-mac");
    write(&mac.join("model/weights.bin"), &"w".repeat(1_000_000));
    write(&mac.join("model/config.json"), "{}");
    // The node's tar unpacks, then the weights come out empty (a disk that filled, a bad unpack).
    fakes[0].bin("tar", "/usr/bin/tar \"$@\" || exit $?\nif [ \"$1\" = -C ]; then find \"$2\" -name weights.bin -exec sh -c ': > \"$1\"' _ {} \\; ; fi\n");
    let (emit, seen) = collector();
    files.transfer(false, TransferSpec { op: Op::Move, ..spec(MAC, &mac, &["model"], "cell1", &homes[0]) }, emit).await.unwrap();
    match finished(&seen).await {
        TransferEvent::Failed { message } => assert!(message.contains("model/weights.bin arrived as 0 B, expected 976.6 KiB"), "{message}"),
        e => panic!("{e:?}"),
    }
    assert!(!homes[0].join("model").exists() && mac.join("model/weights.bin").exists(), "not placed, source kept");
}

#[tokio::test]
async fn old_staging_folders_are_swept_and_a_failed_cleanup_is_retried() {
    let (files, fakes, homes) = setup("sweep", 1).await;
    files.set_retry_every(Duration::from_millis(100));
    // The retry waits for Admin mode (`retry_leftovers`).
    let (gate, _, _) = crate::admin::testing::gate();
    gate.unlock(None).unwrap();
    files.set_gate(gate);
    let home = &homes[0];
    let stale = home.join(".sparkbox-incoming-9-dead");
    write(&stale.join("half.bin"), "partial");
    for p in [stale.join("half.bin"), stale.clone()] {
        let t = std::time::SystemTime::now() - Duration::from_secs(2 * 86_400);
        std::fs::File::open(&p).unwrap().set_modified(t).unwrap();
    }
    let fresh = home.join(".sparkbox-incoming-9-busy");
    write(&fresh.join("f"), "being written by another SparkBox");
    let mac = temp("files-sweep-mac");
    write(&mac.join("a.txt"), "a");

    // The cleanup after this copy fails once (the node "went away"): it's reported, then retried.
    fakes[0].bin("python3", &format!("#!/bin/sh\nif [ \"$2\" = cleanup ] && [ ! -e {flag} ]; then touch {flag}; echo 'connection lost' >&2; exit 1; fi\nexec {py} \"$@\"\n", flag = fakes[0].state_dir.join("failed-once").display(), py = which_python()));
    let (emit, seen) = collector();
    let spec1 = TransferSpec { policies: [("a.txt".to_string(), transfer::Policy::KeepBoth)].into(), ..spec(MAC, &mac, &["a.txt"], "cell1", home) };
    files.transfer(false, spec1, emit).await.unwrap();
    assert!(matches!(finished(&seen).await, TransferEvent::Leftover { .. } | TransferEvent::Done { .. }));
    for _ in 0..100 {
        if seen.lock().unwrap().iter().any(|e| matches!(e, TransferEvent::Done { .. })) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let left = seen.lock().unwrap().iter().find_map(|e| match e {
        TransferEvent::Leftover { place, path, .. } => Some((place.clone(), path.clone())),
        _ => None,
    });
    let (place, path) = left.expect("the leftover was reported");
    assert_eq!(place, "cell1");
    assert!(path.contains("/.sparkbox-incoming-"), "{path}");
    assert!(home.join("a.txt").exists(), "the copy itself succeeded");
    assert!(!stale.exists(), "a staging folder untouched for 2 days was swept before the copy");
    assert!(fresh.exists(), "a recent one (maybe another copy's) was left alone");
    for _ in 0..100 {
        if files.leftovers().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(files.leftovers().is_empty(), "retried once the node answered: {:?}", files.leftovers());
    assert!(!Path::new(&path).exists());
}

fn which_python() -> String {
    let out = std::process::Command::new("sh").arg("-c").arg("command -v python3").output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// R1-K (P1.46): the preflight's rule (`mac::secret_read`; du and backups use the narrower
/// `mac::is_secret_at`, tested below) covers the Mac's key folders and files (the list now includes .netrc,
/// .git-credentials, gcloud, azure, the docker config and the HF token cache) and a node's
/// secret paths (~/.ssh, ~/.netrc, ~/hf_token.txt, ~/glm53/api_key…), and the refusal message
/// is the secret-path one.
#[test]
fn du_preflight_and_backups_refuse_secret_paths() {
    let home = temp("files-secret-read-home");
    for d in [".ssh", ".config/gcloud", ".azure", ".cache/huggingface", ".docker", "Documents"] {
        std::fs::create_dir_all(home.join(d)).unwrap();
    }
    for f in [".netrc", ".git-credentials", ".cache/huggingface/token", ".docker/config.json", "Documents/notes.txt"] {
        write(&home.join(f), "x");
    }
    let h = home.display().to_string();
    // On this Mac: the new entries and the key folders are secret; ordinary files aren't.
    for p in [".netrc", ".git-credentials", ".config/gcloud", ".azure", ".cache/huggingface/token", ".docker/config.json", ".ssh"] {
        assert!(mac::secret_read(MAC, &format!("{h}/{p}"), &home), "Mac secret: {p}");
    }
    assert!(!mac::secret_read(MAC, &format!("{h}/Documents/notes.txt"), &home), "an ordinary file isn't secret");
    // Copying a folder that holds a key folder is secret too (holds_secrets).
    assert!(mac::secret_read(MAC, &h, &home), "the home folder holds ~/.ssh");

    // On a node: the same keys-and-credentials paths plus the two the owner keeps there.
    for p in ["~/.netrc", "/home/sparkuser/.netrc", "~/.ssh/id_ed25519", "~/hf_token.txt", "/home/sparkuser/hf_token.txt", "~/glm53/api_key", "~/.docker/config.json", "/root/.aws/credentials"] {
        assert!(mac::secret_read("cell1", p, &home), "node secret: {p}");
    }
    for p in ["~/models/m.safetensors", "/home/sparkuser/notes.txt", "~/glm53/inner.sh", "/etc/netplan/x"] {
        assert!(!mac::secret_read("cell1", p, &home), "not a node secret: {p}");
    }
    assert!(!mac::SECRET_PATH.is_empty());
}

/// P1 r1 fix (P1.46): the predicates the commands call. files_list and files_read gate on
/// `Files::is_secret`, which now covers a node's secrets as well as the Mac's key folders, so
/// opening ~/glm53/api_key on a node needs Mac scope (refused in look-only) like ~/.ssh on the
/// Mac. files_du and files_backups refuse only a secret path or one inside it, so the Mac's
/// home, ~/.cache/huggingface, ~/.config, ~/.docker and ~/Library can be measured; the
/// preflight, which reads its items whole, still refuses a folder that holds one.
#[test]
fn list_read_du_and_preflight_check_the_right_secret_rule() {
    use super::commands::{measure_refused, preflight_refused};
    let (files, home) = mac_only("secret-rules");
    for d in [".ssh", ".cache/huggingface/hub", ".config/gh", ".docker", "Library/Keychains", "Documents"] {
        std::fs::create_dir_all(home.join(d)).unwrap();
    }
    for f in [".netrc", ".cache/huggingface/token", ".docker/config.json", ".ssh/id_ed25519"] {
        write(&home.join(f), "x");
    }
    let h = home.display().to_string();
    let at = |p: &str| if p.is_empty() { h.clone() } else { format!("{h}/{p}") };

    // files_list / files_read on a node: node secrets need Mac scope, ordinary files don't.
    for p in ["~/glm53/api_key", "~/hf_token.txt", "/home/sparkuser/hf_token.txt", "~/.netrc", "/home/sparkuser/.ssh", "/home/sparkuser/.ssh/id_ed25519", "~/.cache/huggingface/token"] {
        assert!(files.is_secret("cell1", p), "node secret, Mac scope to list or read: {p}");
    }
    for p in ["~", "/home/sparkuser", "~/glm53", "~/glm53/inner.sh", "~/models", "~/.cache/huggingface", "/etc/netplan/40-cx7.yaml"] {
        assert!(!files.is_secret("cell1", p), "not a node secret: {p}");
    }
    // …and on this Mac as before.
    assert!(files.is_secret(MAC, &at(".ssh")) && files.is_secret(MAC, &at(".netrc")));
    assert!(!files.is_secret(MAC, &at("")) && !files.is_secret(MAC, &at(".cache/huggingface")));

    // files_du / files_backups on this Mac: a parent folder can be measured, a secret can't.
    for p in ["", ".cache", ".cache/huggingface", ".cache/huggingface/hub", ".config", ".docker", "Library", "Documents"] {
        assert!(!measure_refused(MAC, &[at(p)], &home), "Calculate size is allowed on ~/{p}");
    }
    for p in [".netrc", ".ssh", ".ssh/id_ed25519", ".cache/huggingface/token", ".docker/config.json", "Library/Keychains", ".config/gh"] {
        assert!(measure_refused(MAC, &[at(p)], &home), "Calculate size is refused on ~/{p}");
    }
    assert!(measure_refused(MAC, &[at("Documents"), at(".netrc")], &home), "one secret among several refuses the lot");
    // …and on a node.
    assert!(measure_refused("cell1", &["~/glm53/api_key".into()], &home));
    assert!(!measure_refused("cell1", &["~/glm53".into(), "~/models".into()], &home));

    // files_preflight: an item that holds a key folder is read whole, so it's refused.
    let node = Path::new("/tmp");
    assert!(preflight_refused(&spec(MAC, &home, &[".cache"], "cell1", node), &home), "~/.cache holds the HF token");
    assert!(preflight_refused(&spec(MAC, &home, &["Documents", ".ssh"], "cell1", node), &home));
    assert!(!preflight_refused(&spec(MAC, &home, &["Documents"], "cell1", node), &home));
    assert!(!preflight_refused(&spec(MAC, &home.join(".cache/huggingface"), &["hub"], "cell1", node), &home));
    assert!(preflight_refused(&spec("cell1", Path::new("/home/sparkuser/glm53"), &["api_key"], MAC, &home.join("Downloads")), &home));
    assert!(!preflight_refused(&spec("cell1", Path::new("/home/sparkuser/glm53"), &["inner.sh"], MAC, &home.join("Downloads")), &home));
}
