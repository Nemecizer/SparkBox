//! Expected GPU reservations are not pressure. MemAvailable already excludes the pool:
//! never add the reservation back to it. Keep the percentage fallback if headroom is unknown.
use crate::model::NodeSnapshot;

const GIB: f64 = 1_073_741_824.0;

pub(super) fn available_gib(n: &NodeSnapshot) -> Option<f64> {
    let m = &n.mem;
    // Older/incomplete fixtures and malformed readings must not look like zero headroom.
    (m.total > 0 && m.available <= m.total && m.used <= m.total
        && m.used.abs_diff(m.total - m.available) <= 1_048_576)
        .then_some(m.available as f64 / GIB)
}

pub(super) fn reserved(n: &NodeSnapshot) -> u64 {
    let measured = if n.gpu.readable && n.mem.gpu_alloc <= n.mem.total { n.mem.gpu_alloc } else { 0 };
    let declared: f64 = n.containers.iter().filter(|c| c.state == "running").filter_map(|c| {
        // Only SparkBox's native ranks carry a configured reservation. Old or external
        // containers use the measured allocation, never their image/name as a guess.
        let l = &c.labels;
        if !l.get("sparkbox.instance").is_some_and(|x| x == "release" || x == "dev")
            || !l.get("sparkbox.deployment").is_some_and(|x| !x.is_empty())
            || !l.get("sparkbox.path").is_some_and(|x| !x.is_empty())
            || !l.get("sparkbox.role").is_some_and(|x| matches!(x.as_str(), "single" | "head" | "worker")) {
            return None;
        }
        let share: f64 = l.get("sparkbox.gpu-share")?.parse().ok()?;
        (share.is_finite() && share > 0.0 && share < 1.0).then_some(share * n.mem.total as f64)
    }).sum();
    // Measured and declared describe the same pool; adding them would count it twice.
    measured.max(declared.min(n.mem.total as f64) as u64).min(n.mem.used)
}

pub(super) fn used_pct(n: &NodeSnapshot, headroom_enabled: bool) -> (f64, u64) {
    let pool = if headroom_enabled && available_gib(n).is_some() { reserved(n) } else { 0 };
    let pct = if n.mem.total > 0 { n.mem.used.saturating_sub(pool) as f64 / n.mem.total as f64 * 100.0 } else { 0.0 };
    (pct, pool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Container, MemStats};

    #[test]
    fn measured_and_declared_are_not_added_or_treated_as_available() {
        let mut n = NodeSnapshot { mem: MemStats { total: 256 << 30, used: 250 << 30, available: 6 << 30, gpu_alloc: 224 << 30, ..Default::default() }, ..Default::default() };
        n.containers.push(Container { state: "running".into(), labels: [
            ("sparkbox.instance", "release"), ("sparkbox.deployment", "d"),
            ("sparkbox.path", "/model"), ("sparkbox.role", "worker"), ("sparkbox.gpu-share", "0.875"),
        ].into_iter().map(|(k,v)| (k.into(),v.into())).collect(), ..Default::default() });
        assert_eq!(reserved(&n), 224 << 30);
        assert_eq!(available_gib(&n), Some(6.0));
        assert_eq!(used_pct(&n, true), (26.0 / 256.0 * 100.0, 224 << 30));
        assert_eq!(used_pct(&n, false), (250.0 / 256.0 * 100.0, 0));
        n.containers[0].state = "exited".into();
        n.gpu.readable = false;
        assert_eq!(reserved(&n), 0);
    }
}
