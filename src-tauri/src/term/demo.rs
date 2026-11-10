//! Demo-mode terminal: a small bash look-alike so the Terminal window can be tried without the
//! cluster. Files come from the demo filesystem (the same ones the Files window shows) and
//! system commands (nvidia-smi, docker, df, systemctl…) from the simulated cluster, so every view
//! agrees. Supports `;`, `&&`, pipes into `grep`, `head`, `tail`, `wc`, `sort`, `uniq` and `cut`,
//! `>` / `>>` (a writable /tmp included), history (↑ ↓), `$?`, `nvidia-smi --query-gpu=…` with
//! the fields asked for, and `ping` / `sleep` that run until Ctrl-C. A node that goes offline
//! drops its sessions, like a real SSH session.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;

use super::{Input, Out, Target};
use crate::demo::{world, DemoCluster};
use crate::files::demo::DemoFs;

fn crlf(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\n', "\r\n")
}

const BLUE: &str = "\x1b[01;34m";
const CYAN: &str = "\x1b[01;36m";
const GREEN: &str = "\x1b[01;32m";
const RESET: &str = "\x1b[0m";

/// A command that keeps running until it finishes or Ctrl-C.
enum Job {
    Sleep { left: u32 },
    Ping { host: String, ip: String, seq: u32, count: Option<u32> },
}

struct Shell {
    node: String,
    user: String,
    home: String,
    cwd: String,
    container: Option<String>,
    status: u32,
    history: Vec<String>,
    cluster: Arc<Mutex<DemoCluster>>,
    fs: Arc<Mutex<DemoFs>>,
}

fn lsdate(mtime: u64) -> String {
    let (_, m, d, h, min) = crate::files::demo::calendar(mtime);
    const MON: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {:>2} {h:02}:{min:02}", MON[(m as usize).saturating_sub(1) % 12], d)
}

/// sshd's "Last login: Wed Oct 28 08:41:12 2026": this morning, a few hours before now.
fn last_login() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (y, m, d, h, mi, s, wd) = crate::demo::ops::civil(now.saturating_sub(4 * 3_600 + 17 * 60));
    const MON: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    const DAY: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    format!("{} {} {d:>2} {h:02}:{mi:02}:{s:02} {y}", DAY[wd], MON[(m as usize).saturating_sub(1) % 12])
}

