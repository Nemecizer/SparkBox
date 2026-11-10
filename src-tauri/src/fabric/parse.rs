//! Parsers for what the Fabric doctor runs: perftest (`ib_write_bw`, `ib_send_lat`),
//! nccl-tests (`all_reduce_perf`) and iputils `ping`. Each takes the command's
//! stdout and stderr together and returns what it measured, or why it couldn't.

/// The table row under a header line that contains `header`: the header's column
/// names and the first row of numbers after it.
fn table_row(text: &str, header: &str) -> Option<(Vec<String>, Vec<f64>)> {
    let mut lines = text.lines();
    while let Some(l) = lines.next() {
        if l.contains(header) {
            // Column names are single words, except "BW average[Gb/sec]" and "99% percentile[usec]".
            let mut cols: Vec<String> = Vec::new();
            let mut words = l.split_whitespace();
            while let Some(w) = words.next() {
                if w == "BW" || w.ends_with('%') {
                    cols.push(format!("{w} {}", words.next().unwrap_or("")));
                } else {
                    cols.push(w.to_string());
                }
            }
            for row in lines.by_ref() {
                let nums: Vec<f64> = row.split_whitespace().filter_map(|w| w.parse().ok()).collect();
                if nums.len() >= 3 {
                    return Some((cols, nums));
                }
                if row.trim().starts_with("---") && row.contains("-----") {
                    continue;
                }
            }
        }
    }
    None
}

/// Why perftest couldn't run: the first line that says so, in plain words.
fn perftest_failure(text: &str) -> String {
    let l = text.lines().map(str::trim).find(|l| {
        let l = l.to_ascii_lowercase();
        l.contains("couldn't") || l.contains("unable") || l.contains("failed") || l.contains("not found") || l.contains("error")
    });
    match l {
        Some(l) if l.contains("Couldn't connect") => format!("{l} (the server side didn't start or the address isn't reachable)"),
        Some(l) => l.to_string(),
        None => "perftest printed no result".into(),
    }
}

/// `ib_write_bw … --report_gbits`: the average bandwidth in Gb/s.
pub fn ib_write_bw(text: &str) -> Result<f64, String> {
    let (cols, nums) = table_row(text, "BW average").ok_or_else(|| perftest_failure(text))?;
    let i = cols.iter().position(|c| c.starts_with("BW average")).ok_or("no BW average column")?;
    let v = *nums.get(i).ok_or("short result row")?;
    // Without --report_gbits perftest reports MB/s.
    Ok(if cols[i].contains("MB/sec") { v * 8.0 / 1000.0 } else { v })
}

/// `ib_send_lat`: the typical one-way latency in µs.
pub fn ib_send_lat(text: &str) -> Result<f64, String> {
    let (cols, nums) = table_row(text, "t_typical").ok_or_else(|| perftest_failure(text))?;
    let i = cols.iter().position(|c| c.starts_with("t_typical")).ok_or("no t_typical column")?;
    nums.get(i).copied().ok_or_else(|| "short result row".into())
}

#[derive(Clone, Debug, PartialEq)]
pub struct AllReduce {
    /// "Avg bus bandwidth", Gb/s.
    pub avg_busbw_gbps: f64,
    /// Out-of-place bus bandwidth at the largest size, Gb/s.
    pub peak_busbw_gbps: f64,
    pub largest_bytes: u64,
    /// "# Out of bounds values : 0 OK".
    pub wrong: u64,
}

