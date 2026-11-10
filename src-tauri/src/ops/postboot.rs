//! Read-only checks after a confirmed reboot. Missing evidence is a warning, never a pass.
use std::collections::BTreeMap;
use std::time::Duration;
use super::Ops;
use crate::tasks::TaskHandle;

pub const READ: &str = "echo '--- boot'; cat /proc/sys/kernel/random/boot_id; echo '--- kernel'; uname -r; echo '--- gpu'; nvidia-smi --query-gpu=name,driver_version --format=csv,noheader; echo '--- fabric'; for i in enp1s0f1np1 enP2p1s0f1np1; do printf '%s ' \"$i\"; cat /sys/class/net/$i/speed /sys/class/net/$i/mtu | tr '\\n' ' '; echo; done; echo '--- docker'; docker info --format '{{.ServerVersion}}'; echo '--- system'; systemctl is-system-running; echo '--- failed'; systemctl --failed --no-legend --plain; echo '--- end'";

#[derive(Clone, Debug, Default)]
pub struct Expected { pub boot: Option<String>, pub kernel: Option<String>, pub driver: Option<String>, pub peers: Vec<(String, String)> }

pub fn judge(text: &str, expected: &Expected) -> Vec<String> {
    judge_boot(text, expected, false)
}
/// Pre-install health requires the same boot as the independently collected baseline.
/// Restart verification always uses `judge`, which still requires a different boot.
pub(super) fn judge_running(text: &str, expected: &Expected) -> Vec<String> {
    judge_boot(text, expected, true)
}
fn judge_boot(text: &str, expected: &Expected, same_boot: bool) -> Vec<String> {
    let mut sections: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut key = "";
    for line in text.lines() { if let Some(k) = line.strip_prefix("--- ") { key = k; sections.entry(k).or_default(); } else if !line.trim().is_empty() { sections.entry(key).or_default().push(line.trim()); } }
    let one = |k| sections.get(k).and_then(|v| (v.len() == 1).then(|| v[0]));
    let mut warnings = Vec::new();
    if !sections.contains_key("end") { warnings.push("post-reboot read did not finish".into()); }
    match (one("boot"), expected.boot.as_deref()) { (Some(b), Some(old)) if !b.is_empty() && ((b == old) == same_boot) => {}, _ => warnings.push(if same_boot { "current boot changed or could not be confirmed" } else { "new boot identity could not be confirmed" }.into()) }
    match (one("kernel"), expected.kernel.as_deref()) { (Some(k), Some(want)) if k == want => {}, (got, want) => warnings.push(format!("kernel is {}, expected {}", got.unwrap_or("unknown"), want.unwrap_or("unknown"))) }
    let gpu = one("gpu").and_then(|s| s.split_once(','));
    match gpu {
        Some((name, driver)) if name.contains("GB10") => {
            let driver = driver.trim();
            if expected.driver.as_deref() != Some(driver) { warnings.push(format!("GPU driver is {driver}, before reboot {}", expected.driver.as_deref().unwrap_or("unknown"))); }
            for (node, peer) in &expected.peers { if peer != driver { warnings.push(format!("GPU driver {driver} differs from {node} ({peer})")); } }
        }
        _ => warnings.push("GB10 GPU and driver were not reported".into()),
    }
    for iface in ["enp1s0f1np1", "enP2p1s0f1np1"] {
        let want = format!("{iface} 200000 9000");
        if !sections.get("fabric").is_some_and(|v| v.iter().any(|l| l.split_whitespace().collect::<Vec<_>>().join(" ") == want)) { warnings.push(format!("{iface}: expected 200000 Mb/s and MTU 9000")); }
    }
    if !one("docker").is_some_and(|v| v.starts_with(|c: char| c.is_ascii_digit())) { warnings.push("Docker server did not report its version".into()); }
    if one("system") != Some("running") { warnings.push(format!("systemd state is {}", one("system").unwrap_or("unknown"))); }
    match sections.get("failed") { Some(v) if v.is_empty() => {}, Some(v) => warnings.push(format!("failed units: {}", v.join("; "))), None => warnings.push("failed units were not reported".into()) }
    warnings
}

impl Ops {
    pub(super) async fn checks_after_restart(&self, demo: bool, task: &TaskHandle, node: &str) -> Option<String> {
        if demo { task.step_skipped("Checks after the restart", "Demo: hardware checks are not performed"); return None; }
        task.step(format!("Checks after the restart of {}", self.name_of(node)));
        let expected = self.postboot.lock().unwrap().remove(node).unwrap_or_default();
        let warnings = match self.live.run(node, READ, None, Duration::from_secs(30)).await {
            Ok(o) if o.code == Some(0) => judge(&o.stdout, &expected),
            Ok(_) => vec!["post-reboot read failed".into()],
            Err(e) => vec![format!("post-reboot read failed: {}", crate::live::audit::redact(&e))],
        };
        if warnings.is_empty() { task.step_ok(Some("New boot, expected kernel and driver, GB10, both fabric links, Docker and systemd checked.".into())); None }
        else { let warning = format!("{} restarted, but {}", self.name_of(node), warnings.join("; ")); task.step_ok(Some(warning.clone())); Some(warning) }
    }
}

#[cfg(test)] mod tests {
    use super::*;
    const GOOD: &str = "--- boot\nnew\n--- kernel\n7.0-nvidia\n--- gpu\nNVIDIA GB10, 580.1\n--- fabric\nenp1s0f1np1 200000 9000\nenP2p1s0f1np1 200000 9000\n--- docker\n28.0.0\n--- system\nrunning\n--- failed\n--- end\n";
    #[test] fn complete_evidence_passes_and_each_missing_fact_warns() {
        assert!(crate::ops::classify::read_only(READ), "post-boot checks must remain read-only");
        assert!(!crate::ops::classify::escalates(READ));
        let e = Expected { boot: Some("old".into()), kernel: Some("7.0-nvidia".into()), driver: Some("580.1".into()), peers: vec![("cell2".into(), "580.1".into())] };
        assert!(judge(GOOD, &e).is_empty());
        for (from,to,word) in [("new", "old", "boot"), ("7.0-nvidia", "6.0", "kernel"), ("GB10", "other", "GB10"), ("580.1", "581.0", "driver"), ("200000", "100000", "enp1"), ("9000", "1500", "MTU"), ("28.0.0", "", "Docker"), ("running", "degraded", "systemd"), ("--- failed\n", "--- failed\nbroken.service failed\n", "failed units"), ("--- end", "", "finish")] {
            assert!(judge(&GOOD.replace(from,to), &e).iter().any(|w| w.contains(word)), "{word}");
        }
        assert!(judge("", &e).len() >= 8);
    }
}