fn human(bytes: u64) -> String {
    let units = ["", "K", "M", "G", "T"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{bytes}") } else if v < 10.0 { format!("{v:.1}{}", units[i]) } else { format!("{v:.0}{}", units[i]) }
}

impl Shell {
    fn prompt(&self) -> String {
        match &self.container {
            Some(c) => format!("root@{c}:{}# ", self.cwd),
            None => {
                let shown = self.shown_cwd();
                // Ubuntu's bash sets the window title on every prompt (OSC 0); the tab follows it.
                format!("\x1b]0;{}@{}: {shown}\x07{GREEN}{}@{}{RESET}:{BLUE}{shown}{RESET}$ ", self.user, self.node, self.user, self.node)
            }
        }
    }

    fn shown_cwd(&self) -> String {
        if self.cwd == self.home {
            "~".into()
        } else if let Some(rest) = self.cwd.strip_prefix(&format!("{}/", self.home)) {
            format!("~/{rest}")
        } else {
            self.cwd.clone()
        }
    }

    fn abs(&self, p: &str) -> String {
        if p.starts_with('/') || p.starts_with('~') {
            p.to_string()
        } else {
            format!("{}/{p}", self.cwd.trim_end_matches('/'))
        }
    }

    fn fleet(&self, cmd: &str) -> (u32, String, String) {
        self.cluster.lock().unwrap().fleet_reply(&self.node, cmd)
    }

    fn ls(&self, args: &[&str]) -> (u32, String) {
        let long = args.iter().any(|a| a.starts_with('-') && a.contains('l'));
        let all = args.iter().any(|a| a.starts_with('-') && a.contains('a'));
        let human_sizes = args.iter().any(|a| a.starts_with('-') && a.contains('h'));
        let target = args.iter().find(|a| !a.starts_with('-')).map(|a| self.abs(a)).unwrap_or_else(|| self.cwd.clone());
        let listing = match self.fs.lock().unwrap().list(&self.node, &target) {
            Ok(l) => l,
            Err(e) if e.contains("is a file") => return (0, format!("{}\n", args.iter().find(|a| !a.starts_with('-')).unwrap_or(&""))),
            Err(e) if e.contains("Permission denied") => return (2, format!("ls: cannot open directory '{target}': Permission denied\n")),
            Err(_) => return (2, format!("ls: cannot access '{target}': No such file or directory\n")),
        };
        let mut entries = listing.entries;
        entries.retain(|e| all || !e.name.starts_with('.'));
        entries.sort_by(|a, b| a.name.to_lowercase().trim_start_matches('.').cmp(b.name.to_lowercase().trim_start_matches('.')));
        let paint = |e: &crate::files::Entry| {
            let color = if e.kind == "link" { CYAN } else if e.kind == "dir" { BLUE } else if e.mode.contains('x') { GREEN } else { "" };
            if color.is_empty() { e.name.clone() } else { format!("{color}{}{RESET}", e.name) }
        };
        if !long {
            let names: Vec<String> = entries.iter().map(paint).collect();
            return (0, if names.is_empty() { String::new() } else { format!("{}\n", names.join("  ")) });
        }
        let total: u64 = entries.iter().map(|e| e.size.div_ceil(4096) * 4).sum();
        let mut out = format!("total {total}\n");
        if all {
            // -a lists the folder itself and its parent first, as GNU ls does.
            let mine = |p: &str| p == self.home || p.starts_with(&format!("{}/", self.home));
            let parent = target.rsplit_once('/').map(|(p, _)| if p.is_empty() { "/" } else { p }).unwrap_or("/");
            let newest = entries.iter().map(|e| e.mtime).max().unwrap_or(0);
            for (name, path) in [(".", target.as_str()), ("..", parent)] {
                let owner = if mine(path) { self.user.as_str() } else { "root" };
                out += &format!("drwxr-xr-x {:>2} {owner:<8} {owner:<8} {:>12} {} {BLUE}{name}{RESET}\n", 2, 4096, lsdate(newest));
            }
        }
        for e in &entries {
            let owner = e.owner.clone().unwrap_or_default();
            let size = if human_sizes { human(e.size) } else { e.size.to_string() };
            let link = e.target.as_ref().map(|t| format!(" -> {t}")).unwrap_or_default();
            out += &format!("{} {:>2} {owner:<8} {owner:<8} {size:>12} {} {}{link}\n", e.mode, if e.kind == "dir" { 2 } else { 1 }, lsdate(e.mtime), paint(e));
        }
        (0, out)
    }

    fn cat(&self, args: &[&str], head: Option<usize>) -> (u32, String) {
        let mut out = String::new();
        let mut code = 0;
        for a in args.iter().filter(|a| !a.starts_with('-')) {
            match self.fs.lock().unwrap().read(&self.node, &self.abs(a)) {
                Ok(f) if f.large => out += f.head.as_deref().unwrap_or_default(),
                Ok(f) => out += &f.content,
                Err(e) => {
                    code = 1;
                    let why = if e.contains("folder") { "Is a directory" } else if e.contains("binary") { "binary file (not shown)" } else if e.contains("Permission") { "Permission denied" } else { "No such file or directory" };
                    out += &format!("{}: {a}: {why}\n", if head.is_some() { "head" } else { "cat" });
                }
            }
        }
        if let Some(n) = head {
            out = out.lines().take(n).map(|l| format!("{l}\n")).collect();
        }
        (code, out)
    }

    /// One simple command (no `;`, `&&` or `|`): exit status and output.
    fn simple(&mut self, line: &str) -> (u32, String, Option<Job>) {
        let words: Vec<String> = line.split_whitespace().map(|w| if w == "$?" { self.status.to_string() } else { w.trim_matches(|c| c == '"' || c == '\'').to_string() }).collect();
        let args: Vec<&str> = words.iter().skip(1).map(String::as_str).collect();
        let Some(cmd) = words.first().map(String::as_str) else { return (0, String::new(), None) };
        let reply = match cmd {
            "help" => (0, "Demo mode: a simulated shell on the demo cluster. Try ls -la, cd models, cat /etc/hosts, df -h, du -sh models,\n\
                           nvidia-smi, nvidia-smi --query-gpu=name,temperature.gpu --format=csv, docker ps, systemctl status docker,\n\
                           ping cell2-fabric, free -h, history, clear, exit. `;`, `&&`, `>`, `>>`, and grep, head, tail, wc, sort,\n\
                           uniq and cut (on a file or after a pipe) work. Switch Settings › Data source to Live for a real shell.\n".into()),
            "true" => (0, String::new()),
            "false" => (1, String::new()),
            "pwd" => (0, format!("{}\n", self.cwd)),
            "whoami" => (0, format!("{}\n", if self.container.is_some() { "root" } else { &self.user })),
            "id" => (0, format!("uid=1000({u}) gid=1000({u}) groups=1000({u}),4(adm),27(sudo),110(docker)\n", u = self.user)),
            "cd" => {
                let target = args.first().map(|a| self.abs(a)).unwrap_or_else(|| self.home.clone());
                let mut fs = self.fs.lock().unwrap();
                if fs.is_dir(&self.node, &target) {
                    self.cwd = fs.normalize(&target);
                    (0, String::new())
                } else if fs.exists(&self.node, &target) {
                    (1, format!("-bash: cd: {}: Not a directory\n", args[0]))
                } else {
                    (1, format!("-bash: cd: {}: No such file or directory\n", args.first().unwrap_or(&"")))
                }
            }
            "ls" | "ll" => self.ls(&if cmd == "ll" { vec!["-la"] } else { args.clone() }),
            "cat" => self.cat(&args, None),
            "head" => {
                let n = args.iter().position(|a| *a == "-n").and_then(|i| args.get(i + 1)).and_then(|n| n.parse().ok()).unwrap_or(10);
                let files: Vec<&str> = args.iter().enumerate().filter(|(i, a)| !a.starts_with('-') && args.get(i.wrapping_sub(1)) != Some(&"-n")).map(|(_, a)| *a).collect();
                self.cat(&files, Some(n))
            }
            "du" => {
                let paths: Vec<&str> = args.iter().filter(|a| !a.starts_with('-')).copied().collect();
                let paths = if paths.is_empty() { vec!["."] } else { paths };
                let mut out = String::new();
                for p in paths {
                    let abs = if p == "." { self.cwd.clone() } else { self.abs(p) };
                    match self.fs.lock().unwrap().size(&self.node, &abs) {
                        Ok(b) => out += &format!("{}\t{p}\n", if args.iter().any(|a| a.contains('h')) { human(b) } else { (b / 1024).to_string() }),
                        Err(_) => out += &format!("du: cannot access '{p}': No such file or directory\n"),
                    }
                }
                (0, out)
            }
            "touch" | "mkdir" => {
                let mut out = String::new();
                for a in args.iter().filter(|a| !a.starts_with('-')) {
                    let p = self.abs(a);
                    let mut fs = self.fs.lock().unwrap();
                    let r = if cmd == "mkdir" { fs.mkdir(&self.node, &p) } else if fs.exists(&self.node, &p) { Ok(()) } else { fs.write_text(&self.node, &p, "", true) };
                    if let Err(e) = r {
                        out += &format!("{cmd}: cannot {} '{a}': {}\n", if cmd == "mkdir" { "create directory" } else { "touch" }, e.rsplit(": ").next().unwrap_or(&e));
                    }
                }
                (if out.is_empty() { 0 } else { 1 }, out)
            }
            "rm" => {
                let mut out = String::new();
                for a in args.iter().filter(|a| !a.starts_with('-')) {
                    if let Err(e) = self.fs.lock().unwrap().remove(&self.node, &self.abs(a)) {
                        out += &format!("rm: cannot remove '{a}': {}\n", e.rsplit(": ").next().unwrap_or(&e));
                    }
                }
                (if out.is_empty() { 0 } else { 1 }, out)
            }
            "echo" => (0, format!("{}\n", args.join(" "))),
            "history" => (0, self.history.iter().enumerate().map(|(i, h)| format!("{:>5}  {h}\n", i + 1)).collect()),
            "clear" => (0, "\x1b[2J\x1b[3J\x1b[H".into()),
            "sleep" => {
                let left = args.first().and_then(|s| s.parse::<f64>().ok()).unwrap_or(1.0).ceil() as u32;
                return (0, String::new(), Some(Job::Sleep { left }));
            }
            "ping" => {
                let count = args.iter().position(|a| *a == "-c").and_then(|i| args.get(i + 1)).and_then(|n| n.parse().ok());
                let Some(host) = args.iter().enumerate().find(|(i, a)| !a.starts_with('-') && args.get(i.wrapping_sub(1)) != Some(&"-c")).map(|(_, a)| a.to_string()) else {
                    return (2, "ping: usage error: Destination address required\n".into(), None);
                };
                let hosts = self.fs.lock().unwrap().read(&self.node, "/etc/hosts").map(|f| f.content).unwrap_or_default();
                let ip = if host.chars().all(|c| c.is_ascii_digit() || c == '.') {
                    Some(host.clone())
                } else {
                    hosts.lines().filter(|l| !l.trim_start().starts_with('#')).find_map(|l| {
                        let mut w = l.split_whitespace();
                        let ip = w.next()?;
                        w.any(|n| n == host).then(|| ip.to_string())
                    })
                };
                let Some(ip) = ip else { return (2, format!("ping: {host}: Name or service not known\n"), None) };
                let out = format!("PING {host} ({ip}) 56(84) bytes of data.\n");
                return (0, out, Some(Job::Ping { host, ip, seq: 0, count }));
            }
            // The text filters read the files named on the line too: `grep cell /etc/hosts`, `wc -l notes.txt`.
            "grep" | "tail" | "wc" | "sort" | "uniq" | "cut" if self.container.is_none() => {
                let words = shell_words(line);
                let operands: Vec<&str> = words.iter().skip(1).map(String::as_str).filter(|a| !a.starts_with('-')).collect();
                // grep's first operand is its pattern; an option's value (cut -f 2, tail -n 5) is never a file here.
                let files: Vec<&str> = {
                    let mut fs = self.fs.lock().unwrap();
                    operands.iter().skip(usize::from(cmd == "grep")).copied().filter(|a| fs.exists(&self.node, &self.abs(a))).collect()
                };
                if files.is_empty() {
                    (2, format!("{cmd}: reads a file or a pipe in demo mode, e.g. {cmd} … /etc/hosts or cat /etc/hosts | {cmd} …\n"))
                } else {
                    let (code, text) = self.cat(&files, None);
                    // The filter without its file operands, re-quoted so `cut -d ' '` keeps its space.
                    let filter: Vec<String> = words
                        .iter()
                        .filter(|w| !files.contains(&w.as_str()))
                        .map(|w| if w.is_empty() || w.contains(char::is_whitespace) { format!("'{w}'") } else { w.clone() })
                        .collect();
                    let (c, mut out) = pipe_filter(&filter.join(" "), &text);
                    if cmd == "wc" && files.len() == 1 {
                        out = format!("{} {}\n", out.trim_end(), files[0]);
                    }
                    (if code != 0 { 2 } else { c }, out)
                }
            }
            "exit" | "logout" => (0, String::new()),
            "nvidia-smi" if args.iter().any(|a| a.starts_with("--query-gpu=")) => self.gpu_query(&args),
            _ if self.container.is_some() => match cmd {
                "nvidia-smi" | "python3" | "vllm" => {
                    let (code, out, err) = self.fleet(line);
                    (code, if code == 127 { format!("{cmd}: run it in a shell on the node in Demo mode\n") } else { out + &err })
                }
                _ => (127, format!("bash: {cmd}: command not found\n")),
            },
            _ => {
                let (code, out, err) = self.fleet(line);
                if code == 127 && err.contains("no simulated output") {
                    (127, format!("{cmd}: command not found (demo mode: try `help`)\n"))
                } else {
                    (code, out + &err)
                }
            }
        };
        (reply.0, reply.1, None)
    }

    /// A whole line: `a; b && c | grep x > file`.
    fn run(&mut self, line: &str) -> (String, Option<Job>, bool) {
        let mut out = String::new();
        let mut code = 0u32;
        for (i, part) in line.split(';').enumerate() {
            let _ = i;
            for (j, step) in part.split("&&").enumerate() {
                if j > 0 && code != 0 {
                    break;
                }
                let mut step = step.trim().to_string();
                if step.is_empty() {
                    continue;
                }
                if step == "exit" || step == "logout" {
                    out += "logout\n";
                    return (out, None, true);
                }
                // Output redirection.
                let mut redirect = None;
                if let Some(at) = step.find(">>").or_else(|| step.find('>')) {
                    let append = step[at..].starts_with(">>");
                    let file = step[at + if append { 2 } else { 1 }..].trim().to_string();
                    step = step[..at].trim().to_string();
                    redirect = Some((file, append));
                }
                let mut stages = step.split('|').map(str::trim);
                let first = stages.next().unwrap_or_default().to_string();
                let (c, mut text, job) = self.simple(&first);
                code = c;
                self.status = c;
                if job.is_some() {
                    return (out + &text, job, false);
                }
                for filter in stages {
                    let (c, t) = pipe_filter(filter, &text);
                    code = c;
                    self.status = c;
                    text = t;
                    if c == 127 {
                        break;
                    }
                }
                if let Some((file, append)) = redirect {
                    let path = self.abs(&file);
                    if let Err(e) = self.fs.lock().unwrap().write_text(&self.node, &path, &text, append) {
                        text = format!("-bash: {file}: {}\n", e.rsplit(": ").next().unwrap_or(&e));
                        code = 1;
                    } else {
                        text.clear();
                    }
                }
                out += &text;
            }
        }
        self.status = code;
        (out, None, false)
    }

    fn node_online(&self) -> bool {
        self.fleet("hostname").0 != 255
    }

    /// `nvidia-smi --query-gpu=a,b --format=csv[,noheader][,nounits]`, from the simulated GPU:
    /// the fields asked for, in that order, as the GB10 reports them (no dedicated memory, no
    /// power limit: [N/A]).
    fn gpu_query(&self, args: &[&str]) -> (u32, String) {
        let fields: Vec<&str> = args.iter().find_map(|a| a.strip_prefix("--query-gpu=")).unwrap_or("").split(',').map(str::trim).filter(|f| !f.is_empty()).collect();
        let format = args.iter().find_map(|a| a.strip_prefix("--format=")).unwrap_or("");
        if !format.split(',').any(|f| f == "csv") {
            return (2, "Option --format must be csv (with noheader and nounits if wanted).\n".into());
        }
        let (header, units) = (!format.contains("noheader"), !format.contains("nounits"));
        let snap = self.cluster.lock().unwrap().snapshot();
        let Some(n) = snap.nodes.iter().find(|n| n.id == self.node) else { return (9, "No devices were found\n".into()) };
        let g = &n.gpu;
        let mut names = Vec::new();
        let mut values = Vec::new();
        for f in &fields {
            let (unit, value): (&str, String) = match *f {
                "name" | "gpu_name" => ("", "NVIDIA GB10".into()),
                "index" => ("", "0".into()),
                "driver_version" => ("", n.system.driver.clone()),
                "uuid" => ("", format!("GPU-6e2f{:04x}-91c4-4b1a-a8d2-{:012x}", n.id.len() * 977, n.id.bytes().map(u64::from).sum::<u64>() * 1_000_003)),
                "utilization.gpu" => ("%", format!("{:.0}", g.util_pct)),
                "temperature.gpu" => ("", format!("{:.0}", g.temp_c)),
                "power.draw" => ("W", format!("{:.2}", g.power_w)),
                "clocks.sm" | "clocks.current.sm" => ("MHz", format!("{}", g.sm_clock_mhz)),
                "clocks.max.sm" => ("MHz", g.clock_max_mhz.map_or_else(|| "[N/A]".into(), |m| m.to_string())),
                "pstate" => ("", if g.util_pct > 5.0 { "P0".into() } else { "P8".into() }),
                "power.limit" | "enforced.power.limit" | "memory.used" | "memory.total" | "memory.free" | "fan.speed" => ("", "[N/A]".into()),
                other => return (2, format!("Field \"{other}\" is not a valid field to query.\n")),
            };
            names.push(if units && !unit.is_empty() { format!("{f} [{unit}]") } else { f.to_string() });
            values.push(if units && !unit.is_empty() && value != "[N/A]" { format!("{value} {unit}") } else { value });
        }
        let mut out = String::new();
        if header {
            out += &format!("{}\n", names.join(", "));
        }
        out += &format!("{}\n", values.join(", "));
        (0, out)
    }
}

/// Words as a shell splits them: quotes group (and are removed), so `cut -d ' '` gets a space.
fn shell_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut cur, mut quote, mut any) = (String::new(), None::<char>, false);
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() || any {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            (None, c) => cur.push(c),
        }
    }
    if !cur.is_empty() || any {
        out.push(cur);
    }
    out
}