/// nccl-tests `all_reduce_perf`: bus bandwidth (GB/s in the output, Gb/s here).
pub fn all_reduce(text: &str) -> Result<AllReduce, String> {
    let avg = text
        .lines()
        .find_map(|l| l.trim_start_matches('#').trim().strip_prefix("Avg bus bandwidth")?.trim().trim_start_matches(':').trim().parse::<f64>().ok())
        .ok_or_else(|| {
            // mpirun frames its errors in dashed lines: the first line with words says what went wrong.
            let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| l.chars().any(char::is_alphabetic) && !l.starts_with('#')).collect();
            match lines.as_slice() {
                [] => "all_reduce_perf printed no result".to_string(),
                [one] => one.to_string(),
                [a, b, ..] => format!("{a} {b}"),
            }
        })?;
    // Rows: size count type redop root time algbw busbw #wrong time algbw busbw #wrong.
    let rows: Vec<Vec<&str>> = text.lines().filter(|l| !l.trim_start().starts_with('#')).map(|l| l.split_whitespace().collect::<Vec<_>>()).filter(|w| w.len() >= 9).collect();
    let last = rows.iter().filter_map(|w| Some((w[0].parse::<u64>().ok()?, w[7].parse::<f64>().ok()?))).max_by_key(|(size, _)| *size);
    let wrong = text.lines().find_map(|l| l.split("Out of bounds values :").nth(1)?.split_whitespace().next()?.parse::<u64>().ok()).unwrap_or(0);
    let (largest, peak) = last.unwrap_or((0, avg));
    Ok(AllReduce { avg_busbw_gbps: avg * 8.0, peak_busbw_gbps: peak * 8.0, largest_bytes: largest, wrong })
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ping {
    Ok { rtt_ms: f64 },
    /// The packet didn't fit: this end's MTU (`local error`) or a hop's (`Frag needed`).
    TooBig { mtu: Option<u32> },
    /// No replies at all.
    Lost,
}

/// iputils `ping -M do -s … -c …`.
pub fn ping(text: &str) -> Ping {
    let mtu_in = |l: &str| l.split("mtu").nth(1).and_then(|r| r.trim_start_matches(['=', ' ']).split(|c: char| !c.is_ascii_digit()).next()?.parse().ok());
    if let Some(l) = text.lines().find(|l| l.contains("message too long") || l.contains("Frag needed")) {
        return Ping::TooBig { mtu: mtu_in(l) };
    }
    let received = text.lines().find_map(|l| {
        let (_, rest) = l.split_once("packets transmitted, ")?;
        rest.split_whitespace().next()?.parse::<u32>().ok()
    });
    let rtt = text.lines().find_map(|l| l.split_once("rtt min/avg/max/mdev = ").and_then(|(_, v)| v.split('/').nth(1)?.parse::<f64>().ok()));
    match (received, rtt) {
        (Some(n), Some(rtt)) if n > 0 => Ping::Ok { rtt_ms: rtt },
        _ => Ping::Lost,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perftest_bandwidth_and_latency() {
        assert_eq!(ib_write_bw(include_str!("fixtures/ib_write_bw.txt")), Ok(92.95));
        assert_eq!(ib_send_lat(include_str!("fixtures/ib_send_lat.txt")), Ok(2.12));
        let refused = ib_write_bw(include_str!("fixtures/ib_write_bw_refused.txt")).unwrap_err();
        assert!(refused.starts_with("Couldn't connect to 192.168.101.3:18515"), "{refused}");
        assert_eq!(ib_write_bw("bash: line 1: ib_write_bw: command not found\n").unwrap_err(), "bash: line 1: ib_write_bw: command not found");
        // MB/s when --report_gbits was left off.
        let mb = " #bytes     #iterations    BW peak[MB/sec]    BW average[MB/sec]   MsgRate[Mpps]\n 65536      886342           0.00               11618.75  \t\t   0.177289\n";
        assert!((ib_write_bw(mb).unwrap() - 92.95).abs() < 0.01);
    }

    #[test]
    fn nccl_tests_bus_bandwidth() {
        let r = all_reduce(include_str!("fixtures/all_reduce_perf.txt")).unwrap();
        assert!((r.avg_busbw_gbps - 89.0688).abs() < 0.001, "{r:?}");
        assert!((r.peak_busbw_gbps - 117.28).abs() < 0.001, "{r:?}");
        assert_eq!(r.largest_bytes, 8_589_934_592);
        assert_eq!(r.wrong, 0);
        let e = all_reduce("----------\nmpirun was unable to launch the specified application as it could not access\nor execute an executable:\n----------\n").unwrap_err();
        assert_eq!(e, "mpirun was unable to launch the specified application as it could not access or execute an executable:");
    }

    #[test]
    fn ping_outcomes() {
        assert_eq!(ping(include_str!("fixtures/ping_ok.txt")), Ping::Ok { rtt_ms: 0.110 });
        assert_eq!(ping(include_str!("fixtures/ping_local_mtu.txt")), Ping::TooBig { mtu: Some(1500) });
        assert_eq!(ping(include_str!("fixtures/ping_frag_needed.txt")), Ping::TooBig { mtu: Some(1500) });
        assert_eq!(ping(include_str!("fixtures/ping_lost.txt")), Ping::Lost);
        assert_eq!(ping(""), Ping::Lost);
    }
}
