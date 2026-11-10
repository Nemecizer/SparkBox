//! The controller lease (P1.2): which SparkBox, release or dev, is changing a node right now.
//!
//! Each node keeps ~/.sparkbox/controller.json {instance, tag, mac, heartbeat}, written by
//! sparkjob.py's `lease` verb under a lock with the node's own clock. Before a change the exit
//! claims it (the answer is kept for 5 s): when the other instance's heartbeat is under 45 s
//! old, the change is refused with "SparkBox (dev) is controlling cell2 right now". While Admin
//! mode is unlocked, or a fence is open for this build, the claim is refreshed every 15 s, and in
//! the release build it also is while work runs on the node, whatever Admin mode's state
//! (`Exit::renewals`). A clean fence close or a quit, while not look-only, gives it back on the
//! node (`lease release`); a relock or look-only turning on only lets go of it here
//! (`drop_local`), and the node's own 45 s TTL frees it, so neither writes on a node. After a
//! relock, the next round's renewal takes up again a lease with work running under it.
//!
//! The lease is SparkBox's own bookkeeping, not a change to the node: look-only mode never
//! writes it. Its claims run on the control connection outside the exit (the exit itself
//! asks for them), and only a change of holder is written to the activity log. A claim that
//! finds ~/.sparkbox/sparkjob.py isn't this build's copy (COPY_STALE: nothing ran) puts the
//! copy back and asks once more, as `Live::sparkjob` does.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::{sh_quote, wire, Live, SPARKJOB};
use crate::errors::{ErrorKind, SbError};

/// A heartbeat older than this is stale: anyone may take the lease.
pub const TTL_S: u64 = 45;
/// A claim's answer is good for this long.
const KEEP: Duration = Duration::from_secs(5);
/// How often a held lease is renewed.
pub const REFRESH: Duration = Duration::from_secs(15);

/// `git describe --tags --always --dirty` when this build was made (build.rs), else "untagged".
pub fn build_tag() -> &'static str {
    option_env!("SPARKBOX_BUILD_TAG").filter(|t| !t.is_empty()).unwrap_or("untagged")
}

/// This Mac's name (`scutil --get ComputerName`), read once.
pub fn mac_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        std::process::Command::new("/usr/sbin/scutil")
            .args(["--get", "ComputerName"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "this Mac".into())
    })
}

/// Who claims: this build, its tag and the Mac it runs on.
#[derive(Clone, Debug, PartialEq)]
pub struct Who {
    pub instance: String,
    pub tag: String,
    pub mac: String,
}