/// One stage after a `|`: grep, head, tail, wc, sort, uniq or cut, as GNU coreutils do them.
fn pipe_filter(filter: &str, text: &str) -> (u32, String) {
    let w = shell_words(filter);
    let w: Vec<&str> = w.iter().map(String::as_str).collect();
    let lines: Vec<&str> = text.lines().collect();
    let join = |v: Vec<String>| -> String { v.into_iter().map(|l| format!("{l}\n")).collect() };
    let flag = |c: char| w.iter().skip(1).any(|a| a.starts_with('-') && !a.starts_with("--") && a.contains(c));
    // -n N, -N, or -nN
    let count = |default: usize| -> usize {
        for (i, a) in w.iter().enumerate().skip(1) {
            if *a == "-n" {
                return w.get(i + 1).and_then(|n| n.trim_start_matches('+').parse().ok()).unwrap_or(default);
            }
            if let Some(n) = a.strip_prefix("-n").or_else(|| a.strip_prefix('-')).and_then(|n| n.parse().ok()) {
                return n;
            }
        }
        default
    };
    match w.first().copied().unwrap_or("") {
        "grep" => {
            let pat = w.iter().skip(1).find(|a| !a.starts_with('-')).copied().unwrap_or("");
            let (invert, nocase) = (flag('v'), flag('i'));
            let hit = |l: &str| if nocase { l.to_lowercase().contains(&pat.to_lowercase()) } else { l.contains(pat) };
            let kept: Vec<String> = lines.iter().filter(|l| hit(l) != invert).map(|l| l.to_string()).collect();
            let code = if kept.is_empty() { 1 } else { 0 };
            if flag('c') {
                return (code, format!("{}\n", kept.len()));
            }
            (code, join(kept))
        }
        "head" => (0, join(lines.iter().take(count(10)).map(|l| l.to_string()).collect())),
        "tail" => {
            let n = count(10);
            (0, join(lines.iter().skip(lines.len().saturating_sub(n)).map(|l| l.to_string()).collect()))
        }
        "wc" => {
            let (l, words, c) = (lines.len(), text.split_whitespace().count(), text.len());
            let only: Vec<String> = [('l', l), ('w', words), ('c', c)].iter().filter(|(f, _)| flag(*f)).map(|(_, v)| v.to_string()).collect();
            (0, if only.is_empty() { format!("{l:>7} {words:>7} {c:>7}\n") } else { format!("{}\n", only.join(" ")) })
        }
        "sort" => {
            let mut v: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
            if flag('n') {
                let num = |s: &str| s.split_whitespace().next().and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
                v.sort_by(|a, b| num(a).partial_cmp(&num(b)).unwrap_or(std::cmp::Ordering::Equal));
            } else {
                v.sort();
            }
            if flag('r') {
                v.reverse();
            }
            if flag('u') {
                v.dedup();
            }
            (0, join(v))
        }
        "uniq" => {
            let mut out: Vec<(usize, String)> = Vec::new();
            for l in &lines {
                match out.last_mut() {
                    Some((n, last)) if last == l => *n += 1,
                    _ => out.push((1, l.to_string())),
                }
            }
            (0, join(out.into_iter().map(|(n, l)| if flag('c') { format!("{n:>7} {l}") } else { l }).collect()))
        }
        "cut" => {
            let arg = |short: &str| -> Option<String> {
                w.iter().enumerate().skip(1).find_map(|(i, a)| match a.strip_prefix(short) {
                    Some("") => w.get(i + 1).map(|s| s.to_string()),
                    Some(rest) => Some(rest.to_string()),
                    None => None,
                })
            };
            let delim = arg("-d").and_then(|d| d.chars().next()).unwrap_or('\t');
            let Some(fields) = arg("-f") else { return (1, "cut: you must specify a list of bytes, characters, or fields\n".into()) };
            let pick: Vec<usize> = fields.split(',').filter_map(|f| f.parse::<usize>().ok()).filter(|n| *n > 0).collect();
            (0, join(lines.iter().map(|l| {
                let parts: Vec<&str> = l.split(delim).collect();
                if parts.len() == 1 { l.to_string() } else { pick.iter().filter_map(|n| parts.get(n - 1).copied()).collect::<Vec<_>>().join(&delim.to_string()) }
            }).collect()))
        }
        other => (127, format!("{other}: not in the demo shell (pipes work into grep, head, tail, wc, sort, uniq and cut)\n")),
    }
}

/// A simulated log stream line for a container (vLLM's engine stats) or a unit's journal.
fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// One line a follower prints at Unix time `at` (the nodes' clocks run on UTC, like the
/// backlog `journal()` writes): vLLM's engine stats, or a running unit's journal.
fn follow_line(host: &str, target: &Target, n: u64, at: u64) -> String {
    let (_, mo, d, h, m, s, _) = crate::demo::ops::civil(at);
    match target {
        Target::ContainerLogs { .. } => format!(
            "INFO {mo:02}-{d:02} {h:02}:{m:02}:{s:02} [loggers.py:123] Engine 000: Avg prompt throughput: {:.1} tokens/s, Avg generation throughput: {:.1} tokens/s, Running: {} reqs, Waiting: 0 reqs, GPU KV cache usage: {:.1}%, Prefix cache hit rate: {:.1}%\n",
            380.0 + (n % 7) as f64 * 21.0,
            31.0 + (n % 5) as f64,
            n % 4,
            12.0 + (n % 9) as f64,
            41.0 + (n % 11) as f64
        ),
        Target::Journal { unit, .. } => {
            const MON: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
            let msgs = ["Started session-%.scope - Session % of User sparkuser.", "Reloading requested from client PID %.", "Finished %.timer.", "Health check passed."];
            let msg = msgs[(n % msgs.len() as u64) as usize].replace('%', &(n % 97 + 3).to_string());
            format!("{} {d:02} {h:02}:{m:02}:{s:02} {host} {}[{}]: {msg}\n", MON[(mo as usize).saturating_sub(1) % 12], unit.trim_end_matches(".service"), 1000 + n % 7)
        }
        _ => String::new(),
    }
}