/// The holder sparkjob.py names when the lease is someone else's.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Holder {
    pub instance: String,
    pub tag: String,
    pub mac: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Reply {
    held: bool,
    by: Option<Holder>,
    age_s: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
enum Claim {
    Mine,
    Other(Holder, u64),
}

/// A node's lease as Settings › Fence shows it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaseView {
    pub node: String,
    /// release | dev, or None when nobody holds it.
    pub held_by: Option<String>,
    pub tag: Option<String>,
    pub mac: Option<String>,
    pub age_s: Option<u64>,
    pub mine: bool,
    /// Why it couldn't be read.
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Leases {
    seen: Mutex<HashMap<String, (Instant, Claim)>>,
}

impl Leases {
    /// Claims `node` for `who` (the answer kept for 5 s): Ok when it's ours now, OtherController
    /// when the other instance's heartbeat is fresh. A node that can't answer refuses the change.
    /// The exit re-checks look-only once this returns and drops the claim (`drop_local`) if it
    /// came on while the claim was in flight, so `held()` stays empty and the change is refused.
    pub async fn claim(&self, live: &Live, node: &str, who: &Who) -> Result<(), SbError> {
        let cached = self.seen.lock().unwrap().get(node).filter(|(at, _)| at.elapsed() < KEEP).map(|(_, c)| c.clone());
        let claim = match cached {
            Some(c) => c,
            None => {
                let was_mine = matches!(self.seen.lock().unwrap().get(node), Some((_, Claim::Mine)));
                let cmd = format!("python3 ~/.sparkbox/sparkjob.py lease claim {} {} {} {TTL_S}", sh_quote(&who.instance), sh_quote(&who.tag), sh_quote(&who.mac));
                let claim = ask(live, node, &cmd).await.map_err(|e| SbError::failed(format!("Couldn't check which SparkBox controls {node}: {e}")).on(node))?;
                if claim == Claim::Mine && !was_mine {
                    live.log(node, "command", &cmd, Instant::now(), Ok(Some(0)));
                }
                self.seen.lock().unwrap().insert(node.to_string(), (Instant::now(), claim.clone()));
                claim
            }
        };
        match claim {
            Claim::Mine => Ok(()),
            Claim::Other(h, _) => Err(SbError::new(ErrorKind::OtherController, format!("SparkBox ({}) is controlling {node} right now.", h.instance)).on(node)),
        }
    }

    /// Gives `node`'s lease up if it's ours (best effort).
    pub async fn release(&self, live: &Live, node: &str, instance: &str) {
        let was_mine = matches!(self.seen.lock().unwrap().remove(node), Some((_, Claim::Mine)));
        if !was_mine || !live.is_connected(node) {
            return;
        }
        let cmd = format!("python3 ~/.sparkbox/sparkjob.py lease release {}", sh_quote(instance));
        if ask_raw(live, node, &cmd).await.is_ok() {
            live.log(node, "command", &cmd, Instant::now(), Ok(Some(0)));
        }
    }

    /// Lets go of `node`'s lease here without telling the node: this build stops renewing it,
    /// and the node's own TTL frees it (a relock, look-only turning on).
    pub fn drop_local(&self, node: &str) {
        self.seen.lock().unwrap().remove(node);
    }

    /// The nodes whose lease this build holds, as far as it knows.
    pub fn held(&self) -> Vec<String> {
        self.seen.lock().unwrap().iter().filter(|(_, (_, c))| *c == Claim::Mine).map(|(n, _)| n.clone()).collect()
    }

    /// Forgets the cached answers (tests).
    #[cfg(test)]
    pub fn forget(&self) {
        self.seen.lock().unwrap().clear();
    }
}

/// Runs a lease verb on the node's control connection and reads its answer.
async fn ask(live: &Live, node: &str, cmd: &str) -> Result<Claim, String> {
    let out = ask_raw(live, node, cmd).await?;
    let reply: Reply = serde_json::from_str(out.trim().lines().last().unwrap_or("")).map_err(|e| format!("unexpected reply from sparkjob ({e})"))?;
    Ok(match (reply.held, reply.by) {
        (true, _) => Claim::Mine,
        (false, Some(by)) => Claim::Other(by, reply.age_s.unwrap_or(0.0) as u64),
        (false, None) => return Err("sparkjob didn't say who holds it".into()),
    })
}

async fn ask_raw(live: &Live, node: &str, cmd: &str) -> Result<String, String> {
    let mut out = ask_once(live, node, cmd).await?;
    // The copy changed since this connection checked it (another SparkBox's, put there since):
    // nothing ran, and the connection checks its helpers again. Put it back and ask once more.
    if out.code == Some(super::COPY_STALE) {
        out = ask_once(live, node, cmd).await?;
    }
    if out.code != Some(0) {
        let why = out.stderr.trim().lines().last().unwrap_or("").to_string();
        return Err(if why.is_empty() { format!("exit {}", out.code.map(|c| c.to_string()).unwrap_or_else(|| "without status".into())) } else { why });
    }
    Ok(out.stdout)
}

async fn ask_once(live: &Live, node: &str, cmd: &str) -> Result<super::ssh::Output, String> {
    // The approved helper refresh, outside the exit like every copy into ~/.sparkbox.
    live.ensure_file(node, "sparkjob.py", SPARKJOB).await?;
    let session = live.session(node).ok_or_else(|| super::not_connected(node))?;
    session.exec(&wire(cmd), None, Duration::from_secs(15)).await
}

/// The command lease_state reads with (a read: the file and the node's clock).
pub const SHOW: &str = "cat ~/.sparkbox/controller.json 2>/dev/null; echo; date +%s";

/// What `SHOW` printed, for `node`, from `mine`'s point of view.
pub fn parse_show(node: &str, out: &str, mine: &str) -> LeaseView {
    #[derive(Deserialize)]
    struct File {
        instance: String,
        #[serde(default)]
        tag: String,
        #[serde(default)]
        mac: String,
        heartbeat: f64,
    }
    let lines: Vec<&str> = out.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let now: Option<f64> = lines.last().and_then(|l| l.parse().ok());
    let file: Option<File> = lines.iter().find(|l| l.starts_with('{')).and_then(|l| serde_json::from_str(l).ok());
    match (file, now) {
        (Some(f), Some(now)) => {
            let age = (now - f.heartbeat).max(0.0) as u64;
            if age >= TTL_S {
                return LeaseView { node: node.into(), ..Default::default() };
            }
            LeaseView { node: node.into(), mine: f.instance == mine, held_by: Some(f.instance), tag: Some(f.tag), mac: Some(f.mac), age_s: Some(age), error: None }
        }
        _ => LeaseView { node: node.into(), ..Default::default() },
    }
}

#[cfg(test)]
mod tests {
    /// build.rs names the tree this build came from (it reads CARGO_MANIFEST_DIR when it runs, so a
    /// copy sharing the target folder can't leave its own answer behind).
    #[test]
    fn the_build_names_its_git_tag() {
        let tag = super::build_tag();
        assert!(tag.chars().all(|c| c.is_ascii_alphanumeric() || "-._+".contains(c)), "{tag}");
        if std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.git").exists() {
            assert_ne!(tag, "untagged", "a git checkout's build carries `git describe`");
        }
    }
}