/// The requests a serving container answered in second `t` (vLLM's uvicorn access log), at
/// the pace its engine stats imply: SparkBox's probe from this Mac every 5 s (/health, then the
/// model list), and a chat completion about once a minute from Open WebUI on cell1 (a 70B
/// stream runs for minutes, so few finish).
fn access_lines(t: u64) -> String {
    let line = |who: &str, port: u64, verb: &str, path: &str| format!("INFO:     {who}:{port} - \"{verb} {path} HTTP/1.1\" 200 OK\n");
    let mut out = String::new();
    if t % 5 == 0 {
        let port = 50_000 + (t / 5 * 2) % 10_000;
        out += &line("192.168.50.20", port, "GET", "/health");
        out += &line("192.168.50.20", port + 1, "GET", "/v1/models");
    }
    if (t.wrapping_mul(2_654_435_761) >> 7) % 53 == 0 {
        out += &line("192.168.50.51", 40_000 + t % 20_000, "POST", "/v1/chat/completions");
    }
    out
}

/// The stats lines a serving container's engine wrote between two Unix times: the same
/// numbers its endpoint card showed (models::demo_engine_log). Empty for other containers,
/// and for the time before Models started watching it.
fn engine_lines(node: &str, target: &Target, from_s: u64, to_s: u64) -> Vec<(u64, String)> {
    match target {
        Target::ContainerLogs { container, .. } => crate::models::demo_engine_log(node, container, from_s, to_s),
        _ => Vec::new(),
    }
}

/// `docker logs --since <span>` of a serving container: the engine's stats every 10 s and every
/// request in between, oldest first (a busy day is tens of MB, as on a real node). The stats
/// are the engine's own from the moment Models watched it, made up before that.
fn container_backlog(node: &str, host: &str, target: &Target, span_s: u64, now: u64) -> String {
    let from = now.saturating_sub(span_s);
    let engine = engine_lines(node, target, from, now);
    let first = engine.first().map(|(t, _)| *t).unwrap_or(u64::MAX);
    let mut next = engine.iter().peekable();
    let mut out = String::new();
    for t in from..now {
        while let Some((_, line)) = next.next_if(|(at, _)| *at <= t) {
            out.push_str(line);
            out.push('\n');
        }
        if t % 10 == 0 && t < first {
            out.push_str(&follow_line(host, target, t / 10, t));
        }
        out.push_str(&access_lines(t));
    }
    out
}

pub(crate) async fn run(
    node: &str,
    target: &Target,
    _cols: u32,
    rx: &mut mpsc::Receiver<Input>,
    out: &mpsc::Sender<Out>,
    cluster: Arc<Mutex<DemoCluster>>,
    fs: Arc<Mutex<DemoFs>>,
) -> (Option<u32>, Option<String>) {
    let send = |s: String| out.send(Out::Data(crlf(&s).into_bytes()));
    let online = || cluster.lock().unwrap().fleet_reply(node, "hostname").0 != 255;
    if !online() {
        let ip = crate::inventory::Inventory::example().nodes.iter().find(|n| n.id == node).and_then(|n| n.ip.clone()).unwrap_or_else(|| node.into());
        return (Some(255), Some(format!("ssh: connect to host {ip} port 22: Operation timed out")));
    }
    let mut check = tokio::time::interval(Duration::from_secs(2));
    check.tick().await;
    match target {
        Target::ContainerLogs { since, .. } | Target::Journal { since, .. } => {
            // The backlog: the unit's journal, or the engine's recent output, as far back as asked.
            let lines = match since.as_deref() {
                Some("10m") => 40,
                Some("1h") | Some("boot") => 120,
                Some(_) => 400,
                None => 30,
            };
            let span_s: Option<u64> = match since.as_deref() {
                Some("10m") => Some(600),
                Some("1h") => Some(3_600),
                Some("6h") => Some(6 * 3_600),
                Some("24h") | Some("boot") => Some(24 * 3_600),
                _ => None,
            };
            let host = cluster.lock().unwrap().fleet_reply(node, "hostname").1.trim().to_string();
            let now = unix_now();
            // A container that has stopped: `docker logs` prints what it wrote and ends (exit 0),
            // as on a real node; with --since, only what it wrote in that span (often nothing).
            if let Target::ContainerLogs { container, .. } = target {
                let stopped = cluster.lock().unwrap().container_log(node, container);
                if let Some((text, finished)) = stopped {
                    if span_s.is_none_or(|span| finished >= now.saturating_sub(span)) {
                        let tail: Vec<&str> = text.lines().collect();
                        let _ = send(tail[tail.len().saturating_sub(500)..].iter().map(|l| format!("{l}\n")).collect()).await;
                    }
                    return (Some(0), None);
                }
            }
            if let Target::Journal { unit, .. } = target {
                let backlog = cluster.lock().unwrap().journal(node, unit, lines).unwrap_or_default();
                let _ = send(backlog).await;
            } else if let Some(span) = span_s {
                let _ = send(container_backlog(node, &host, target, span, now)).await;
            } else {
                // `--tail 500`: the last stats lines and requests, ending now.
                let all = container_backlog(node, &host, target, 600, now);
                let tail: Vec<&str> = all.lines().collect();
                let _ = send(tail[tail.len().saturating_sub(500)..].iter().map(|l| format!("{l}\n")).collect()).await;
            }
            // A unit that isn't running writes nothing more; journalctl -f just waits.
            let running = |cluster: &Arc<Mutex<DemoCluster>>| match target {
                Target::Journal { unit, .. } => cluster.lock().unwrap().services(node).is_ok_and(|us| us.iter().any(|u| (u.unit == *unit || format!("{}.service", u.unit) == *unit) && u.active == "active")),
                _ => true,
            };
            let mut n = 0u64;
            let mut stats_at = 0u64;
            let mut engine_at = now.saturating_sub(1);
            let mut tick = tokio::time::interval(Duration::from_millis(1500));
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        n += 1;
                        if !running(&cluster) {
                            continue;
                        }
                        let now = unix_now();
                        let line = if matches!(target, Target::ContainerLogs { .. }) {
                            // Requests as they're answered; vLLM's engine stats only every 10 s, as in
                            // the backlog: the engine's own lines when Models watches it.
                            let mut l = access_lines(now);
                            let engine = engine_lines(node, target, engine_at + 1, now);
                            if let Some((t, _)) = engine.last() {
                                engine_at = *t;
                                l.extend(engine.iter().map(|(_, line)| format!("{line}\n")));
                            } else if now / 10 != stats_at && engine_lines(node, target, now.saturating_sub(60), now).is_empty() {
                                stats_at = now / 10;
                                l += &follow_line(&host, target, stats_at, now);
                            }
                            l
                        } else {
                            follow_line(&host, target, n, now)
                        };
                        if line.is_empty() {
                            continue;
                        }
                        if send(line).await.is_err() {
                            return (None, None);
                        }
                    }
                    _ = check.tick() => {
                        if !online() {
                            return (None, Some(format!("Connection to {node} closed: the node stopped answering")));
                        }
                    }
                    input = rx.recv() => match input {
                        Some(Input::Close) | None => return (None, None),
                        _ => {}
                    },
                }
            }
        }
        _ => run_shell(node, target, rx, out, cluster, fs, &mut check).await,
    }
}

async fn run_shell(
    node: &str,
    target: &Target,
    rx: &mut mpsc::Receiver<Input>,
    out: &mpsc::Sender<Out>,
    cluster: Arc<Mutex<DemoCluster>>,
    fs: Arc<Mutex<DemoFs>>,
    check: &mut tokio::time::Interval,
) -> (Option<u32>, Option<String>) {
    let send = |s: &str| out.send(Out::Data(crlf(s).into_bytes()));
    let user = world::user();
    let home = fs.lock().unwrap().home();
    let container = match target {
        Target::ContainerShell { container } => Some(container.chars().take(12).collect::<String>()),
        _ => None,
    };
    let start = match target {
        Target::ShellIn { cwd: dir } => {
            let mut f = fs.lock().unwrap();
            let p = f.normalize(dir);
            if f.is_dir(node, &p) { p } else { home.clone() }
        }
        Target::ContainerShell { .. } => "/workspace".into(),
        _ => home.clone(),
    };
    let mut shell = Shell { node: node.into(), user, home, cwd: start, container, status: 0, history: Vec::new(), cluster, fs };
    if shell.container.is_none() {
        let _ = send(&format!(
            "Welcome to {} on {node} (demo mode: type help)\n\n * Kernel {}, NVIDIA driver {}, CUDA {}\n\nLast login: {} from 192.168.50.20\n",
            world::OS,
            world::KERNEL,
            world::DRIVER,
            world::CUDA,
            last_login()
        ))
        .await;
    }
    let _ = send(&shell.prompt()).await;
    let mut line = String::new();
    let mut recall: Option<usize> = None;
    let mut job: Option<Job> = None;
    let mut esc = String::new();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = tick.tick(), if job.is_some() => {
                let mut finished = false;
                match job.as_mut().unwrap() {
                    Job::Sleep { left } => {
                        *left = left.saturating_sub(1);
                        finished = *left == 0;
                    }
                    Job::Ping { host, ip, seq, count } => {
                        *seq += 1;
                        let rtt = if ip.starts_with("192.168.10") { 0.041 + (*seq % 7) as f64 * 0.006 } else { 0.31 + (*seq % 5) as f64 * 0.05 };
                        let _ = send(&format!("64 bytes from {host} ({ip}): icmp_seq={seq} ttl=64 time={rtt:.3} ms\n")).await;
                        if count.is_some_and(|c| *seq >= c) {
                            let _ = send(&format!("\n--- {host} ping statistics ---\n{seq} packets transmitted, {seq} received, 0% packet loss, time {}ms\n", seq.saturating_sub(1) * 1000)).await;
                            finished = true;
                        }
                    }
                }
                if finished {
                    job = None;
                    let _ = send(&shell.prompt()).await;
                }
            }
            _ = check.tick() => {
                if !shell.node_online() {
                    let _ = send("\n").await;
                    return (None, Some(format!("Connection to {node} closed: the node stopped answering")));
                }
            }
            input = rx.recv() => {
                let bytes = match input {
                    Some(Input::Data(b)) => b,
                    Some(Input::Resize(..)) => continue,
                    Some(Input::Close) | None => return (None, None),
                };
                for ch in String::from_utf8_lossy(&bytes).chars() {
                    if job.is_some() {
                        if ch == '\u{3}' {
                            if let Some(Job::Ping { host, seq, .. }) = &job {
                                let _ = send(&format!("^C\n--- {host} ping statistics ---\n{seq} packets transmitted, {seq} received, 0% packet loss\n")).await;
                            } else {
                                let _ = send("^C\n").await;
                            }
                            shell.status = 130;
                            job = None;
                            let _ = send(&shell.prompt()).await;
                        }
                        continue;
                    }
                    if !esc.is_empty() || ch == '\u{1b}' {
                        esc.push(ch);
                        if esc.len() >= 3 {
                            let seq = std::mem::take(&mut esc);
                            let step: Option<isize> = match seq.as_str() { "\u{1b}[A" => Some(-1), "\u{1b}[B" => Some(1), _ => None };
                            if let Some(step) = step.filter(|_| !shell.history.is_empty()) {
                                let n = shell.history.len() as isize;
                                let next = recall.map(|r| r as isize + step).unwrap_or(if step < 0 { n - 1 } else { n });
                                let next = next.clamp(0, n);
                                let _ = send(&"\u{8} \u{8}".repeat(line.chars().count())).await;
                                line = shell.history.get(next as usize).cloned().unwrap_or_default();
                                recall = (next < n).then_some(next as usize);
                                let _ = send(&line).await;
                            }
                        }
                        continue;
                    }
                    match ch {
                        '\r' | '\n' => {
                            let _ = send("\n").await;
                            let cmd = line.trim().to_string();
                            line.clear();
                            recall = None;
                            if !cmd.is_empty() {
                                shell.history.push(cmd.clone());
                            }
                            let (text, started, exit) = shell.run(&cmd);
                            if !text.is_empty() {
                                let _ = send(&text).await;
                            }
                            if exit {
                                return (Some(0), None);
                            }
                            job = started;
                            if job.is_none() {
                                let _ = send(&shell.prompt()).await;
                            }
                        }
                        '\u{7f}' | '\u{8}' => {
                            if line.pop().is_some() {
                                let _ = send("\u{8} \u{8}").await;
                            } else {
                                let _ = send("\u{7}").await; // readline rings the bell at the start of the line
                            }
                        }
                        '\u{3}' => {
                            line.clear();
                            shell.status = 130;
                            let _ = send("^C\n").await;
                            let _ = send(&shell.prompt()).await;
                        }
                        '\u{c}' => {
                            let _ = send("\x1b[2J\x1b[H").await;
                            let _ = send(&shell.prompt()).await;
                            let _ = send(&line).await;
                        }
                        '\u{4}' if line.is_empty() => {
                            let _ = send("logout\n").await;
                            return (Some(0), None);
                        }
                        '\t' => {
                            let _ = send("\u{7}").await;
                        }
                        c if !c.is_control() => {
                            line.push(c);
                            let _ = send(&c.to_string()).await;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod follow_tests {
    use super::*;

    #[test]
    fn followers_print_the_time_and_host_they_are_given() {
        // 2026-10-28 16:42:49 UTC
        let at = 1_793_205_769;
        let logs = follow_line("cell1-example", &Target::ContainerLogs { container: "vllm-head".into(), since: None }, 1, at);
        assert!(logs.starts_with("INFO 10-28 16:42:49 "), "{logs}");
        let journal = follow_line("cell1-example", &Target::Journal { unit: "docker.service".into(), since: None }, 1, at);
        assert!(journal.starts_with("Oct 28 16:42:49 cell1-example docker["), "{journal}");
    }

    #[test]
    fn a_serving_containers_logs_carry_the_engines_own_stats() {
        let at = 1_793_205_760;
        let target = Target::ContainerLogs { container: "vllm-term-test".into(), since: Some("1m".into()) };
        for k in 0..3 {
            crate::models::demo_engine_log_push("cell2", "vllm-term-test", at + k * 10, &format!("INFO engine line {k}"));
        }
        let log = container_backlog("cell2", "cell2-example", &target, 60, at + 30);
        let stats: Vec<&str> = log.lines().filter(|l| l.contains("Engine 000") || l.contains("engine line")).collect();
        // Made up before Models watched it, the engine's own lines after: never both at once.
        assert_eq!(&stats[stats.len() - 3..], ["INFO engine line 0", "INFO engine line 1", "INFO engine line 2"], "{stats:?}");
        assert!(stats[..stats.len() - 3].iter().all(|l| l.contains("Engine 000")), "{stats:?}");
        assert_eq!(stats.len(), 6, "every 10 s: {stats:?}");
        // Another container keeps the made-up ones.
        let other = container_backlog("cell2", "cell2-example", &Target::ContainerLogs { container: "sglang".into(), since: None }, 60, at + 30);
        assert_eq!(other.lines().filter(|l| l.contains("Engine 000")).count(), 6);
    }

    /// Runs a log tab against the as-found world until it ends: (what it printed, how it ended).
    async fn logs_of(container: &str, since: Option<&str>) -> (String, Option<u32>, Option<String>) {
        let cluster = Arc::new(Mutex::new(DemoCluster::as_found()));
        let fs = Arc::new(Mutex::new(DemoFs::as_found()));
        let (_tx, mut rx) = mpsc::channel(4);
        let (out, mut got) = mpsc::channel(64);
        let target = Target::ContainerLogs { container: container.into(), since: since.map(Into::into) };
        let (code, message) = tokio::time::timeout(Duration::from_secs(10), run("cell1", &target, 80, &mut rx, &out, cluster, fs)).await.expect("docker logs of a stopped container ends by itself");
        drop(out);
        let mut text = Vec::new();
        while let Some(o) = got.recv().await {
            if let Out::Data(d) = o {
                text.extend(d);
            }
        }
        (String::from_utf8_lossy(&text).into_owned(), code, message)
    }

    #[tokio::test]
    async fn a_stopped_containers_logs_print_its_log_and_end_with_code_0() {
        let (text, code, message) = logs_of("glm53", None).await;
        assert_eq!((code, message), (Some(0), None), "docker logs exits 0 once it has printed everything");
        assert!(text.contains("ValueError: No valid attention backend found") || text.contains("kv_cache_dtype=bfloat16"), "the real log");
        assert!(text.trim_end().ends_with("Failed core proc(s): {}"), "and it ends where the engine gave up");
        assert!(!text.contains("Engine 000: Avg prompt throughput"), "no made-up stats for a container that never served");
        // Asked for the last 10 minutes of a container that stopped two days ago: nothing, then the end.
        let (text, code, _) = logs_of("glm53", Some("10m")).await;
        assert_eq!((text.as_str(), code), ("", Some(0)));
    }
}
