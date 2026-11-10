//! What a command does, in the fence's terms (P1.1): it only reads, it keeps SparkBox's own
//! books on the node (the controller lease), or it changes the node in one named class that a
//! fence can allow. Decided from the command's exact shape, the way SparkBox composes it, never
//! from what the command says it is; anything unrecognised that isn't a read is `other`, which
//! only an `any` fence admits.
//!
//! Each change names its items where the class has them (a unit, a container's name or id, a
//! package), so a fence's `only` list can narrow it ("service" only for earlyoom.service).
//!
//! A `$(…)`, backtick or `<(…)` the shell would expand runs a command no shape's words show, so a
//! command with one is a read only when the classifier says so, and otherwise `other`.
//!
//! The shapes read commands the way the nodes' /bin/sh (dash 0.5.12) does, and `segments` refuses
//! what it doesn't model (T2 blocker 2): such a command is `other`, or a read only through the
//! classifier. Files' rename and Updates' Cancel (C-X), whose `if … fi` it refuses, are recognised
//! by their exact text instead. Check (d) still looks through every word for container names.

use serde::{Deserialize, Serialize};

use super::exit::ExecKind;
use crate::model::Container;
use crate::ops::classify::{self, classify, escalates, Kind};

/// A class of change a fence allows. The whole P1 list, so later rounds only add call-site tags.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FenceClass {
    SudoProbe,
    RootHelper,
    Service,
    ContainerOwner,
    ContainerTest,
    ProcessTest,
    FilesTest,
    Fleet,
    Shell,
    PowerOne,
    AptRefresh,
    AptOne,
    AptHold,
    Wifi,
    ModelLoadOne,
    /// A rank of a load on several nodes (a head or a worker). A fence naming it also admits
    /// `model-load-one` (`wider`), so one fence serves a session's loads of either size.
    ModelLoad,
    ModelUnload,
    DropCaches,
    RdpRepair,
    /// Everything, `other` included (release-instance fences only).
    Any,
    /// A change SparkBox doesn't name: only `any` admits it. Never opened on its own.
    Other,
}

impl FenceClass {
    /// Every class a fence can name.
    #[cfg(test)]
    pub const OPENABLE: [FenceClass; 20] = [
        FenceClass::SudoProbe,
        FenceClass::RootHelper,
        FenceClass::Service,
        FenceClass::ContainerOwner,
        FenceClass::Fleet,
        FenceClass::Shell,
        FenceClass::PowerOne,
        FenceClass::AptRefresh,
        FenceClass::AptOne,
        FenceClass::AptHold,
        FenceClass::Wifi,
        FenceClass::ModelLoadOne,
        FenceClass::ModelLoad,
        FenceClass::ModelUnload,
        FenceClass::DropCaches,
        FenceClass::RdpRepair,
        FenceClass::ContainerTest,
        FenceClass::ProcessTest,
        FenceClass::FilesTest,
        FenceClass::Any,
    ];

    /// The kebab-case id, as the fence record and the activity log write it.
    pub fn id(self) -> &'static str {
        match self {
            FenceClass::SudoProbe => "sudo-probe",
            FenceClass::RootHelper => "root-helper",
            FenceClass::Service => "service",
            FenceClass::ContainerOwner => "container-owner",
            FenceClass::ContainerTest => "container-test",
            FenceClass::ProcessTest => "process-test",
            FenceClass::FilesTest => "files-test",
            FenceClass::Fleet => "fleet",
            FenceClass::Shell => "shell",
            FenceClass::PowerOne => "power-one",
            FenceClass::AptRefresh => "apt-refresh",
            FenceClass::AptOne => "apt-one",
            FenceClass::AptHold => "apt-hold",
            FenceClass::Wifi => "wifi",
            FenceClass::ModelLoadOne => "model-load-one",
            FenceClass::ModelLoad => "model-load",
            FenceClass::ModelUnload => "model-unload",
            FenceClass::DropCaches => "drop-caches",
            FenceClass::RdpRepair => "rdp-repair",
            FenceClass::Any => "any",
            FenceClass::Other => "other",
        }
    }

    #[cfg(test)]
    pub fn parse(id: &str) -> Option<FenceClass> {
        FenceClass::OPENABLE.into_iter().chain([FenceClass::Other]).find(|c| c.id() == id)
    }

    /// What it allows, for "the fence doesn't allow service changes".
    pub fn words(self) -> &'static str {
        match self {
            FenceClass::SudoProbe => "sudo checks",
            FenceClass::RootHelper => "installing SparkBox's root helper",
            FenceClass::Service => "service changes",
            FenceClass::ContainerOwner => "container changes",
            FenceClass::ContainerTest => "test containers",
            FenceClass::ProcessTest => "process signals",
            FenceClass::FilesTest => "file changes",
            FenceClass::Fleet => "Fleet commands",
            FenceClass::Shell => "shells",
            FenceClass::PowerOne => "restarts and shutdowns",
            FenceClass::AptRefresh => "package list refreshes",
            FenceClass::AptOne => "package upgrades",
            FenceClass::AptHold => "package holds",
            FenceClass::Wifi => "Wi-Fi changes",
            FenceClass::ModelLoadOne => "model loads",
            FenceClass::ModelLoad => "model loads on several nodes",
            FenceClass::ModelUnload => "model unloads",
            FenceClass::DropCaches => "page-cache drops",
            FenceClass::RdpRepair => "RDP configuration repairs",
            FenceClass::Any | FenceClass::Other => "this change",
        }
    }

    /// The class a fence may name in this one's place: `model-load` covers a load on one node. It
    /// never goes the other way, so a fence naming only `model-load-one` refuses a head or worker.
    pub fn wider(self) -> Option<FenceClass> {
        match self {
            FenceClass::ModelLoadOne => Some(FenceClass::ModelLoad),
            _ => None,
        }
    }
}

/// One thing a change acts on, under each name it goes by (a container's name and its ids).
pub type Item = Vec<String>;

/// What a command does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shape {
    Read,
    /// SparkBox's own bookkeeping on the node (the controller lease).
    Bookkeeping,
    Change(FenceClass, Vec<Item>),
}

impl Shape {
    fn change(class: FenceClass) -> Shape {
        Shape::Change(class, Vec::new())
    }
}

/// What the shapes need to know besides the command.
#[derive(Clone, Copy, Default)]
pub struct Ctx<'a> {
    /// The node's containers in its latest frame (the sparkbox.deployment label, ids by name).
    pub containers: &'a [Container],
    /// The cluster user's $HOME on the node, when known (paths under ~/sparkbox-test).
    pub home: Option<&'a str>,
    /// Root's copy of sparkjob.py and its SHA-256 (Live::sparkjob_root's command).
    pub root_lib: &'a str,
    pub sparkjob_sha: &'a str,
}

/// sparkjob verbs that only look. `check` only looks with `quick`: without it, it runs the vLLM
/// image in a test container to read its version (`sparkjob_shape`).
const SPARKJOB_READS: [&str; 12] = ["list", "scan", "manifest", "ls", "read", "size", "du", "df", "backups", "summary", "tree", "recipe-import"];
/// sparkjob verbs that change files or jobs.
const SPARKJOB_WRITES: [&str; 12] = ["commit", "trash", "untrash", "rm", "place", "cp", "cleanup", "sweep", "start", "cancel", "delete", "deploy-files"];
pub(crate) const SPARKJOB: &str = "python3 ~/.sparkbox/sparkjob.py ";
/// The SparkBox agent's CLI verbs that only read its state files.
const AGENT_READS: [&str; 4] = ["status", "export", "alerts", "deliveries"];
const DOCKER_VERBS: [&str; 7] = ["start", "stop", "restart", "kill", "rm", "pause", "unpause"];

/// `cmd` without the blanks around it that the shell ignores (space, tab, newline). A carriage
/// return or a non-ASCII space stays: it's part of a word to the shell (`segments` refuses it).
fn blank_trim(cmd: &str) -> &str {
    cmd.trim_matches([' ', '\t', '\n'])
}

/// The command-shape table (see the module notes). `kind` is how Live runs it.
pub fn class_of(kind: ExecKind, cmd: &str, ctx: &Ctx) -> Shape {
    let cmd = unwrap_sh(blank_trim(cmd));
    if kind == ExecKind::Sudo {
        if let Some(shape) = crate::ops::rdp::shape(&cmd) { return shape; }
    }
    // A `$(…)` or backtick the shell expands runs a command of its own that a shape's words
    // don't show (`mkdir -- ~/sparkbox-test/$(rm -rf ~/models)`); a parameter expansion or glob
    // (`${IFS}`, `$HOME`, `*`) splits or grows a word into paths the fence never saw
    // (`chmod -- 000 ~/sparkbox-test/x${IFS}$HOME/victim`). Only the classifier's read tier, which
    // checks each substitution, can still call one a read; anything else is `other`.
    if (substitutes(&cmd) || expands(&cmd)) && !matches!(kind, ExecKind::DirectTcpip | ExecKind::Pty) {
        return substituted(kind, &cmd, ctx);
    }
    match kind {
        // A forward to a port on the node: a web page there can do what a shell does.
        ExecKind::DirectTcpip => Shape::change(FenceClass::Shell),
        ExecKind::Sudo => sudo_shape(&cmd, ctx),
        ExecKind::SudoInput => {
            if !ctx.root_lib.is_empty() && cmd == super::root_install_command(ctx.root_lib, ctx.sparkjob_sha) {
                Shape::change(FenceClass::RootHelper)
            } else if reads(&cmd) {
                Shape::change(FenceClass::SudoProbe)
            } else {
                Shape::change(FenceClass::Other)
            }
        }
        ExecKind::SparkjobRoot => match sparkjob_shape(&cmd, ctx) {
            Shape::Read => Shape::change(FenceClass::SudoProbe),
            _ => Shape::change(FenceClass::Other),
        },
        ExecKind::Sparkjob => sparkjob_shape(&cmd, ctx),
        ExecKind::ContainerAction => docker_verb(&cmd, ctx).unwrap_or_else(|| Shape::change(FenceClass::Other)),
        // No command: a login shell. A follower only reads. Any other read in a terminal takes
        // keys: less's `!`, top's `k`.
        ExecKind::Pty if follower(&cmd) => Shape::Read,
        ExecKind::Pty => Shape::change(FenceClass::Shell),
        ExecKind::ExecIo => match cmd.strip_prefix(SPARKJOB) {
            Some(args) => sparkjob_shape(args, ctx),
            // A copy packs files off the node (a read) or unpacks into a staging folder under
            // ~/sparkbox-test (files-test); anything else over exec_io is a shell.
            None => exec_io_copy_shape(&cmd, ctx).unwrap_or_else(|| if reads(&cmd) { Shape::Read } else { Shape::change(FenceClass::Shell) }),
        },
        ExecKind::Run | ExecKind::Probe | ExecKind::Cancellable | ExecKind::Stream => run_shape(&cmd, ctx),
    }
}

/// A log follower alone, as `term::command_for` writes it: `journalctl -f` or `docker logs -f` with
/// only the options that say what to follow and from where. One command and no redirection: its
/// words hold no `;`, `&`, `|` or newline, so one in `cmd` is the shell's, and a pager's options
/// (`--list-boots`) stay out, because a pager takes keys.
fn follower(cmd: &str) -> bool {
    if cmd.contains([';', '&', '|', '\n']) || !reads(cmd) {
        return false;
    }
    let Some(segs) = segments(cmd) else { return false };
    let [words] = segs.as_slice() else { return false };
    if words.iter().any(|w| redirection(w).is_some()) {
        return false;
    }
    let w: Vec<&str> = words.iter().map(String::as_str).collect();
    let options = match w.as_slice() {
        ["journalctl", "-f", rest @ ..] => classify::known_options(rest, &["-b", "--no-hostname"], &["-n", "--since", "-u"]),
        ["docker", "logs", "-f", rest @ ..] => classify::known_options(rest, &[], &["--since", "--tail"]),
        _ => None,
    };
    options.is_some_and(|names| names.len() <= 1)
}

/// A command with a substitution the shell expands: a read only when the classifier says so; a
/// load's unload (`ids=$(docker ps -aq --filter label=sparkbox.deployment=…)`), root's checked
/// sparkjob.py and its install keep their shapes; anything else is `other` (a shell over exec_io).
fn substituted(kind: ExecKind, cmd: &str, ctx: &Ctx) -> Shape {
    match kind {
        ExecKind::Sudo if root_sparkjob_args(cmd, ctx).is_some() => sudo_shape(cmd, ctx),
        ExecKind::SudoInput if !ctx.root_lib.is_empty() && cmd == super::root_install_command(ctx.root_lib, ctx.sparkjob_sha) => Shape::change(FenceClass::RootHelper),
        // A model load's `docker run` carries only parameter expansions, its `$HOME` mounts, so it
        // reaches here through `expands`, not `substitutes`; its shape is still its class. An
        // unload carries `$(docker ps …)` and `$ids` (`model_unload`). A command substitution
        // (`$(…)`, backtick) runs a command the words don't show, so `docker_run` is never trusted
        // for one (it would re-admit `docker run … $(rm -rf ~/models)`); anything else with an
        // expansion is `other`.
        ExecKind::Run | ExecKind::Probe | ExecKind::Cancellable | ExecKind::Stream => {
            match segments(cmd).and_then(|s| model_unload(&s).or_else(|| (!substitutes(cmd)).then(|| docker_run(&s, cmd)).flatten())) {
                Some(s) => s,
                None if reads(cmd) => Shape::Read,
                None => Shape::change(FenceClass::Other),
            }
        }
        ExecKind::Sudo | ExecKind::SudoInput if reads(cmd) => Shape::change(FenceClass::SudoProbe),
        ExecKind::ExecIo if reads(cmd) => Shape::Read,
        ExecKind::ExecIo => Shape::change(FenceClass::Shell),
        _ => Shape::change(FenceClass::Other),
    }
}

/// A `$(…)`, backtick or `<(…)`/`>(…)` the shell would expand: outside single quotes and not
/// escaped (double quotes still expand them).
fn substitutes(cmd: &str) -> bool {
    let chars: Vec<char> = cmd.chars().collect();
    let mut i = 0;
    let mut double = false;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1,
            '\'' if !double => match chars[i + 1..].iter().position(|&c| c == '\'') {
                Some(n) => i += n + 1,
                None => return true,
            },
            '"' => double = !double,
            '`' => return true,
            '$' if chars.get(i + 1) == Some(&'(') => return true,
            '<' | '>' if !double && chars.get(i + 1) == Some(&'(') => return true,
            _ => {}
        }
        i += 1;
    }
    false
}

/// A parameter expansion (`$HOME`, `${IFS}`, `$1`, `$@`…) outside single quotes, or a glob
/// (`*`, `?`, `[x`) outside any quotes: the shell's word splitting or pathname expansion then runs
/// on a word the fence's shape never saw (`~/sparkbox-test/x${IFS}$HOME/models`). A lone `[` or a
/// `$` before a space isn't one.
fn expands(cmd: &str) -> bool {
    let chars: Vec<char> = cmd.chars().collect();
    let mut i = 0;
    let mut double = false;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1,
            '\'' if !double => match chars[i + 1..].iter().position(|&c| c == '\'') {
                Some(n) => i += n + 1,
                None => return true,
            },
            '"' => double = !double,
            '$' if chars.get(i + 1).is_some_and(|c| c.is_ascii_alphanumeric() || "_{@*#?$!-".contains(*c)) => return true,
            '*' | '?' if !double => return true,
            '[' if !double && chars.get(i + 1).is_some_and(|c| !c.is_whitespace()) => return true,
            _ => {}
        }
        i += 1;
    }
    false
}

/// Only reads: the classifier's read tier (no sudo anywhere), or SparkBox's agent reading its own state.
fn reads(cmd: &str) -> bool {
    if !escalates(cmd) && matches!(classify(cmd), Kind::Read | Kind::Probe) {
        return true;
    }
    // `systemctl is-active sparkbox-agent; test -f …/sparkbox_agent.py && <cli status>`: the
    // agent's CLI reading its own files, and every other part a read on its own.
    let Some(segs) = segments(cmd) else { return false };
    // Quoting a word again for the classifier would turn its `$` into text.
    let built = |s: &[String]| s.iter().any(|w| w.contains('$') || w.contains('`'));
    segs.iter().any(|s| agent_read(s))
        && segs.iter().all(|s| !built(s) && (agent_read(s) || (!escalates(&join_quoted(s)) && matches!(classify(&join_quoted(s)), Kind::Read | Kind::Probe))))
}

/// `SPARKBOX_AGENT_STATE=… SPARKBOX_AGENT_CONFIG=… python3 -I <opt>/sparkbox_agent.py <read verb> …`
fn agent_read(words: &[String]) -> bool {
    if redirects(words) {
        return false;
    }
    let rest: Vec<&str> = words.iter().map(String::as_str).skip_while(|w| w.starts_with("SPARKBOX_AGENT_STATE=") || w.starts_with("SPARKBOX_AGENT_CONFIG=")).collect();
    match rest.as_slice() {
        ["python3", "-I", script, verb, ..] => script.ends_with("/sparkbox_agent.py") && AGENT_READS.contains(verb),
        _ => false,
    }
}

/// Live::sparkjob_root's command, root's verified copy of sparkjob.py then its arguments: the
/// arguments.
fn root_sparkjob_args<'a>(cmd: &'a str, ctx: &Ctx) -> Option<&'a str> {
    if ctx.root_lib.is_empty() {
        return None;
    }
    let f = super::sh_quote(&format!("{}/sparkjob.py", ctx.root_lib));
    let prefix = format!("[ \"$(sha256sum < {f} 2>/dev/null | cut -c1-64)\" = {} ] || exit {}; exec python3 -I {f} ", ctx.sparkjob_sha, super::ROOT_COPY_STALE);
    cmd.strip_prefix(&prefix)
}

fn sudo_shape(cmd: &str, ctx: &Ctx) -> Shape {
    if cmd == "true" {
        return Shape::change(FenceClass::SudoProbe);
    }
    if let Some(args) = root_sparkjob_args(cmd, ctx) {
        return class_of(ExecKind::SparkjobRoot, args, ctx);
    }
    let Some(segs) = segments(cmd) else { return Shape::change(FenceClass::Other) };
    if is_drop_caches(&segs) {
        return Shape::change(FenceClass::DropCaches);
    }
    if let Some(shape) = apt_one(&segs, ctx) {
        return shape;
    }
    // The named commands, with no redirection: `shutdown -c >/etc/passwd` is root writing a file.
    let redirected = segs.iter().flatten().any(|w| redirection(w).is_some());
    if let ([words], false) = (segs.as_slice(), redirected) {
        let w: Vec<&str> = words.iter().map(String::as_str).collect();
        match w.as_slice() {
            ["systemctl", "daemon-reload"] => return Shape::change(FenceClass::Service),
            ["systemctl", "reboot" | "poweroff" | "halt"] | ["reboot"] | ["shutdown", ..] => return Shape::change(FenceClass::PowerOne),
            ["systemctl", verb, rest @ ..] if ["start", "stop", "restart", "reload", "enable", "disable"].contains(verb) => {
                let mut rest: Vec<&str> = rest.iter().copied().filter(|a| *a != "--now").collect();
                if rest.first() == Some(&"--") {
                    rest.remove(0);
                }
                if !rest.is_empty() && rest.iter().all(|u| !u.starts_with('-')) {
                    return Shape::Change(FenceClass::Service, rest.iter().map(|u| vec![u.to_string()]).collect());
                }
            }
            ["apt-mark", "hold" | "unhold", pkgs @ ..] if !pkgs.is_empty() && pkgs.iter().all(|p| !p.starts_with('-')) => {
                return Shape::Change(FenceClass::AptHold, pkgs.iter().map(|p| vec![p.to_string()]).collect());
            }
            ["nmcli", "radio", "wifi", "on" | "off"] => return Shape::change(FenceClass::Wifi),
            // `apt-get -o DPkg::Lock::Timeout=300 update` (C3): no environment, and only the -o
            // settings `apt_option_reads` allows (a hook setting runs its command as root).
            ["apt-get", opts @ .., "update"] if apt_options(opts) => return Shape::change(FenceClass::AptRefresh),
            _ => {}
        }
    }
    if reads(cmd) {
        Shape::change(FenceClass::SudoProbe)
    } else {
        Shape::change(FenceClass::Other)
    }
}

/// `-o <setting>` pairs, each a setting `classify::apt_option_reads` allows.
fn apt_options(opts: &[&str]) -> bool {
    opts.len().is_multiple_of(2) && opts.chunks(2).all(|kv| kv[0] == "-o" && classify::apt_option_reads(kv[1]))
}

/// The folder detached upgrades log to and keep their package list in (`Live::upgrade_log_dir`):
/// fixed, so an upgrade's root writes (mkdir, chmod, the list) can't be pointed anywhere else. A
/// folder under root's helper folder in tests.
fn upgrade_dir_ok(d: &str, ctx: &Ctx) -> bool {
    d == "/var/log/sparkbox" || (cfg!(test) && !ctx.root_lib.is_empty() && d == format!("{}/logs", ctx.root_lib))
}

/// ops/updates.rs `detached_command` (C1′): the package list saved first, then one systemd unit
/// running apt-get on the named packages, each pinned to a version (`install --only-upgrade
/// name=version…`, its names the items), then the checks after it. Exactly these commands, one
/// folder and one id throughout, the three environment settings and only the -o settings
/// `apt_option_reads` allows; anything more is `other`.
fn apt_one(segs: &[Vec<String>], ctx: &Ctx) -> Option<Shape> {
    let s: Vec<Vec<&str>> = segs.iter().map(|w| w.iter().map(String::as_str).collect()).collect();
    match s.as_slice() {
        [mk, chmod, before, run] => {
            let ["mkdir", "-p", d] = mk.as_slice() else { return None };
            let ["dpkg-query", "-W", list] = before.as_slice() else { return None };
            if !upgrade_dir_ok(d, ctx) || chmod.as_slice() != ["chmod", "0755", *d] {
                return None;
            }
            let (id, inner) = upgrade_unit(run, d)?;
            if redirection(list) != Some(&format!(">{d}/dpkg-before-{id}.txt")) {
                return None;
            }
            upgrade_inner(inner)
        }
        _ => None,
    }
}

/// `systemd-run --unit sparkbox-upgrade-<id> --collect -p StandardOutput=append:<d>/upgrade-<id>.log
/// -p StandardError=append:<same> /bin/sh -c <inner>`: the id and the inner script.
fn upgrade_unit<'a>(run: &[&'a str], d: &str) -> Option<(&'a str, &'a str)> {
    let ["systemd-run", "--unit", unit, "--collect", "-p", out, "-p", err, "/bin/sh", "-c", inner] = run else { return None };
    let id = unit.strip_prefix("sparkbox-upgrade-").filter(|id| !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))?;
    let log = format!("{d}/upgrade-{id}.log");
    (*out == format!("StandardOutput=append:{log}") && *err == format!("StandardError=append:{log}")).then_some((id, *inner))
}

/// C1's script in the unit: `trap : TERM; env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l
/// NEEDRESTART_SUSPEND=1 apt-get … ; code=$?; echo SPARKBOX-AFTER; apt-get check; needrestart -b
/// -r l; echo SPARKBOX-AFTER-END; echo "SPARKBOX-EXIT $code"; exit $code`, nothing else.
fn upgrade_inner(inner: &str) -> Option<Shape> {
    let segs = segments(inner)?;
    let s: Vec<Vec<&str>> = segs.iter().map(|w| w.iter().map(String::as_str).collect()).collect();
    let [trap, apt, code, after, check, needrestart, after_end, exit_line, exit] = s.as_slice() else { return None };
    let fixed: [(&Vec<&str>, &[&str]); 8] = [
        (trap, &["trap", ":", "TERM"]),
        (code, &["code=$?"]),
        (after, &["echo", "SPARKBOX-AFTER"]),
        (check, &["apt-get", "check"]),
        (needrestart, &["needrestart", "-b", "-r", "l"]),
        (after_end, &["echo", "SPARKBOX-AFTER-END"]),
        (exit_line, &["echo", "SPARKBOX-EXIT $code"]),
        (exit, &["exit", "$code"]),
    ];
    if !fixed.iter().all(|(got, want)| got.as_slice() == *want) {
        return None;
    }
    let ["env", "DEBIAN_FRONTEND=noninteractive", "NEEDRESTART_MODE=l", "NEEDRESTART_SUSPEND=1", "apt-get", rest @ ..] = apt.as_slice() else { return None };
    apt_get_upgrade(rest)
}

/// apt-get's words after its name: `-o <setting>` pairs (`apt_option_reads`), then C1′'s
/// `-y --no-remove install --only-upgrade <name=version…>`, every package pinned to the version
/// the dry run showed. The items are the names.
fn apt_get_upgrade(rest: &[&str]) -> Option<Shape> {
    let opts = rest.iter().take_while(|w| !matches!(**w, "-y")).count();
    if !apt_options(&rest[..opts]) {
        return None;
    }
    let pinned = |p: &str| p.contains('=') && classify::package_spec(p);
    match &rest[opts..] {
        ["-y", "--no-remove", "install", "--only-upgrade", pins @ ..] if !pins.is_empty() && pins.iter().all(|p| pinned(p)) => {
            Some(Shape::Change(FenceClass::AptOne, pins.iter().map(|p| vec![p.split('=').next().unwrap_or(p).to_string()]).collect()))
        }
        ["-y", "--no-remove", "install", pins @ ..] if crate::ops::protected_updates::allowed_pins(pins) => {
            Some(Shape::Change(FenceClass::AptOne, pins.iter().map(|p| vec![p.split('=').next().unwrap_or(p).to_string()]).collect()))
        }
        _ => None,
    }
}

/// `sync; echo 3 > /proc/sys/vm/drop_caches` (or `&&`).
fn is_drop_caches(segs: &[Vec<String>]) -> bool {
    let flat: Vec<Vec<&str>> = segs.iter().map(|s| s.iter().map(String::as_str).collect()).collect();
    matches!(flat.as_slice(), [a, b] if a.as_slice() == ["sync"] && matches!(b.as_slice(), ["echo", "3", out] if redirection(out) == Some(">/proc/sys/vm/drop_caches")))
}

fn run_shape(cmd: &str, ctx: &Ctx) -> Shape {
    if let Some(args) = cmd.strip_prefix(SPARKJOB) {
        return sparkjob_shape(args, ctx);
    }
    if reads(cmd) {
        return Shape::Read;
    }
    if let Some(s) = docker_verb(cmd, ctx) {
        return s;
    }
    // The Files window's New folder, Rename and permission change, all confined to ~/sparkbox-test.
    if let Some(s) = confined_file_op(cmd, ctx) {
        return s;
    }
    let Some(segs) = segments(cmd) else { return Shape::change(FenceClass::Other) };
    // `mkdir -p ~/.sparkbox[/…]`: SparkBox's own folder on the node (a redirection is no folder).
    let own = |p: &String| (p == "~/.sparkbox" || p.starts_with("~/.sparkbox/")) && !p.split('/').any(|c| c == "..");
    if segs.iter().all(|s| s.len() >= 3 && s[0] == "mkdir" && s[1] == "-p" && s[2..].iter().all(own)) {
        return Shape::Bookkeeping;
    }
    if let Some(s) = docker_run(&segs, cmd) {
        return s;
    }
    if let Some(s) = model_unload(&segs) {
        return s;
    }
    if let [words] = segs.as_slice() {
        let w: Vec<&str> = words.iter().map(String::as_str).collect();
        let pid = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit());
        match w.as_slice() {
            ["kill", "-s", sig, p] if sig.chars().all(|c| c.is_ascii_alphanumeric()) && pid(p) => return Shape::change(FenceClass::ProcessTest),
            ["kill", sig, p] if sig.starts_with('-') && sig[1..].chars().all(|c| c.is_ascii_alphanumeric()) && pid(p) => return Shape::change(FenceClass::ProcessTest),
            ["renice", "-n", n, "-p", p] if n.trim_start_matches('-').chars().all(|c| c.is_ascii_digit()) && pid(p) => return Shape::change(FenceClass::ProcessTest),
            _ => {}
        }
    }
    Shape::change(FenceClass::Other)
}

/// The Files window's shell commands that stay inside ~/sparkbox-test (T2's files-test): a New
/// folder (`mkdir -- <p>`), a permission change (`chmod -- <mode> <p>`), or a rename
/// (`rename_literal`), every path under ~/sparkbox-test with no `..`. None otherwise.
fn confined_file_op(cmd: &str, ctx: &Ctx) -> Option<Shape> {
    let test = |p: &str| in_test_dir(p, ctx.home);
    if let Some((src, to)) = rename_literal(cmd) {
        return (test(&src) && test(&to)).then(|| Shape::change(FenceClass::FilesTest));
    }
    let segs = segments(cmd)?;
    let [words] = segs.as_slice() else { return None };
    let w = plain_words(words)?;
    match w.as_slice() {
        ["mkdir", "--", p] if test(p) => Some(Shape::change(FenceClass::FilesTest)),
        ["chmod", "--", mode, p] if !mode.is_empty() && mode.chars().all(|c| c.is_ascii_digit()) && test(p) => Some(Shape::change(FenceClass::FilesTest)),
        _ => None,
    }
}

/// `files::rename_command(SRC, T)` — `if [ -e T ] || [ -L T ]; then printf '…' T >&2; exit 1;
/// fi; mv -- SRC T` — byte for byte, and its (SRC, T), or None. Its `if … fi` is a block
/// `segments` refuses, so it's recognised by its exact text instead.
fn rename_literal(cmd: &str) -> Option<(String, String)> {
    let t = cmd.strip_prefix("if [ -e ")?.split(" ] || [ -L ").next()?;
    let to = unquote(t)?;
    let src = unquote(cmd.split("; fi; mv -- ").nth(1)?.strip_suffix(&format!(" {t}"))?)?;
    (crate::files::rename_command(&src, &to) == cmd).then_some((src, to))
}

/// The text of one `live::sh_quote`d word, or None when `q` is anything else.
fn unquote(q: &str) -> Option<String> {
    let inner = q.strip_prefix('\'')?.strip_suffix('\'')?;
    let text = inner.replace("'\\''", "\u{0}");
    (!text.contains('\'')).then(|| text.replace('\u{0}', "'"))
}

/// The unit of `updates::cancel_command(unit)` (C-X), byte for byte, for a unit SparkBox names
/// (`sparkbox-upgrade-<id>`), or None. Its `if … fi` and `${p:-0}` are syntax `segments` refuses,
/// so it's recognised by its exact text: the exit lets it past an apt-one fence's `only` list.
pub fn upgrade_cancel_unit(cmd: &str) -> Option<String> {
    let q = cmd.split("p=$(systemctl show -p MainPID --value -- ").nth(1)?.split(')').next()?;
    let unit = unquote(q)?;
    let id = unit.strip_prefix("sparkbox-upgrade-")?;
    let named = !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    (named && crate::ops::updates::cancel_command(&unit) == cmd).then_some(unit)
}

/// The commands whose shape is `other` that a call site may still tag with its class, each exactly
/// as SparkBox writes it: Updates' Cancel (C-X, `upgrade_cancel_unit`) as apt-one, the Processes
/// window's PID-guarded signal (`guarded_signal`) as process-test, and Doctor's sshd check as
/// sudo-probe. Under any other tag an `other` stays `other` (`exit::tagged`), so a call site's
/// class never covers a command its shape doesn't name.
pub fn tag_names(kind: ExecKind, cmd: &str, class: FenceClass) -> bool {
    match (class, kind) {
        (FenceClass::AptOne, ExecKind::Sudo) => upgrade_cancel_unit(cmd).is_some(),
        (FenceClass::ProcessTest, ExecKind::Run) => guarded_signal(cmd),
        (FenceClass::SudoProbe, ExecKind::Sudo) => cmd == super::DOCTOR_SSHD_T,
        _ => false,
    }
}

/// `ops::pid_guard_cmd(pid, signal, Some(start), "/proc")` byte for byte, for a signal the
/// Processes window sends and a PID above 1. A test's copy of /proc stands in for it.
pub fn guarded_signal(cmd: &str) -> bool {
    let Some(stat) = cmd.strip_prefix("s=$(sed 's/^.*) //' ").and_then(|r| r.split_once(" 2>/dev/null | cut ")).map(|(s, _)| s) else { return false };
    let Some((root, pid)) = stat.strip_suffix("/stat").and_then(|p| p.rsplit_once('/')) else { return false };
    let start = cmd.split("[ \"$s\" = \"").nth(1).and_then(|r| r.split('"').next()).and_then(|s| s.parse::<u64>().ok());
    let signal = cmd.rsplit_once("; kill -s ").and_then(|(_, t)| t.split_once(' ')).map(|(s, _)| s).unwrap_or("");
    let (Ok(pid), Some(start)) = (pid.parse::<u32>(), start) else { return false };
    let proc_ok = root == "/proc" || (cfg!(test) && root.starts_with('/') && !root.split('/').any(|p| p == "..") && root.chars().all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c)));
    pid > 1 && proc_ok && matches!(signal, "TERM" | "INT" | "HUP" | "KILL" | "STOP" | "CONT") && crate::ops::pid_guard_cmd(pid, signal, Some(start), root) == cmd
}

/// A copy over exec_io: `tar -C <dir> -cvf - -- <names…>` packs files off the node (a read);
/// `mkdir -p -- <stage> && tar -C <stage> -xf -` unpacks into a staging folder, files-test when
/// that folder is under ~/sparkbox-test. None otherwise.
fn exec_io_copy_shape(cmd: &str, ctx: &Ctx) -> Option<Shape> {
    let segs = segments(cmd)?;
    // A redirection opens a file the copy never named (`tar … -- x >~/.bashrc`).
    let s: Vec<Vec<&str>> = segs.iter().map(|w| plain_words(w)).collect::<Option<_>>()?;
    match s.as_slice() {
        [tar] => match tar.as_slice() {
            ["tar", "-C", _dir, "-cvf", "-", "--", names @ ..] if !names.is_empty() => Some(Shape::Read),
            _ => None,
        },
        [mk, tar] => match (mk.as_slice(), tar.as_slice()) {
            (["mkdir", "-p", "--", stage], ["tar", "-C", stage2, "-xf", "-"]) if stage == stage2 && in_test_dir(stage, ctx.home) => Some(Shape::change(FenceClass::FilesTest)),
            _ => None,
        },
        _ => None,
    }
}

/// `docker start|stop|restart|kill|rm|pause|unpause [options] <name|id>…` on named containers:
/// a test container, a load's container, or anyone else's (`container-owner`).
fn docker_verb(cmd: &str, ctx: &Ctx) -> Option<Shape> {
    let segs = segments(cmd)?;
    let [words] = segs.as_slice() else { return None };
    let w = plain_words(words)?;
    let ["docker", verb, args @ ..] = w.as_slice() else { return None };
    if !DOCKER_VERBS.contains(verb) {
        return None;
    }
    let mut names = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match *a {
            "-t" | "--time" | "-s" | "--signal" => {
                it.next();
            }
            o if o.starts_with('-') => {}
            n => names.push(n),
        }
    }
    if names.is_empty() || !names.iter().all(|n| n.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))) {
        return None;
    }
    let mut class = None;
    let mut items = Vec::new();
    for n in names {
        let found = find_container(ctx.containers, n);
        let c = if n.starts_with("sparkbox-test-") || found.is_some_and(|c| c.name.starts_with("sparkbox-test-")) {
            FenceClass::ContainerTest
        } else if found.is_some_and(|c| c.labels.contains_key("sparkbox.deployment")) {
            FenceClass::ModelUnload
        } else {
            FenceClass::ContainerOwner
        };
        if class.is_some_and(|k| k != c) {
            return Some(Shape::change(FenceClass::Other));
        }
        class = Some(c);
        let mut aliases = vec![n.to_string()];
        for a in found.into_iter().flat_map(|c| [c.name.clone(), c.id.clone()]) {
            if !aliases.contains(&a) {
                aliases.push(a);
            }
        }
        items.push(aliases);
    }
    Some(Shape::Change(class?, items))
}

/// A change that acts on no item by nature, so a class's `only` list doesn't narrow it:
/// `systemctl daemon-reload` reloads unit files and starts, stops or enables no unit.
pub fn acts_on_no_item(kind: ExecKind, cmd: &str) -> bool {
    kind == ExecKind::Sudo && segments(&unwrap_sh(blank_trim(cmd))).is_some_and(|s| s == [vec!["systemctl".to_string(), "daemon-reload".to_string()]])
}

/// Every container a `docker start|stop|restart|kill|rm|pause|unpause` (or `docker container
/// <verb>`) in the command names, and the one a `docker exec` or `docker attach` runs in (a
/// shell in it, a command in it), in any of its simple commands, under each name it goes by:
/// what the exit's check (d) looks at, whatever class let the change through (a typed Fleet
/// command's, a terminal's `docker exec -it <name> sh`). A name built at run time (`$(docker ps
/// -q)`, `$X`, a glob) can't be known here, so it names every container in the frame (fail
/// closed); a load's unload (C-U2, C-S) removes what its own label filter finds, and the exit
/// checks that label (`unload_instance`) instead.
pub fn named_containers(kind: ExecKind, cmd: &str, ctx: &Ctx) -> Vec<Item> {
    if kind == ExecKind::DirectTcpip {
        return Vec::new();
    }
    let cmd = unwrap_sh(blank_trim(cmd));
    if segments(&cmd).is_some_and(|s| unload_filter(&s).is_some()) {
        return Vec::new();
    }
    // Every word that could name a container, syntax `segments` refuses included.
    let Some(segs) = split_words(&cmd, false) else { return Vec::new() };
    let mut out: Vec<Item> = Vec::new();
    let every = |out: &mut Vec<Item>| {
        for c in ctx.containers {
            collect_container(out, ctx.containers, &c.name);
        }
    };
    for words in &segs {
        // Past a block's words (`if`, `then`, `{`, `!`…) to the command.
        let w0: Vec<&str> = words.iter().map(String::as_str).skip_while(|w| RESERVED.contains(w) || matches!(*w, "{" | "}")).collect();
        // Wrappers that leave docker's arguments alone (`/usr/bin/docker`, `env`, `timeout 5`,
        // `sudo -u root`, `nice`, `nohup`, `command`, `exec`, `setsid`), then docker's own global
        // options before the verb (`docker -H … --tls stop x`).
        let w = match peel_to_docker(&w0) {
            Some(Peeled::Docker(w)) => w,
            // `env -S STRING`: what runs is in the string.
            Some(Peeled::Line(line)) => {
                out.extend(named_containers(kind, &line, ctx));
                continue;
            }
            None => continue,
        };
        let kept = docker_global_opts(&w);
        // A container named by a word the shell builds at run time could be any of them.
        let name = |out: &mut Vec<Item>, n: &str| if n.contains(['$', '`', '*', '?', '[', '{']) { every(out) } else { collect_container(out, ctx.containers, n) };
        match kept.as_slice() {
            ["docker", "container", "exec" | "attach", args @ ..] | ["docker", "exec" | "attach", args @ ..] => {
                if let Some(n) = exec_container(args) {
                    name(&mut out, n);
                }
            }
            // Prunes name every container in the frame (docker removes the stopped ones): fail closed.
            ["docker", "container", "prune", ..] | ["docker", "system", "prune", ..] => every(&mut out),
            // `docker cp <container>:<path> <dest>` (either end may be the container; the other is
            // a local path with no colon).
            ["docker", "container", "cp", args @ ..] | ["docker", "cp", args @ ..] => {
                for a in operands_of(args) {
                    if let Some((host, _path)) = a.split_once(':') {
                        if !host.is_empty() && !host.contains('/') {
                            name(&mut out, host);
                        }
                    }
                }
            }
            // `docker network connect|disconnect <net> <container>`.
            ["docker", "network", act, args @ ..] if matches!(*act, "connect" | "disconnect") => {
                if let Some(n) = operands_of(args).get(1) {
                    name(&mut out, n);
                }
            }
            // The verb's one container (update, rename, commit, export act on the first operand).
            ["docker", "container", verb, args @ ..] | ["docker", verb, args @ ..] if matches!(*verb, "update" | "rename" | "commit" | "export") => {
                if let Some(n) = operands_of(args).first() {
                    name(&mut out, n);
                }
            }
            // stop|start|restart|kill|rm|pause|unpause: every container named.
            ["docker", "container", verb, args @ ..] | ["docker", verb, args @ ..] if DOCKER_VERBS.contains(verb) => {
                let mut it = args.iter();
                while let Some(a) = it.next() {
                    match *a {
                        "-t" | "--time" | "-s" | "--signal" => {
                            it.next();
                        }
                        o if o.starts_with('-') => {}
                        n => name(&mut out, n),
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// A container name as a word, under each name it goes by: a leading `/name` is stripped (docker's
/// form), the name must be plain, and the frame's container (if any) adds its name and id.
fn collect_container(out: &mut Vec<Item>, containers: &[Container], word: &str) {
    let n = word.strip_prefix('/').unwrap_or(word);
    if n.is_empty() || !n.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)) {
        return;
    }
    let mut aliases = vec![n.to_string()];
    for a in find_container(containers, n).into_iter().flat_map(|c| [c.name.clone(), c.id.clone()]) {
        if !aliases.contains(&a) {
            aliases.push(a);
        }
    }
    out.push(aliases);
}

/// The operands of a docker verb's arguments (past options, and the values of the options that
/// take one), for the verbs whose container is a plain operand.
fn operands_of<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let takes_value = |o: &str| matches!(o, "-s" | "--signal" | "-t" | "--time" | "-f" | "--filter" | "--restart" | "-l" | "--label" | "-c" | "--cpu-shares" | "-m" | "--memory" | "-a" | "--archive" | "--network");
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if a == "--" {
            out.extend(&args[i + 1..]);
            break;
        }
        if a.starts_with('-') && a != "-" {
            if !a.contains('=') && takes_value(a) {
                i += 1;
            }
            i += 1;
            continue;
        }
        out.push(a);
        i += 1;
    }
    out
}

/// What's under a segment's wrappers.
enum Peeled<'a> {
    /// The words from `docker` on (its basename normalised to `docker`).
    Docker(Vec<&'a str>),
    /// A command line of its own (`env -S`), to read again.
    Line(String),
}

/// Peels the wrappers that don't change docker's arguments, or None when the segment isn't a
/// docker invocation.
fn peel_to_docker<'a>(words: &[&'a str]) -> Option<Peeled<'a>> {
    let mut w: Vec<&'a str> = words.to_vec();
    loop {
        let &first = w.first()?;
        if is_assignment(first) {
            w.remove(0);
            continue;
        }
        w = match first.rsplit('/').next().unwrap_or(first) {
            "docker" => {
                let mut v = w;
                v[0] = "docker";
                return Some(Peeled::Docker(v));
            }
            "sudo" | "doas" | "pkexec" | "run0" => drop_opt_values(&w[1..], &["-u", "-g", "-h", "-p", "-C", "-U", "-r", "-t", "-D", "--user"]),
            "env" => match classify::env_runs(&w[1..]) {
                classify::EnvRuns::Line(line) => return Some(Peeled::Line(line)),
                classify::EnvRuns::Words(rest) => rest,
                classify::EnvRuns::Unread => return None,
            },
            "setsid" => drop_opt_values(&w[1..], &[]),
            // nice, nohup, command, exec, taskset, timeout… each with its own options (`command -p docker`).
            name => match classify::wrapped(name, &w[1..])? {
                classify::Wrapped::Runs(rest) => rest,
                _ => return None,
            },
        };
    }
}

/// Drops leading options (and the values of those in `with_value`) up to the first other word.
fn drop_opt_values<'a>(args: &[&'a str], with_value: &[&str]) -> Vec<&'a str> {
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if a == "--" {
            i += 1;
            break;
        }
        if !a.starts_with('-') || a == "-" {
            break;
        }
        if with_value.contains(&a) {
            i += 2;
        } else {
            i += 1;
        }
    }
    args[i.min(args.len())..].to_vec()
}

/// docker's own global options before the verb (`docker -H unix:///… --tls stop x`): dropped.
fn docker_global_opts<'a>(w: &[&'a str]) -> Vec<&'a str> {
    if w.first() != Some(&"docker") {
        return w.to_vec();
    }
    let mut rest = w[1..].iter().copied();
    let mut kept = vec!["docker"];
    while let Some(a) = rest.next() {
        match a {
            "-H" | "--host" | "--context" | "-c" | "--config" | "-l" | "--log-level" | "--tlscacert" | "--tlscert" | "--tlskey" => {
                rest.next();
            }
            o if o.starts_with('-') => {}
            verb => {
                kept.push(verb);
                kept.extend(rest.by_ref());
            }
        }
    }
    kept
}

/// `docker exec` (and `attach`) options that take a value, long and one-letter.
const EXEC_LONG_VALUE: [&str; 5] = ["env", "env-file", "user", "workdir", "detach-keys"];
const EXEC_SHORT_VALUE: &str = "euw";

/// The container `docker exec [options] <name|id> <command>…` runs in, its options read the way
/// docker's CLI reads them: `-e TERM=x`, `--user=0`, `-uroot`, `-itu 0`.
fn exec_container<'a>(args: &[&'a str]) -> Option<&'a str> {
    let mut it = args.iter();
    while let Some(&a) = it.next() {
        if a == "--" {
            return it.next().copied();
        }
        if let Some(long) = a.strip_prefix("--") {
            if !long.contains('=') && EXEC_LONG_VALUE.contains(&long) {
                it.next();
            }
            continue;
        }
        if let Some(short) = a.strip_prefix('-').filter(|s| !s.is_empty()) {
            // In a cluster, the first letter that takes a value takes the rest of it, or the next word.
            if short.find(|c| EXEC_SHORT_VALUE.contains(c)).is_some_and(|i| i + 1 == short.len()) {
                it.next();
            }
            continue;
        }
        return Some(a);
    }
    None
}

/// The frame's container called `n`, or whose id starts with it (or it with the id).
pub fn find_container<'a>(containers: &'a [Container], n: &str) -> Option<&'a Container> {
    containers.iter().find(|c| c.name == n || same_id(&c.id, n))
}

/// For the exit's check (d): the frame container a word could reach — by exact name, a leading
/// `/name` (docker's form), the full id, or a short hex word that prefixes an id (docker takes
/// any unique id prefix). Fails closed: a 4+‑hex word that prefixes the other build's container's
/// id is taken to name it.
pub fn names_container<'a>(containers: &'a [Container], word: &str) -> Option<&'a Container> {
    let w = word.strip_prefix('/').unwrap_or(word);
    let hex_prefix = w.len() >= 4 && w.chars().all(|c| c.is_ascii_hexdigit());
    containers.iter().find(|c| c.name == w || same_id(&c.id, w) || (hex_prefix && c.id.starts_with(w)))
}

/// Two container ids name the same one: equal, or one a prefix of the other (12 hex at least).
pub fn same_id(a: &str, b: &str) -> bool {
    let hex = |s: &str| s.len() >= 12 && s.chars().all(|c| c.is_ascii_hexdigit());
    hex(a) && hex(b) && (a.starts_with(b) || b.starts_with(a))
}

/// A `sparkbox-test-*` container labelled sparkbox.test=1, or a rank of a load (C-L2, `rank_load`),
/// as models/plan.rs and ops/containers.rs write them.
fn docker_run(segs: &[Vec<String>], raw: &str) -> Option<Shape> {
    let i = segs.iter().position(|s| s.len() > 2 && s[0] == "docker" && s[1] == "run")?;
    let run = &segs[i];
    let others = || segs.iter().enumerate().filter(move |(j, _)| *j != i).map(|(_, s)| s);
    let label = |want: &str| run.windows(2).any(|w| w[0] == "--label" && w[1] == want) || run.iter().any(|w| *w == format!("--label={want}"));
    let name = run.windows(2).find(|w| w[0] == "--name").map(|w| w[1].clone());
    if name.as_deref().is_some_and(|n| n.starts_with("sparkbox-test-")) && label("sparkbox.test=1") {
        // A test container stands alone: anything alongside it, or a redirection into a file, is
        // `other`.
        if others().next().is_some() || redirects(run) {
            return Some(Shape::change(FenceClass::Other));
        }
        // A test container is container-test only when its run options are all on the short
        // list below (fail closed: --security-opt, --userns, --volumes-from, a named volume and
        // whatever docker adds next are `other`), it takes no root (no --privileged, host
        // mount, host pid/net, --cap-add, --device or uid 0) and its own command is a
        // non-destructive change; anything else is `other` (an `any` fence only).
        if !test_run_options_ok(&run[2..]) {
            return Some(Shape::change(FenceClass::Other));
        }
        let v = crate::ops::classify::verdict(&join_quoted(run));
        if v.root || v.kind == Kind::Destructive {
            return Some(Shape::change(FenceClass::Other));
        }
        return Some(Shape::Change(FenceClass::ContainerTest, name.into_iter().map(|n| vec![n]).collect()));
    }
    rank_load(segs, raw)
}

/// A deployment or load id as plan.rs makes it (`safe` minus `/`): one plain name, not `.` or `..`
/// (an unload is `rm -rf` of it), no `$` or glob, at most 40 characters.
fn plain_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && !matches!(id, "." | "..") && id.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// A test container's `docker run` options, before its image: only these, none of which reaches
/// the host. `-d`/`-i`/`-t` (and their clusters), `--rm`, `--init`, a name, labels, memory, CPU and
/// process limits, environment, a working directory, a tmpfs, and the network `none` or `bridge`.
fn test_run_options_ok(args: &[String]) -> bool {
    const FLAGS: &[&str] = &["-d", "--detach", "-i", "--interactive", "-t", "--tty", "--rm", "--init"];
    const VALUED: &[&str] = &["--name", "--label", "-l", "--memory", "-m", "--memory-swap", "--cpus", "--pids-limit", "-e", "--env", "-w", "--workdir", "--tmpfs"];
    let mut i = 0;
    while let Some(a) = args.get(i).map(String::as_str) {
        if !a.starts_with('-') {
            // The image: what follows is its command, classified on its own.
            return true;
        }
        let (name, attached) = match a.split_once('=') {
            Some((n, v)) => (n, Some(v)),
            None => (a, None),
        };
        let value = || attached.or_else(|| args.get(i + 1).map(String::as_str));
        match name {
            _ if attached.is_none() && FLAGS.contains(&name) => i += 1,
            // `-dit`, `-it`: one-letter flags run together.
            _ if attached.is_none() && name.len() > 2 && !name.starts_with("--") && name[1..].chars().all(|c| "dit".contains(c)) => i += 1,
            "--network" | "--net" if matches!(value(), Some("none" | "bridge")) => i += if attached.is_some() { 1 } else { 2 },
            _ if VALUED.contains(&name) && value().is_some() => i += if attached.is_some() { 1 } else { 2 },
            _ => return false,
        }
    }
    // No image at all.
    false
}

/// The compile caches' mounts ("$HOME/…", quoted), and the launcher's, as plan.rs writes them.
const CACHE_MOUNTS: [&str; 3] = ["$HOME/.cache/vllm:/root/.cache/vllm", "$HOME/.cache/flashinfer:/root/.cache/flashinfer", "$HOME/.triton:/root/.triton"];
fn launch_mount() -> String {
    format!("$HOME/.sparkbox/{}:/run/sparkbox/launch.py:ro", crate::models::plan::launcher_name())
}
const LAUNCH: &str = "/run/sparkbox/launch.py";
/// Folders a model mount may never be (or be under).
const NOT_MODELS: [&str; 8] = ["/etc", "/root", "/boot", "/proc", "/sys", "/dev", "/run", "/var/run"];

/// A rank of a load (C-L2), exactly, else None: a one-node load is `model-load-one`, a head or a
/// worker `model-load`, so a fence naming only the first refuses the second. Items: the container's
/// name.
///
/// Reviewer's note (the argv on stdin, round plan finding 8): these words only start launch.py. What
/// it runs is in the deploy folder, sent on stdin to `sparkjob.py deploy-files`, which this exit
/// never reads, so a fence can't name a load's flags. Two checks bound that, neither of them here,
/// each on its own: `deploy_files` refuses anything but `vllm serve …`, the flags that carry a key
/// or run code, an environment name that's a secret, isn't on the list, or loads code or writes a
/// file (any name with PLUGIN in it, VLLM_LOGGING_CONFIG_PATH, NCCL_DEBUG_FILE) and a key on a
/// worker, before it writes a file; and
/// launch.py refuses a head other than `vllm serve` and env.json's PATH, LD_PRELOAD and the like
/// again in the container, then execs with no shell. launch.py comes from ~/.sparkbox: a load copies
/// it there after this exit admits `deploy-files`, reading its checksum for every load, and it's
/// mounted read-only. So the worst a wrong payload does is `vllm serve` with wrong flags in a
/// container this shape already allowed, and each rank's argv and env (never the key) go in the
/// activity log. A fence that wants a load's flags narrowed can't have that; it can only refuse the
/// class. The host launcher filename contains this build's content hash, and this parser requires
/// that exact mount. Another build with different launcher bytes writes a different filename.
fn rank_load(segs: &[Vec<String>], raw: &str) -> Option<Shape> {
    let l = load(segs, raw)?;
    let class = if l.role == "single" { FenceClass::ModelLoadOne } else { FenceClass::ModelLoad };
    Some(Shape::Change(class, vec![vec![l.name.to_string()]]))
}

/// What the exit needs of a rank's `docker run`.
struct Load<'a> {
    name: &'a str,
    instance: &'a str,
    role: &'a str,
}

/// C-L2 alone: one command, with no preamble and no redirection (`load_run` sees one as a word).
/// None of its words holds a `;`, `&`, `|` or newline, so one in `raw` is the shell's: a trailing
/// `&` or `;` leaves `segs` as it was.
fn load<'a>(segs: &'a [Vec<String>], raw: &str) -> Option<Load<'a>> {
    if raw.contains([';', '&', '|', '\n']) {
        return None;
    }
    let [run] = segs else { return None };
    let words: Vec<&str> = run.iter().map(String::as_str).collect();
    let ["docker", "run", args @ ..] = words.as_slice() else { return None };
    load_run(args, raw)
}

/// The build a load (C-L2) labels its container for (its sparkbox.instance), when `cmd` is one: the
/// exit refuses a load labelled for the other build, as check (d).
pub fn load_instance(kind: ExecKind, cmd: &str) -> Option<String> {
    if !matches!(kind, ExecKind::Run | ExecKind::Probe | ExecKind::Cancellable | ExecKind::Stream) {
        return None;
    }
    let cmd = unwrap_sh(blank_trim(cmd));
    let segs = segments(&cmd)?;
    load(&segs, &cmd).map(|l| l.instance.to_string())
}

/// C-L2's words, walked in the order plan.rs writes them.
struct Words<'a, 'b>(&'b [&'a str]);

impl<'a> Words<'a, '_> {
    fn take(&mut self) -> Option<&'a str> {
        let (&w, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(w)
    }

    /// These words come next, and are passed.
    fn next(&mut self, want: &[&str]) -> Option<()> {
        (self.0.len() >= want.len() && self.0.iter().zip(want).all(|(a, b)| a == b)).then(|| self.0 = &self.0[want.len()..])
    }

    /// `--label <key>=<value>` comes next: its value.
    fn label(&mut self, key: &str) -> Option<&'a str> {
        let ["--label", kv, rest @ ..] = self.0 else { return None };
        let value = kv.strip_prefix(key)?.strip_prefix('=')?;
        self.0 = rest;
        Some(value)
    }
}

/// Letters, digits and `._/-`, and `extra`: a label's value that nothing reads anything into.
fn plain(v: &str, extra: &str) -> bool {
    !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c) || extra.contains(c))
}

fn digits(v: &str) -> bool {
    !v.is_empty() && v.chars().all(|c| c.is_ascii_digit())
}

/// A rank's `docker run` arguments, word for word as `plan::rank_command_for` writes them and
/// nothing else. `-d`; its name; the labels sparkbox.instance (release or dev), .deployment,
/// .group, .role, .rank, .model, .recipe, .image, .path, .parallel (`<tp>x<pp>`) and .gpu-share
/// (the plan's two-decimal fraction strictly between zero and one), then .port and
/// .listen on the rank that serves, and .maxlen when it's set, each plain; `--network host --ipc
/// host --gpus all`; three ulimits; `--privileged` only on a head or a worker; `--restart no`, or
/// `unless-stopped` on one node; the model mount (`model_mount`) of .path's folder, the caches',
/// this deployment's launch files, the launcher's, and on a rank that serves its key's, when it
/// has one; then the launcher by the image's local id, with `--key` exactly when the key's mounted.
/// The name says the role and the rank: `sparkbox-<id>` on one node, `-r0` on the head, `-r<rank>`
/// on a worker, never rank 0. `raw` holds the command as written: the mounts stay quoted.
fn load_run<'a>(args: &[&'a str], raw: &str) -> Option<Load<'a>> {
    let mut w = Words(args);
    w.next(&["-d", "--name"])?;
    let name = w.take()?;
    let instance = w.label("sparkbox.instance").filter(|i| matches!(*i, "release" | "dev"))?;
    let dep = w.label("sparkbox.deployment").filter(|d| plain_id(d))?;
    w.label("sparkbox.group").filter(|g| plain_id(g))?;
    let role = w.label("sparkbox.role")?;
    let rank = w.label("sparkbox.rank").filter(|r| digits(r))?;
    w.label("sparkbox.model").filter(|m| plain(m, ":"))?;
    w.label("sparkbox.recipe").filter(|r| plain_id(r))?;
    w.label("sparkbox.image").filter(|r| plain(r, ":@"))?;
    let path = w.label("sparkbox.path").filter(|p| plain(p, "+@"))?;
    w.label("sparkbox.parallel").filter(|p| p.split_once('x').is_some_and(|(t, n)| digits(t) && digits(n)))?;
    w.label("sparkbox.gpu-share").filter(|p| *p != "0.00" && p.strip_prefix("0.").is_some_and(|f| f.len() == 2 && digits(f)))?;
    let named = match role {
        "single" => rank == "0" && name == format!("sparkbox-{dep}"),
        "head" => rank == "0" && name == format!("sparkbox-{dep}-r0"),
        "worker" => digits(rank) && !rank.starts_with('0') && name == format!("sparkbox-{dep}-r{rank}"),
        _ => false,
    };
    if !named {
        return None;
    }
    let serves = role != "worker";
    if serves {
        w.label("sparkbox.port").filter(|p| p.parse::<u16>().is_ok())?;
        w.label("sparkbox.listen").filter(|l| plain(l, ":"))?;
    }
    if w.0.first() == Some(&"--label") {
        w.label("sparkbox.maxlen").filter(|n| digits(n))?;
    }
    w.next(&["--network", "host", "--ipc", "host", "--gpus", "all", "--ulimit", "memlock=-1", "--ulimit", "stack=67108864", "--ulimit", "nofile=1048576:1048576"])?;
    if w.next(&["--privileged"]).is_some() && role == "single" {
        return None;
    }
    w.next(&["--restart"])?;
    if !matches!((w.take()?, role), ("no", _) | ("unless-stopped", "single")) {
        return None;
    }
    let root = crate::models::plan::mount_root(path);
    let model = format!("{root}:{root}:ro");
    w.next(&["-v", &model])?;
    if !model_mount(&model) {
        return None;
    }
    let quoted = |m: &str| raw.contains(&format!("-v \"{m}\"")).then_some(());
    let deploy = format!("$HOME/.sparkbox/deploy/{dep}:/run/sparkbox/deploy:ro");
    let launcher = launch_mount();
    for mount in CACHE_MOUNTS.iter().copied().chain([deploy.as_str(), launcher.as_str()]) {
        w.next(&["-v", mount])?;
        quoted(mount)?;
    }
    let key = format!("$HOME/.sparkbox/keys/{dep}:/run/sparkbox/key:ro");
    let keyed = w.next(&["-v", &key]).is_some();
    if keyed {
        quoted(&key)?;
    }
    w.next(&["--entrypoint", "python3"])?;
    let image = w.take()?;
    if !image.strip_prefix("sha256:").is_some_and(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))) {
        return None;
    }
    w.next(&[LAUNCH])?;
    let key_flag = w.next(&["--key"]).is_some();
    (w.0.is_empty() && keyed == key_flag && !(keyed && !serves)).then_some(Load { name, instance, role })
}

/// A model folder's mount, `<path>:<path>:ro`: absolute, the same path on both sides, plain
/// characters, at least three parts and no `.` or `..` among them, never a system folder
/// (`NOT_MODELS`) or anything in one, and not Docker's socket.
fn model_mount(m: &str) -> bool {
    let Some((src, dst)) = m.strip_suffix(":ro").and_then(|p| p.split_once(':')) else { return false };
    let parts: Vec<&str> = src.split('/').skip(1).collect();
    src == dst
        && src.starts_with('/')
        && parts.len() >= 3
        && parts.iter().all(|p| !p.is_empty() && *p != "." && *p != "..")
        && src.chars().all(|c| c.is_ascii_alphanumeric() || "._/-+@".contains(c))
        && !NOT_MODELS.iter().any(|d| src == *d || src.starts_with(&format!("{d}/")))
        && !src.ends_with("/docker.sock")
}

/// plan.rs `unload_command` (C-U2) and `stop_role_command` (C-S), exactly (`unload_filter`).
fn model_unload(segs: &[Vec<String>]) -> Option<Shape> {
    unload_filter(segs).map(|_| Shape::change(FenceClass::ModelUnload))
}

/// The instance whose containers a load's unload removes (its sparkbox.instance filter), when
/// `cmd` is one (C-U2, C-S): the exit refuses it for the other build's, as check (d).
pub fn unload_instance(kind: ExecKind, cmd: &str) -> Option<String> {
    if kind == ExecKind::DirectTcpip {
        return None;
    }
    unload_filter(&segments(&unwrap_sh(blank_trim(cmd)))?).map(String::from)
}

/// C-U2 and C-S, exactly: the load's containers by its labels (release's or dev's, the
/// deployment's plain id, a role for C-S), stopped with 30 s to finish, removed, then (C-U2) that
/// id's key and launch files, and `echo ok`. The instance its filter names, or None.
fn unload_filter(segs: &[Vec<String>]) -> Option<&str> {
    let [first] = segs.first()?.as_slice() else { return None };
    let inner = first.strip_prefix("ids=$(")?.strip_suffix(')')?;
    let words: Vec<&str> = inner.split(' ').collect();
    let ["docker", "ps", "-aq", "--filter", instance, "--filter", dep, more @ ..] = words.as_slice() else { return None };
    let dep = dep.strip_prefix("label=sparkbox.deployment=").filter(|d| plain_id(d))?;
    let role = match more {
        [] => None,
        ["--filter", r] => Some(r.strip_prefix("label=sparkbox.role=").filter(|r| matches!(*r, "single" | "head" | "worker"))?),
        _ => return None,
    };
    let instance = instance.strip_prefix("label=sparkbox.instance=").filter(|i| matches!(*i, "release" | "dev"))?;
    let (key, deploy) = (format!("~/.sparkbox/keys/{dep}"), format!("~/.sparkbox/deploy/{dep}"));
    let rest: Vec<Vec<&str>> = segs[1..].iter().map(|s| s.iter().map(String::as_str).collect()).collect();
    let quiet = format!("{REDIRECT}>/dev/null");
    let mut want: Vec<Vec<&str>> = vec![vec!["[", "-z", "$ids", "]"], vec!["docker", "stop", "-t", "30", "$ids", &quiet], vec!["[", "-z", "$ids", "]"], vec!["docker", "rm", "$ids", &quiet]];
    if role.is_none() {
        want.push(vec!["rm", "-rf", &key, &deploy]);
    }
    want.push(vec!["echo", "ok"]);
    (rest == want).then_some(instance)
}

/// `sparkjob.py <args>`: a read verb, the lease, or a write verb on files and jobs.
fn sparkjob_shape(args: &str, ctx: &Ctx) -> Shape {
    let Some(segs) = segments(args) else { return Shape::change(FenceClass::Other) };
    let [words] = segs.as_slice() else { return Shape::change(FenceClass::Other) };
    // Its verbs take any words; a redirection among them is the shell's, writing a file
    // (`list > ~/.bashrc`), whatever the verb.
    let Some(words) = plain_words(words) else { return Shape::change(FenceClass::Other) };
    let Some(&verb) = words.first() else { return Shape::change(FenceClass::Other) };
    if words.as_slice() == ["download-tools"] || words.as_slice() == ["disk-preflight"] || words.as_slice() == ["chain-preflight"] {
        return Shape::Read;
    }
    if verb == "lease" {
        return Shape::Bookkeeping;
    }
    if SPARKJOB_READS.contains(&verb) {
        return Shape::Read;
    }
    // `check <root> <image> <ray image> quick [--instance <i>]` reads. The full check starts the
    // vLLM image in a test container to read its version (`docker run --rm --name
    // sparkbox-checkimage-<rand> --label sparkbox.test=1 --label sparkbox.instance=<i> …`):
    // `container-test` with this build's instance named, `other` without one (it'd be unlabelled).
    if verb == "check" {
        return match &words[1..] {
            [_, _, "quick"] | [_, _, _, "quick"] => Shape::Read,
            [_, _, "quick", "--instance", i] | [_, _, _, "quick", "--instance", i] if instance_ok(i) => Shape::Read,
            [_, _, "--instance", i] | [_, _, _, "--instance", i] if instance_ok(i) => Shape::change(FenceClass::ContainerTest),
            _ => Shape::change(FenceClass::Other),
        };
    }
    if !SPARKJOB_WRITES.contains(&verb) {
        return Shape::change(FenceClass::Other);
    }
    let a = &words[1..];
    // A load's launch files, by the role it starts (`rank_load`'s note says why the JSON on stdin
    // is bounded though no shape reads it).
    if verb == "deploy-files" {
        return Shape::change(match a {
            [id, "single"] if plain_id(id) => FenceClass::ModelLoadOne,
            [id, "head" | "worker"] if plain_id(id) => FenceClass::ModelLoad,
            _ => FenceClass::Other,
        });
    }
    let test = |p: &str| in_test_dir(p, ctx.home);
    let own = |p: &str| in_test_dir(p, ctx.home) || in_own_dir(p, ctx.home);
    let name = |n: &str| !n.is_empty() && !n.contains('/') && n != ".." && n != ".";
    let ok = match (verb, a) {
        ("commit", [path, opts @ ..]) => {
            test(path) && opts.chunks(2).all(|kv| match kv {
                ["--from" | "--backups", p] => own(p),
                [_, _] => true,
                _ => false,
            })
        }
        // An item trashed from ~/sparkbox-test goes to ~/sparkbox-test/.sparkbox-trash, and
        // sparkjob.py puts it back only inside the folder; the home Trash puts things back
        // wherever they came from, so untrashing from it is `other`. The folder itself, and what's
        // already in its Trash, have no Trash inside the folder to go to: `other` too.
        ("trash", paths) => !paths.is_empty() && paths.iter().all(|p| test(p) && trashable(p, ctx.home)),
        ("untrash" | "rm", paths) => !paths.is_empty() && paths.iter().all(|p| test(p)),
        ("place", [from, to]) => test(from) && test(to),
        ("cp", [dir, to, _id, names @ ..]) => test(dir) && test(to) && names.iter().all(|n| name(n)),
        ("cleanup", [to, _id]) => test(to),
        ("sweep", [dir, ..]) => test(dir),
        ("delete", [root, path]) => test(root) && (name(path) || test(path)),
        // Jobs: a download's destination is in its spec on stdin, a cancel names no path.
        _ => false,
    };
    if ok {
        Shape::change(FenceClass::FilesTest)
    } else {
        Shape::change(FenceClass::Other)
    }
}

/// Under ~/sparkbox-test (as `~/…` or the node's real home), with no `..` anywhere.
fn in_test_dir(p: &str, home: Option<&str>) -> bool {
    under(p, "sparkbox-test", home)
}

/// Inside ~/sparkbox-test with a Trash of its own to go to: not the folder itself, nor what's in
/// its .sparkbox-trash already.
fn trashable(p: &str, home: Option<&str>) -> bool {
    let rest = p.strip_prefix("~/").or_else(|| home.filter(|h| !h.is_empty()).and_then(|h| p.strip_prefix(h.trim_end_matches('/'))?.strip_prefix('/')));
    let parts: Vec<&str> = rest.unwrap_or("").split('/').filter(|c| !c.is_empty() && *c != ".").collect();
    parts.len() > 1 && parts[0] == "sparkbox-test" && parts[1] != ".sparkbox-trash"
}

/// Under ~/.sparkbox: SparkBox's staging and backups.
fn in_own_dir(p: &str, home: Option<&str>) -> bool {
    under(p, ".sparkbox", home)
}

fn under(p: &str, dir: &str, home: Option<&str>) -> bool {
    if p.split('/').any(|c| c == "..") {
        return false;
    }
    let rest = match (p.strip_prefix("~/"), home) {
        (Some(r), _) => r,
        (None, Some(h)) if !h.is_empty() => match p.strip_prefix(h.trim_end_matches('/')).and_then(|r| r.strip_prefix('/')) {
            Some(r) => r,
            None => return false,
        },
        _ => return false,
    };
    rest == dir || rest.starts_with(&format!("{dir}/"))
}

/// A `--instance` value: release, dev (a short lowercase name, never an option).
fn instance_ok(i: &str) -> bool {
    i.len() <= 32 && i.starts_with(|c: char| c.is_ascii_lowercase()) && i.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn is_assignment(w: &str) -> bool {
    w.split_once('=').is_some_and(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !k.starts_with(|c: char| c.is_ascii_digit()))
}

/// `env LC_ALL=C sh -c '<inner>'`, `sh -c '<inner>'` and `bash -lc '<inner>'` are the inner
/// command. The shell may be a path (`/bin/sh`), and `-c` may sit anywhere among its options
/// (`-lc`, `-ec`, `-c -e`, `-e -c`, `-c --`, `-o errexit -c`): the script is the first operand
/// after them, as POSIX reads it (`classify::ShellArgs`). A shell that sources an rcfile (-i,
/// --rcfile, --init-file), or has operands after its script (its $0, $1…), isn't a plain wrapper.
fn unwrap_sh(cmd: &str) -> String {
    let mut cmd = cmd.to_string();
    for _ in 0..4 {
        let Some(segs) = segments(&cmd) else { break };
        let [w] = segs.as_slice() else { break };
        let w: Vec<&str> = w.iter().map(String::as_str).collect();
        let rest = match w.as_slice() {
            ["env", "LC_ALL=C", rest @ ..] => rest,
            rest => rest,
        };
        let Some((&shell, args)) = rest.split_first() else { break };
        if !matches!(shell.rsplit('/').next().unwrap_or(shell), "sh" | "bash" | "dash" | "zsh") {
            break;
        }
        let sh = classify::ShellArgs::of(args);
        match sh.operands.as_slice() {
            [inner] if sh.c && !sh.sources_rcfile => cmd = inner.to_string(),
            _ => break,
        }
    }
    cmd
}

/// A typed Fleet command that is, whole, Wi-Fi on or off: `[sudo [-n]] nmcli [-w N | --wait N]
/// <r|ra|rad|radi|radio> <wi|wif|wifi> on|off` (nmcli takes any prefix of its words), inside
/// Fleet's wrapper (`fleet::wrap`: `[env LC_ALL=C] timeout --signal=TERM --kill-after=5s <N>s sh -c
/// '…'`) or as it is. Wi-Fi, as the exit judges a typed command (C9, C-W). `w` alone is also
/// wwan's prefix and `radio all` turns off more than Wi-Fi: those stay Fleet commands.
pub fn typed_wifi(cmd: &str) -> bool {
    let inner = fleet_inner(cmd).unwrap_or_else(|| blank_trim(cmd).to_string());
    let Some(segs) = segments(&inner) else { return false };
    let [w] = segs.as_slice() else { return false };
    let w: Vec<&str> = w.iter().map(String::as_str).collect();
    let w = match w.as_slice() {
        ["sudo", "-n", rest @ ..] | ["sudo", rest @ ..] => rest,
        rest => rest,
    };
    let w = match w {
        ["nmcli", "-w" | "--wait", n, rest @ ..] if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => rest,
        ["nmcli", rest @ ..] => rest,
        _ => return false,
    };
    matches!(w, [obj, dev, "on" | "off"] if matches!(*obj, "r" | "ra" | "rad" | "radi" | "radio") && matches!(*dev, "wi" | "wif" | "wifi"))
}

/// What Fleet's wrapper runs: the script of `[env LC_ALL=C] timeout --signal=TERM
/// --kill-after=5s <N>s sh -c '<script>'`, or None for anything else.
fn fleet_inner(cmd: &str) -> Option<String> {
    let segs = segments(blank_trim(cmd))?;
    let [w] = segs.as_slice() else { return None };
    let w: Vec<&str> = w.iter().map(String::as_str).collect();
    let rest = match w.as_slice() {
        ["env", "LC_ALL=C", rest @ ..] => rest,
        rest => rest,
    };
    match rest {
        ["timeout", "--signal=TERM", "--kill-after=5s", limit, "sh", "-c", script] => {
            let secs = limit.strip_suffix('s')?;
            (!secs.is_empty() && secs.chars().all(|c| c.is_ascii_digit())).then(|| script.to_string())
        }
        _ => None,
    }
}

/// What a Files change through sparkjob.py would have done, in words, for its "Refused" line:
/// "Save notes.txt", "Move notes.txt to the Trash", "Delete 3 items", "Put back notes.txt",
/// "Copy a.txt to backup". None for a verb Files doesn't send.
pub fn files_words(args: &str) -> Option<String> {
    let segs = segments(args)?;
    let [w] = segs.as_slice() else { return None };
    let (verb, rest) = w.split_first()?;
    let items = |paths: &[String]| match paths {
        [one] => base_name(one),
        many => format!("{} items", many.len()),
    };
    Some(match (verb.as_str(), rest) {
        // C8: Files › New file, a commit that only creates.
        ("commit", [path, rest @ ..]) if rest.windows(2).any(|w| w[0] == "--expect" && w[1] == "absent") => format!("New file {}", base_name(path)),
        ("commit", [path, ..]) => format!("Save {}", base_name(path)),
        ("trash", paths) if !paths.is_empty() => format!("Move {} to the Trash", items(paths)),
        ("untrash", paths) if !paths.is_empty() => format!("Put back {}", items(paths)),
        ("rm", paths) if !paths.is_empty() => format!("Delete {}", items(paths)),
        ("delete", [_root, path]) => format!("Delete {}", base_name(path)),
        ("place", [_stage, to]) => format!("Copy files to {}", base_name(to)),
        ("cp", [_dir, to, _id, names @ ..]) if !names.is_empty() => format!("Copy {} to {}", items(names), base_name(to)),
        ("cleanup", [to, _id]) => format!("Clean up a copy to {}", base_name(to)),
        ("sweep", [dir, ..]) => format!("Clean up copies in {}", base_name(dir)),
        _ => return None,
    })
}

/// The Files window's own shell commands, in words, for their "Refused" line: "Rename a.txt to
/// b.txt", "New folder new", "Change the permissions of notes.txt". None for anything else.
pub fn file_op_words(cmd: &str) -> Option<String> {
    if let Some((src, to)) = rename_literal(cmd) {
        return Some(format!("Rename {} to {}", base_name(&src), base_name(&to)));
    }
    let segs = segments(cmd)?;
    let [w] = segs.as_slice() else { return None };
    let w: Vec<&str> = w.iter().map(String::as_str).collect();
    match w.as_slice() {
        ["mkdir", "--", p] => Some(format!("New folder {}", base_name(p))),
        ["chmod", "--", _mode, p] => Some(format!("Change the permissions of {}", base_name(p))),
        _ => None,
    }
}

/// A path's last part ("notes.txt"), or the path itself when it has none ("/").
fn base_name(p: &str) -> String {
    p.trim_end_matches('/').rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or(p).to_string()
}

/// Words put back into one command for the classifier, each quoted again where it needs it, and
/// each redirection as the redirection it was (its target quoted).
fn join_quoted(words: &[String]) -> String {
    let word = |w: &str| if w.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:,+%@~$()&{}*".contains(c)) && !w.is_empty() { w.to_string() } else { super::sh_quote(w) };
    words
        .iter()
        .map(|w| match redirection(w) {
            Some(r) => {
                // The stream number and operator (`2>`, `>>`, `>&`), then the target.
                let digits = r.len() - r.trim_start_matches(|c: char| c.is_ascii_digit()).len();
                let op = digits + if ["<&", ">&", ">>"].iter().any(|o| r[digits..].starts_with(o)) { 2 } else { 1 };
                format!("{}{}", &r[..op], word(&r[op..]))
            }
            None => word(w),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Words that open or close a block, or change how a pipeline runs, where a command would start:
/// no shape follows what they do (T2 blocker 2).
const RESERVED: [&str; 20] = ["if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "in", "function", "select", "time", "coproc", "!", "[[", "]]"];

/// The command split the way sh splits it into simple commands (at `;`, `&&`, `||`, `|`, `&`
/// and newlines) and words, with quotes removed. A `$(…)` or backtick part stays one word, and so
/// do `$NAME`, `${NAME}`, `$?`, `$1` (`expands` flags them). A redirection — `>`, `>>`, `<`, each
/// with a stream number before it or not, and its target; `>&N`, `N>&M` — is one word marked
/// `REDIRECT` (`> f` and `>f` alike are `REDIRECT` `>f`), so a quoted `'>f'` stays an argument and
/// a shape that takes any arguments can refuse what writes a file (`redirects`). None when the
/// quoting doesn't balance, and on anything else the shell would read some other way than these
/// words (T2 blocker 2), outside single quotes: `(` or `)` outside a `$(…)`, any `{` or `}` outside
/// a `${NAME}`, a `#` that starts a word, `<<`, `<<<`, `<>`, `>|`, `|&`, `&>`, bash's `>&file`, a
/// redirection run into the word before it (`x>f`), `;;`, `<(`, `>(`, `$((`, `$[`, `$'`, `$"`, a
/// `${…}` that isn't a plain name, a reserved word (`RESERVED`) where a command starts, an ASCII
/// control character other than tab and newline, and a non-ASCII space. dash (Ubuntu's /bin/sh)
/// and bash agree on everything this keeps.
pub fn segments(cmd: &str) -> Option<Vec<Vec<String>>> {
    split_words(cmd, true)
}

/// Marks a word `segments` made of a redirection: a control character, which it refuses in a
/// command, so no quoted text can carry it.
pub const REDIRECT: char = '\u{1}';

/// A redirection's text (`>f`, `2>/dev/null`, `>&2`) when `w` is one (`segments`).
pub fn redirection(w: &str) -> Option<&str> {
    w.strip_prefix(REDIRECT)
}

/// A redirection that only quiets or merges output: `>/dev/null`, `2>/dev/null`, `2>&1`, `>&2`.
fn quiet(r: &str) -> bool {
    matches!(r, ">/dev/null" | "2>/dev/null" | "2>&1" | ">&2")
}

/// Any redirection in `words` but a quiet one: a shape that takes any arguments refuses these,
/// which open a file the shape never named (`tar … -- x >~/.bashrc`).
fn redirects<S: AsRef<str>>(words: &[S]) -> bool {
    words.iter().filter_map(|w| redirection(w.as_ref())).any(|r| !quiet(r))
}

/// `words` without its quiet redirections, for a shape that takes any arguments; None when one
/// opens a file (`redirects`).
fn plain_words(words: &[String]) -> Option<Vec<&str>> {
    (!redirects(words)).then(|| words.iter().map(String::as_str).filter(|w| redirection(w).is_none()).collect())
}

/// `segments`, or with `strict` off the words as far as they go (constructs it doesn't follow
/// left in the words as characters): what check (d) looks through for container names, which
/// fails no command for its syntax.
fn split_words(cmd: &str, strict: bool) -> Option<Vec<Vec<String>>> {
    let chars: Vec<char> = cmd.chars().collect();
    let mut segs: Vec<Vec<String>> = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    // Strict: a redirection's operator (`2>`), waiting for its target, the next word.
    let mut redirect: Option<String> = None;
    let mut i = 0;
    // None for a target that would read as part of its operator (`> '>f'` isn't `>>f`).
    let end_word = |words: &mut Vec<String>, word: &mut String, in_word: &mut bool, redirect: &mut Option<String>| -> Option<()> {
        if *in_word {
            let w = std::mem::take(word);
            words.push(match redirect.take() {
                Some(_) if w.starts_with(['<', '>', '&']) => return None,
                Some(op) => format!("{REDIRECT}{op}{w}"),
                None => w,
            });
            *in_word = false;
        }
        Some(())
    };
    // A control character, or a space that isn't the shell's.
    let unfollowed = |c: char| (c.is_ascii_control() && !matches!(c, '\t' | '\n')) || (!c.is_ascii() && (c.is_whitespace() || c == '\u{feff}'));
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' => {
                let close = chars[i + 1..].iter().position(|&x| x == '\'')? + i + 1;
                word.extend(&chars[i + 1..close]);
                in_word = true;
                i = close + 1;
                continue;
            }
            '"' => {
                let mut j = i + 1;
                loop {
                    match *chars.get(j)? {
                        '"' => break,
                        '\\' if matches!(chars.get(j + 1), Some('"' | '\\' | '$' | '`')) => {
                            word.push(chars[j + 1]);
                            j += 2;
                        }
                        '\\' if chars.get(j + 1) == Some(&'\n') => j += 2,
                        '$' => j = dollar(&chars, j, true, strict, &mut word)?,
                        '`' => {
                            let close = chars[j + 1..].iter().position(|&x| x == '`')? + j + 1;
                            word.extend(&chars[j..=close]);
                            j = close + 1;
                        }
                        '(' | ')' | '{' | '}' if strict => return None,
                        x if strict && unfollowed(x) => return None,
                        x => {
                            word.push(x);
                            j += 1;
                        }
                    }
                }
                in_word = true;
                i = j + 1;
                continue;
            }
            '\\' => {
                if let Some(&n) = chars.get(i + 1) {
                    if n != '\n' {
                        word.push(n);
                        in_word = true;
                    }
                }
                i += 2;
                continue;
            }
            '$' => {
                i = dollar(&chars, i, false, strict, &mut word)?;
                in_word = true;
                continue;
            }
            '`' => {
                let close = chars[i + 1..].iter().position(|&x| x == '`')? + i + 1;
                word.extend(&chars[i..=close]);
                in_word = true;
                i = close + 1;
                continue;
            }
            '(' | ')' | '{' | '}' if strict => return None,
            // Not strict: a subshell's parentheses end a command as `;` does.
            '(' | ')' => {
                end_word(&mut words, &mut word, &mut in_word, &mut redirect)?;
                if !words.is_empty() {
                    segs.push(std::mem::take(&mut words));
                }
            }
            '#' if strict && !in_word => return None,
            // `<<` and `<<<` fall to the redirection below: their second `<` finds one already open.
            '<' if strict && matches!(chars.get(i + 1), Some('>' | '(')) => return None,
            '>' if strict && matches!(chars.get(i + 1), Some('|' | '(')) => return None,
            '|' if strict && chars.get(i + 1) == Some(&'&') => return None,
            '&' if strict && chars.get(i + 1) == Some(&'>') => return None,
            ';' if strict && chars.get(i + 1) == Some(&';') => return None,
            // A redirection: its stream number (one digit alone before it, unquoted: dash reads
            // `12>f` as the word 12 and `>f`), the operator, then its target (the next word) or the
            // stream it copies (`>&2`).
            '<' | '>' if strict => {
                if redirect.is_some() {
                    return None; // `> >f`
                }
                let mut op = String::new();
                if in_word {
                    // The word began right before the operator: it's that one character.
                    let digit = chars[i - 1].is_ascii_digit() && (i == 1 || matches!(chars[i - 2], ' ' | '\t' | '\n' | ';' | '|' | '&'));
                    if !digit {
                        return None; // `x>f`: dash writes f, whatever the shape makes of x
                    }
                    op = std::mem::take(&mut word);
                    in_word = false;
                }
                op.push(c);
                if c == '>' && chars.get(i + 1) == Some(&'>') {
                    op.push('>');
                    i += 1;
                }
                if chars.get(i + 1) == Some(&'&') {
                    // Only a stream number: bash writes the file for `>&file` (dash refuses it).
                    let digits: String = chars[i + 2..].iter().take_while(|d| d.is_ascii_digit()).collect();
                    let end = i + 2 + digits.len();
                    if digits.is_empty() || op.ends_with(">>") || chars.get(end).is_some_and(|d| !matches!(d, ' ' | '\t' | '\n' | ';' | '|' | '&')) {
                        return None;
                    }
                    words.push(format!("{REDIRECT}{op}&{digits}"));
                    i = end;
                    continue;
                }
                redirect = Some(op);
            }
            ' ' | '\t' => end_word(&mut words, &mut word, &mut in_word, &mut redirect)?,
            ';' | '\n' | '|' | '&' => {
                end_word(&mut words, &mut word, &mut in_word, &mut redirect)?;
                if redirect.is_some() {
                    return None; // `cmd >` with no file
                }
                if !words.is_empty() {
                    segs.push(std::mem::take(&mut words));
                }
                if matches!(c, '|' | '&') && chars.get(i + 1) == Some(&c) {
                    i += 1;
                }
            }
            c if strict && unfollowed(c) => return None,
            _ => {
                word.push(c);
                in_word = true;
                // `>&2`, `2>&1`: keep the redirection in its word.
                if c == '>' && chars.get(i + 1) == Some(&'&') {
                    word.push('&');
                    i += 1;
                }
            }
        }
        i += 1;
    }
    end_word(&mut words, &mut word, &mut in_word, &mut redirect)?;
    if redirect.is_some() {
        return None;
    }
    if !words.is_empty() {
        segs.push(words);
    }
    if strict && segs.iter().any(|s| RESERVED.contains(&s[0].as_str())) {
        return None;
    }
    Some(segs)
}

/// A `$` at `chars[i]` (in double quotes when `quoted`), added to `word`: a `$(…)` whole, a
/// `${NAME}`, or the `$` alone (a `$NAME`, `$?` or `$1` follows as plain characters). Strict, it
/// refuses arithmetic, `$'…'`, `$"…"` and a `${…}` that isn't a plain name. The index after it.
fn dollar(chars: &[char], i: usize, quoted: bool, strict: bool, word: &mut String) -> Option<usize> {
    let plain = |s: &[char]| s.first().is_some_and(|c| c.is_ascii_alphabetic() || *c == '_') && s.iter().all(|c| c.is_ascii_alphanumeric() || *c == '_');
    match chars.get(i + 1) {
        Some('(') if strict && chars.get(i + 2) == Some(&'(') => None,
        Some('(') => {
            let close = matching(chars, i + 1)?;
            word.extend(&chars[i..=close]);
            Some(close + 1)
        }
        Some('[') if strict => None,
        Some('\'' | '"') if strict && !quoted => None,
        Some('{') if strict => {
            let close = chars[i + 2..].iter().position(|&x| x == '}')? + i + 2;
            plain(&chars[i + 2..close]).then(|| {
                word.extend(&chars[i..=close]);
                close + 1
            })
        }
        _ => {
            word.push('$');
            Some(i + 1)
        }
    }
}

/// The `)` that closes the `(` at `open`, skipping quotes and nested parentheses.
fn matching(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0;
    let mut i = open;
    while i < chars.len() {
        match chars[i] {
            '\'' => i = chars[i + 1..].iter().position(|&x| x == '\'')? + i + 1,
            '"' => {
                let mut j = i + 1;
                while chars.get(j)? != &'"' {
                    j += if chars[j] == '\\' { 2 } else { 1 };
                }
                i = j;
            }
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::{root_install_command, root_sparkjob_command};
    use crate::ops::classify::contracts;
    use std::collections::BTreeMap;

    const LIB: &str = "/usr/local/lib/sparkbox";
    const SHA: &str = "0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f";

    fn container(id: &str, name: &str, labels: &[(&str, &str)]) -> Container {
        Container { id: id.into(), name: name.into(), labels: labels.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>(), ..Default::default() }
    }

    fn shape(kind: ExecKind, cmd: &str) -> Shape {
        let containers = [
            container("0123456789abcdef0123", "glm53", &[]),
            container("aaaaaaaaaaaa1111", "gemma-cell2", &[("sparkbox.deployment", "d1"), ("sparkbox.instance", "release")]),
            container("bbbbbbbbbbbb2222", "sparkbox-test-sleep", &[("sparkbox.test", "1")]),
        ];
        class_of(kind, cmd, &Ctx { containers: &containers, home: Some("/home/sparkuser"), root_lib: LIB, sparkjob_sha: SHA })
    }

    fn class(kind: ExecKind, cmd: &str) -> &'static str {
        match shape(kind, cmd) {
            Shape::Read => "read",
            Shape::Bookkeeping => "bookkeeping",
            Shape::Change(c, _) => c.id(),
        }
    }

    fn items(kind: ExecKind, cmd: &str) -> Vec<Item> {
        match shape(kind, cmd) {
            Shape::Change(_, items) => items,
            _ => Vec::new(),
        }
    }

    /// Round 3's single-node load (`plan::single_command` as tagged `p1-r3`): SparkBox's Ray image
    /// by its tag, `--shm-size 16g`, no caches. `other` since the end of round 4 (a negative row).
    fn r3_load(keyed: bool) -> String {
        let run = "docker run -d --name sparkbox-d1 --label sparkbox.instance=release --label sparkbox.deployment=d1 --label sparkbox.role=single --label sparkbox.rank=0 --label sparkbox.model=gemma-3-27b-it --network host --ipc host --shm-size 16g --gpus all --restart no";
        let model = "-v /home/sparkuser/models/google/gemma-3-27b-it:/home/sparkuser/models/google/gemma-3-27b-it:ro";
        if keyed {
            format!("umask 077 && mkdir -p ~/.sparkbox/keys && rm -f ~/.sparkbox/keys/d1 && IFS= read -r k && printf '%s' \"$k\" > ~/.sparkbox/keys/d1 && unset k && chmod 0400 ~/.sparkbox/keys/d1 && {run} -v \"$HOME/.sparkbox/keys/d1:/run/sparkbox/key:ro\" {model} --entrypoint sh sparkbox/vllm-ray:26.05-py3 -c 'VLLM_API_KEY=\"$(cat /run/sparkbox/key)\" exec vllm \"serve\" \"/home/sparkuser/models/google/gemma-3-27b-it\" \"--port\" \"8000\"'")
        } else {
            format!("{run} {model} --entrypoint vllm sparkbox/vllm-ray:26.05-py3 'serve' '/home/sparkuser/models/google/gemma-3-27b-it' '--port' '8000'")
        }
    }

    /// Round 4's single-node load, C-L: a key preamble, the options in the words and a shell for
    /// the key. Nothing sends it now, so it's a negative row: a shape nothing sends is a hole.
    fn r4_load(keyed: bool) -> String {
        let id = format!("sha256:{}", "a".repeat(64));
        let model = "/home/sparkuser/models/google/gemma-3-27b-it";
        let run = "docker run -d --name sparkbox-d1 --label sparkbox.instance=release --label sparkbox.deployment=d1 --label sparkbox.role=single --label sparkbox.rank=0 --label sparkbox.model=gemma-3-27b-it --label sparkbox.image=vllm/vllm-openai:glm53-flash-arm64-cu130 --network host --ipc host --gpus all --ulimit memlock=-1 --ulimit stack=67108864 --restart no -e HF_HUB_OFFLINE=1 -e VLLM_ENGINE_READY_TIMEOUT_S=3600 -v \"$HOME/.cache/vllm:/root/.cache/vllm\" -v \"$HOME/.cache/flashinfer:/root/.cache/flashinfer\" -v \"$HOME/.triton:/root/.triton\"";
        if keyed {
            format!("umask 077 && mkdir -p ~/.sparkbox/keys && rm -f ~/.sparkbox/keys/d1 && IFS= read -r k && printf '%s' \"$k\" > ~/.sparkbox/keys/d1 && unset k && chmod 0400 ~/.sparkbox/keys/d1 && mkdir -p ~/.cache/vllm ~/.cache/flashinfer ~/.triton && {run} -v \"$HOME/.sparkbox/keys/d1:/run/sparkbox/key:ro\" -v {model}:{model}:ro --entrypoint sh {id} -c 'VLLM_API_KEY=\"$(cat /run/sparkbox/key)\" exec vllm \"serve\" \"{model}\" \"--port\" \"8000\"'")
        } else {
            format!("mkdir -p ~/.cache/vllm ~/.cache/flashinfer ~/.triton && {run} -v {model}:{model}:ro --entrypoint vllm {id} 'serve' '{model}' '--port' '8000'")
        }
    }

    /// The command-shape table: how Live runs it, the command, its class.
    fn positive_rows() -> Vec<(ExecKind, String, &'static str)> {
        use ExecKind::*;
        // C1′'s detached upgrade (ops/updates.rs `detached_command`), its package pinned.
        let detached = crate::ops::updates::detached_command("sparkbox-upgrade-t1", "/var/log/sparkbox", "/var/log/sparkbox/upgrade-t1.log", &["libfoo=1.2-3".into()]);
        vec![
            // sudo-probe: `true`, and anything the classifier calls a read, run as root.
            (Sudo, "true".into(), "sudo-probe"),
            (Sudo, "cat /etc/netplan/40-cx7.yaml".into(), "sudo-probe"),
            (Sudo, "journalctl -u ollama.service -n 25 --no-pager".into(), "sudo-probe"),
            (SparkjobRoot, "read '/etc/netplan/40-cx7.yaml'".into(), "sudo-probe"),
            (Sudo, root_sparkjob_command(LIB, SHA, "read '/etc/netplan/40-cx7.yaml'"), "sudo-probe"),
            (Sudo, root_sparkjob_command(LIB, SHA, "df /"), "sudo-probe"),
            // root-helper: putting root's copy of sparkjob.py in place.
            (SudoInput, root_install_command(LIB, SHA), "root-helper"),
            // service
            (Sudo, "systemctl restart -- 'earlyoom.service'".into(), "service"),
            (Sudo, "systemctl stop -- 'cups.service'".into(), "service"),
            (Sudo, "systemctl enable --now -- 'earlyoom.service'".into(), "service"),
            (Sudo, "systemctl daemon-reload".into(), "service"),
            // power-one
            (Sudo, "systemctl reboot".into(), "power-one"),
            (Sudo, "systemctl poweroff".into(), "power-one"),
            (Sudo, "systemctl halt".into(), "power-one"),
            (Sudo, "reboot".into(), "power-one"),
            (Sudo, crate::ops::power_command("reboot", 5), "power-one"),
            (Sudo, crate::ops::power_command("poweroff", 2), "power-one"),
            // apt
            (Sudo, "apt-get update".into(), "apt-refresh"),
            (Sudo, detached.clone(), "apt-one"),
            (Sudo, "apt-mark hold 'nvidia-driver-580-open' 'libnvidia-compute-580'".into(), "apt-hold"),
            (Sudo, "apt-mark unhold 'nvidia-driver-580-open'".into(), "apt-hold"),
            // wifi, page cache
            (Sudo, "nmcli radio wifi off".into(), "wifi"),
            (Sudo, "nmcli radio wifi on".into(), "wifi"),
            (Sudo, "sh -c 'sync; echo 3 > /proc/sys/vm/drop_caches'".into(), "drop-caches"),
            (Sudo, "env LC_ALL=C sh -c 'sync && echo 3 > /proc/sys/vm/drop_caches'".into(), "drop-caches"),
            // Containers: a test container, a load's container, anyone else's.
            (ContainerAction, "docker stop bbbbbbbbbbbb2222".into(), "container-test"),
            (ContainerAction, "docker rm sparkbox-test-other".into(), "container-test"),
            (ContainerAction, "docker stop aaaaaaaaaaaa1111".into(), "model-unload"),
            (ContainerAction, "docker restart 0123456789abcdef0123".into(), "container-owner"),
            (Run, "docker rm glm53".into(), "container-owner"),
            (Run, "docker kill -s TERM glm53".into(), "container-owner"),
            (Run, "docker stop -t 30 gemma-cell2".into(), "model-unload"),
            (Run, "docker rm -v 0123456789abcdef0123".into(), "container-owner"),
            (Run, "docker pause glm53".into(), "container-owner"),
            (Run, "docker unpause glm53".into(), "container-owner"),
            // Loads and test containers.
            // C-L2 on one node, with its key and without, for either build and with Keep running.
            (Run, contracts::C_L.into(), "model-load-one"),
            (Run, contracts::c_l_no_key(), "model-load-one"),
            (Run, contracts::C_L.replace("sparkbox.instance=release", "sparkbox.instance=dev").replace("--restart no", "--restart unless-stopped"), "model-load-one"),
            // On several nodes: a head and a worker, privileged or not, any rank.
            (Run, contracts::C_L_HEAD.into(), "model-load"),
            (Run, contracts::C_L_WORKER.into(), "model-load"),
            (Run, contracts::C_L_HEAD.replace(" --privileged", ""), "model-load"),
            (Run, contracts::C_L_WORKER.replace(" --privileged", ""), "model-load"),
            (Run, contracts::C_L_WORKER.replace("sparkbox-d1-r1", "sparkbox-d1-r3").replace("sparkbox.rank=1", "sparkbox.rank=3").replace("sparkbox.parallel=2x1", "sparkbox.parallel=2x2"), "model-load"),
            (Run, contracts::C_L_HEAD.replace(r#" -v "$HOME/.sparkbox/keys/d1:/run/sparkbox/key:ro""#, "").replace(" /run/sparkbox/launch.py --key", " /run/sparkbox/launch.py"), "model-load"),
            (Run, contracts::C_L_HEAD.replace(" --label sparkbox.maxlen=32768", ""), "model-load"),
            (Run, contracts::C_L_WORKER.replace(" --label sparkbox.maxlen=32768", ""), "model-load"),
            // A load's launch files, by the role they start.
            (Sparkjob, "deploy-files d1 single".into(), "model-load-one"),
            (Sparkjob, "deploy-files d1 head".into(), "model-load"),
            (Sparkjob, "deploy-files d1 worker".into(), "model-load"),
            (Sparkjob, "deploy-files 'gemma-3_27b.it' worker".into(), "model-load"),
            (Run, format!("{SPARKJOB}deploy-files d1 single"), "model-load-one"),
            (ExecIo, format!("{SPARKJOB}deploy-files d1 worker"), "model-load"),
            (Run, "docker run -d --name sparkbox-test-sleep --label sparkbox.test=1 alpine sleep 600".into(), "container-test"),
            // The short list of options a test container may have.
            (Run, "docker run -dit --rm --init --name sparkbox-test-x --label sparkbox.test=1 -l a=b --network none --memory 64m --cpus=0.5 --pids-limit 64 -e A=1 -w /tmp --tmpfs /scratch busybox sleep 1".into(), "container-test"),
            (Run, "docker run -d --name sparkbox-test-x --label=sparkbox.test=1 --net=bridge busybox sleep 1".into(), "container-test"),
            (Run, crate::models::plan::unload_command("d1"), "model-unload"),
            // C-U2 and C-S.
            (Run, contracts::C_U.into(), "model-unload"),
            (Run, contracts::C_S.into(), "model-unload"),
            (Run, contracts::C_U.replace("instance=release", "instance=dev"), "model-unload"),
            (Run, contracts::C_S.replace("role=head", "role=worker"), "model-unload"),
            // Processes, without sudo.
            (Run, "kill -s TERM 4242".into(), "process-test"),
            (Run, "kill -9 4242".into(), "process-test"),
            (Run, "renice -n 5 -p 4242".into(), "process-test"),
            // Files under ~/sparkbox-test; anywhere else is `other`.
            (Sparkjob, "commit '/home/sparkuser/sparkbox-test/a.txt' --sha256 ab --expect '-' --backups '/home/sparkuser/.sparkbox/backups' --nonce 7".into(), "files-test"),
            (Sparkjob, "trash '/home/sparkuser/sparkbox-test/a.txt' '~/sparkbox-test/b'".into(), "files-test"),
            (Sparkjob, "untrash '/home/sparkuser/sparkbox-test/.sparkbox-trash/files/a.txt'".into(), "files-test"),
            (Sparkjob, "untrash '~/sparkbox-test/.sparkbox-trash/files/a 2.txt'".into(), "files-test"),
            (Sparkjob, "rm '/home/sparkuser/sparkbox-test/old'".into(), "files-test"),
            (Sparkjob, "place '/home/sparkuser/sparkbox-test/.sparkbox-incoming-1' '/home/sparkuser/sparkbox-test/dest'".into(), "files-test"),
            (Sparkjob, "cp '/home/sparkuser/sparkbox-test' '/home/sparkuser/sparkbox-test/copy' 7abc 'a.txt' 'b'".into(), "files-test"),
            (Sparkjob, "cleanup '/home/sparkuser/sparkbox-test/copy' 7abc".into(), "files-test"),
            (Sparkjob, "sweep '/home/sparkuser/sparkbox-test' 7abc".into(), "files-test"),
            (Sparkjob, "delete '/home/sparkuser/sparkbox-test' 'm1'".into(), "files-test"),
            (Run, "python3 ~/.sparkbox/sparkjob.py rm '/home/sparkuser/sparkbox-test/old'".into(), "files-test"),
            // The lease is bookkeeping; sparkjob's reads are reads.
            (Sparkjob, "lease claim 'dev' 'g0' 'Studio' 45".into(), "bookkeeping"),
            (Sparkjob, "lease release 'dev'".into(), "bookkeeping"),
            (Run, "python3 ~/.sparkbox/sparkjob.py lease claim 'dev' 'g0' 'Studio' 45".into(), "bookkeeping"),
            (Sparkjob, "ls '/home/sparkuser'".into(), "read"),
            (Sparkjob, "read '/home/sparkuser/notes.txt'".into(), "read"),
            (Sparkjob, "df /".into(), "read"),
            (Sparkjob, "list".into(), "read"),
            (Sparkjob, "list --look".into(), "read"),
            (Run, "python3 ~/.sparkbox/sparkjob.py scan '/home/sparkuser/models'".into(), "read"),
            // The quick readiness check reads; the full one runs the vLLM image (below).
            (Sparkjob, "check '~/models' 'nvcr.io/nvidia/vllm:26.05-py3' 'nvcr.io/nvidia/vllm:26.05-py3-ray' quick".into(), "read"),
            (Run, "python3 ~/.sparkbox/sparkjob.py check '~/models' 'img' 'img-ray' quick".into(), "read"),
            (Sparkjob, "check '~/models' 'img' 'img-ray' quick --instance 'dev'".into(), "read"),
            // The full check runs the image in a labelled test container of this build's.
            (Sparkjob, "check '~/models' 'nvcr.io/nvidia/vllm:26.05-py3' 'nvcr.io/nvidia/vllm:26.05-py3-ray' --instance 'dev'".into(), "container-test"),
            (Run, "python3 ~/.sparkbox/sparkjob.py check '~/models' 'img' 'img-ray' --instance 'release'".into(), "container-test"),
            (ExecIo, "python3 ~/.sparkbox/sparkjob.py check '~/models' 'img' 'img-ray' --instance 'dev'".into(), "container-test"),
            // Shells: a login shell, a shell in a container, a write over exec_io.
            (Pty, "".into(), "shell"),
            (Pty, "docker exec -it glm53 bash".into(), "shell"),
            (ExecIo, "mkdir -p -- '/home/sparkuser/.sparkbox-incoming-1' && tar -C '/home/sparkuser/.sparkbox-incoming-1' -xf -".into(), "shell"),
            (ExecIo, "python3 ~/.sparkbox/sparkjob.py cp '/home/sparkuser/sparkbox-test' '/home/sparkuser/sparkbox-test/b' 7 'a'".into(), "files-test"),
            // T2's own file ops in ~/sparkbox-test: New folder, permission change, rename, and the
            // tar copies (download reads, an upload into a staging folder under it is files-test).
            (Run, "mkdir -- '/home/sparkuser/sparkbox-test/new'".into(), "files-test"),
            (Run, "chmod -- 600 '/home/sparkuser/sparkbox-test/notes.txt'".into(), "files-test"),
            (Run, crate::files::rename_command("/home/sparkuser/sparkbox-test/a.txt", "/home/sparkuser/sparkbox-test/b.txt"), "files-test"),
            (ExecIo, "tar -C '/home/sparkuser/sparkbox-test' -cvf - -- 'notes.txt'".into(), "read"),
            (ExecIo, "mkdir -p -- '/home/sparkuser/sparkbox-test/.sparkbox-incoming-1' && tar -C '/home/sparkuser/sparkbox-test/.sparkbox-incoming-1' -xf -".into(), "files-test"),
            // A follower over a terminal, a stream of a log: reads.
            (Sudo, "sfdisk -l /dev/nvme0n1".into(), "sudo-probe"),
            (Sudo, "parted -s /dev/nvme0n1 print".into(), "sudo-probe"),
            (Pty, "journalctl -f -u 'ollama.service'".into(), "read"),
            (Pty, "journalctl -f -n 300 --no-hostname -u ssh.service".into(), "read"),
            (Pty, "journalctl -f --since -10min --no-hostname -u ssh.service".into(), "read"),
            (Pty, "journalctl -f -b --no-hostname -u ssh.service".into(), "read"),
            (Pty, "docker logs -f --tail 500 gemma".into(), "read"),
            (Pty, "docker logs -f --since 10m gemma".into(), "read"),
            (Stream, "journalctl -f -n 200 --no-pager".into(), "read"),
            (DirectTcpip, "127.0.0.1:11000".into(), "shell"),
            // SparkBox's own folder: bookkeeping.
            (Run, "mkdir -p ~/.sparkbox".into(), "bookkeeping"),
            (Run, "mkdir -p ~/.sparkbox/jobs ~/.sparkbox/keys".into(), "bookkeeping"),
            (Run, "mkdir -p ~/.sparkbox/a..b".into(), "bookkeeping"),
            // This round's commands (the round plans' Contracts, byte for byte): C1′ pinned, with the
            // download limit and in a test's log folder.
            (Sudo, contracts::C1_PINNED.into(), "apt-one"),
            (Sudo, contracts::C1_PINNED.replace("-o Dpkg::Options::=--force-confold", "-o Dpkg::Options::=--force-confold -o Acquire::http::Dl-Limit=500 -o Acquire::https::Dl-Limit=500"), "apt-one"),
            (Sudo, contracts::c1_pinned_in(&format!("{LIB}/logs"), &["libfoo=1.2-3", "libbar:arm64=2:1.0~rc1+dfsg-1ubuntu0.1"]), "apt-one"),
            (Sudo, contracts::C3.into(), "apt-refresh"),
            (Sudo, contracts::C4_HOLD.into(), "apt-hold"),
            (Sudo, contracts::C4_UNHOLD.into(), "apt-hold"),
            (Sudo, contracts::C5_SUDO_FWUPD.into(), "sudo-probe"),
            (Run, contracts::c7("dev"), "container-test"),
            (Run, contracts::c7("release"), "container-test"),
            (Sparkjob, contracts::c8("/home/sparkuser/sparkbox-test/new.txt"), "files-test"),
            (Sparkjob, contracts::c8("~/sparkbox-test/new.txt"), "files-test"),
            (Run, format!("python3 ~/.sparkbox/sparkjob.py {}", contracts::c8("~/sparkbox-test/new.txt")), "files-test"),
            (Probe, contracts::C2_ONE.into(), "read"),
            (Probe, contracts::C2_EVERYTHING.into(), "read"),
            (Probe, contracts::c2_dl_limit(), "read"),
            (Probe, contracts::C4_PRESET_READ.into(), "read"),
            (Probe, contracts::check_script(), "read"),
            (Probe, contracts::STAMPS.into(), "read"),
            (Probe, contracts::C6_KERNEL.into(), "read"),
            (Probe, contracts::C6_STEP_B.into(), "read"),
            // Round 4's reads: C2′, C-P, C-SH, C-R's models list (its key on stdin) and protocol probe.
            (Probe, contracts::C2_PINNED.into(), "read"),
            (Probe, contracts::C_P.into(), "read"),
            (Probe, contracts::C_SH.into(), "read"),
            (Run, contracts::R_MODELS.into(), "read"),
            (Probe, contracts::R_OPENAPI.into(), "read"),
            (Run, "cat '/var/log/sparkbox/dpkg-before-t1.txt'".into(), "read"),
            (Probe, "needrestart -b -r l".into(), "read"),
            // `sh -c`, its options in any order: what it runs, as root too.
            (Sudo, "sh -c -e 'systemctl reboot'".into(), "power-one"),
            (Sudo, "bash -c -- 'systemctl daemon-reload'".into(), "service"),
            (Sudo, "env LC_ALL=C sh -e -c 'nmcli radio wifi off'".into(), "wifi"),
            (Sudo, "sh -c -o errexit 'sync; echo 3 > /proc/sys/vm/drop_caches'".into(), "drop-caches"),
            // A `$(` the shell doesn't expand (single quotes) is part of a name like any other.
            (Run, "mkdir -- '/home/sparkuser/sparkbox-test/$(x)'".into(), "files-test"),
        ]
    }

    #[test]
    fn download_tool_inventory_is_an_exact_read_and_jobs_remain_guarded() {
        let ctx = Ctx::default();
        assert_eq!(class_of(ExecKind::Sparkjob, "download-tools", &ctx), Shape::Read);
        assert_eq!(class_of(ExecKind::Sparkjob, "disk-preflight", &ctx), Shape::Read);
        assert_eq!(class_of(ExecKind::Sparkjob, "chain-preflight", &ctx), Shape::Read);
        assert_ne!(class_of(ExecKind::Sparkjob, "chain-preflight extra", &ctx), Shape::Read);
        for args in ["disk-preflight extra", "disk-preflight > /tmp/out", "disk-preflight; touch /tmp/out"] {
            assert_ne!(class_of(ExecKind::Sparkjob, args, &ctx), Shape::Read);
        }
        for args in ["download-tools extra", "download-tools > /tmp/out", "download-tools; touch /tmp/out", "start download dl-tool"] {
            assert!(matches!(class_of(ExecKind::Sparkjob, args, &ctx), Shape::Change(FenceClass::Other, _)), "{args}");
        }
    }

    #[test]
    fn reviewed_system_install_keeps_every_pin_in_the_fence() {
        for cmd in [crate::ops::protected_updates::RECOVERY_SETTING, crate::ops::protected_updates::RECOVERY_GRUB, crate::ops::protected_updates::NEXT_GRUB, crate::ops::protected_updates::CONTINUED_GRUB, crate::ops::protected_updates::RECOVERY_1024_GRUB] {
            assert_eq!(class(ExecKind::Sudo, cmd), "sudo-probe");
            assert_ne!(class(ExecKind::Sudo, &format!("{cmd}; chmod 644 /boot/grub/grub.cfg")), "sudo-probe");
        }
        let plan = crate::ops::protected_updates::reviewed();
        let pins: Vec<_> = plan.iter().map(|p| format!("{}={}", p.name, p.to)).collect();
        let opts = crate::ops::updates::UpgradeOpts { protected: true, ..Default::default() };
        let command = |p: &[String]| crate::ops::updates::detached_command_opts("sparkbox-upgrade-t1", "/var/log/sparkbox", "/var/log/sparkbox/upgrade-t1.log", p, &opts);
        let cmd = command(&pins);
        assert_eq!(class(ExecKind::Sudo, &cmd), "apt-one");
        assert_eq!(items(ExecKind::Sudo, &cmd), plan.iter().map(|p| vec![p.name.clone()]).collect::<Vec<_>>());
        assert_ne!(class(ExecKind::Sudo, &command(&pins[..20])), "apt-one");
        assert_ne!(class(ExecKind::Sudo, &cmd.replace("1.0-2", "1.0-3")), "apt-one");
        assert_ne!(class(ExecKind::Sudo, &cmd.replace("--no-remove", "--allow-remove-essential")), "apt-one");
        assert_ne!(class(ExecKind::Sudo, &format!("{cmd}; touch /tmp/extra")), "apt-one");
    }

    #[test]
    fn continuation_install_has_only_the_exact_seventeen_pins() {
        let plan = crate::ops::protected_updates::reviewed_1025();
        let pins: Vec<_> = plan.iter().map(|p| format!("{}={}", p.name, p.to)).collect();
        let opts = crate::ops::updates::UpgradeOpts { protected: true, ..Default::default() };
        let command = |p: &[String]| crate::ops::updates::detached_command_opts("sparkbox-upgrade-t1", "/var/log/sparkbox", "/var/log/sparkbox/upgrade-t1.log", p, &opts);
        let cmd = command(&pins);
        assert_eq!(class(ExecKind::Sudo, &cmd), "apt-one");
        assert_eq!(items(ExecKind::Sudo, &cmd), plan.iter().map(|p| vec![p.name.clone()]).collect::<Vec<_>>());
        for i in 0..pins.len() {
            let mut bad = pins.clone(); bad.remove(i);
            assert_ne!(class(ExecKind::Sudo, &command(&bad)), "apt-one");
            let mut bad = pins.clone(); bad[i].push('9');
            assert_ne!(class(ExecKind::Sudo, &command(&bad)), "apt-one");
        }
        let mut mixed = pins.clone(); mixed[0] = "linux-headers-nvidia-hwe-24.04=7.0.0-1024.25~24.04.1".into();
        assert_ne!(class(ExecKind::Sudo, &command(&mixed)), "apt-one");
        for bad in [cmd.replace("--no-remove", "--allow-remove-essential"), format!("{cmd}; touch /tmp/extra")] {
            assert_ne!(class(ExecKind::Sudo, &bad), "apt-one");
        }
    }

    #[test]
    fn fleet_install_has_only_the_exact_twenty_four_pins() {
        let plan = crate::ops::protected_updates::reviewed_fleet_1025();
        let pins: Vec<_> = plan.iter().map(|p| format!("{}={}", p.name, p.to)).collect();
        let opts = crate::ops::updates::UpgradeOpts { protected: true, ..Default::default() };
        let command = |p: &[String]| crate::ops::updates::detached_command_opts("sparkbox-upgrade-t1", "/var/log/sparkbox", "/var/log/sparkbox/upgrade-t1.log", p, &opts);
        let cmd = command(&pins);
        assert_eq!(class(ExecKind::Sudo, &cmd), "apt-one");
        assert_eq!(items(ExecKind::Sudo, &cmd), plan.iter().map(|p| vec![p.name.clone()]).collect::<Vec<_>>());
        for i in 0..pins.len() {
            let mut bad = pins.clone(); bad.remove(i);
            assert_ne!(class(ExecKind::Sudo, &command(&bad)), "apt-one");
            let mut bad = pins.clone(); bad[i].push('9');
            assert_ne!(class(ExecKind::Sudo, &command(&bad)), "apt-one");
        }
        for bad in [cmd.replace("--no-remove", "--allow-remove-essential"), format!("{cmd}; touch /tmp/extra")] {
            assert_ne!(class(ExecKind::Sudo, &bad), "apt-one");
        }
    }

    #[test]
    fn every_row_of_the_command_shape_table() {
        use ExecKind::*;
        let detached = crate::ops::updates::detached_command("sparkbox-upgrade-t1", "/var/log/sparkbox", "/var/log/sparkbox/upgrade-t1.log", &["libfoo=1.2-3".into()]);
        for (kind, cmd, want) in &positive_rows() {
            assert_eq!(class(*kind, cmd), *want, "{kind:?} `{cmd}`");
        }
        // Items for the fence's `only` lists: C1′ names its packages; round 3's forms are `other`.
        assert_eq!(items(Sudo, contracts::C1_PINNED), vec![vec!["libfoo".to_string()]]);
        assert!(items(Sudo, contracts::C1_ONE).is_empty());
        assert!(items(Sudo, &contracts::c1_everything()).is_empty());
        assert_eq!(items(Run, &contracts::c7("dev")), vec![vec!["sparkbox-test-busybox".to_string()]]);
        assert_eq!(items(Sudo, "systemctl restart -- 'earlyoom.service'"), vec![vec!["earlyoom.service".to_string()]]);
        assert_eq!(items(Sudo, &detached), vec![vec!["libfoo".to_string()]]);
        assert_eq!(items(Sudo, "apt-mark hold 'a' 'b'"), vec![vec!["a".to_string()], vec!["b".to_string()]]);
        assert_eq!(items(ContainerAction, "docker restart 0123456789abcdef0123"), vec![vec!["0123456789abcdef0123".to_string(), "glm53".to_string()]]);
        assert_eq!(items(Run, "docker rm glm53"), vec![vec!["glm53".to_string(), "0123456789abcdef0123".to_string()]]);
    }

    /// Commands the fence must not mistake for one of its classes.
    fn negative_rows() -> Vec<(ExecKind, String, &'static str)> {
        use ExecKind::*;
        let c1 = contracts::C1_PINNED;
        // Round 2's detached upgrade (no package list first, no checks after apt): no longer apt-one.
        let r2_detached = "mkdir -p '/var/log/sparkbox' && systemd-run --unit sparkbox-upgrade-t1 --collect -p StandardOutput=append:/var/log/sparkbox/upgrade-t1.log -p StandardError=append:/var/log/sparkbox/upgrade-t1.log /bin/sh -c 'trap : TERM; DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l NEEDRESTART_SUSPEND=1 apt-get -o Dpkg::Options::=--force-confold -y install --only-upgrade '\''libfoo'\''; code=$?; echo \"SPARKBOX-EXIT $code\"; exit $code'";
        vec![
            (Sudo, r2_detached.into(), "other"),
            // Round 3's forms, refused since the end of round 4: the unpinned and dist-upgrade runs
            // (C1), `docker rm -f` unloads, and the load by the Ray image's tag with `--shm-size`.
            (Sudo, contracts::C1_ONE.into(), "other"),
            (Sudo, contracts::c1_everything(), "other"),
            (Sudo, contracts::c1_dl_limit(), "other"),
            (Sudo, contracts::c1_in(&format!("{LIB}/logs"), true), "other"),
            (Run, "ids=$(docker ps -aq --filter label=sparkbox.instance=release --filter label=sparkbox.deployment=d1); [ -z \"$ids\" ] || docker rm -f $ids >/dev/null; rm -f ~/.sparkbox/keys/d1; echo ok".into(), "other"),
            (Run, "ids=$(docker ps -aq --filter label=sparkbox.instance=release --filter label=sparkbox.deployment=d1 --filter label=sparkbox.role=head); [ -z \"$ids\" ] || docker rm -f $ids >/dev/null; echo ok".into(), "other"),
            (Run, r3_load(true), "other"),
            (Run, r3_load(false), "other"),
            // The spec's own negatives: sshd is a service (its `only` list names it), glm53 is the owner's.
            (Sudo, "systemctl restart sshd".into(), "service"),
            (Run, "docker rm glm53".into(), "container-owner"),
            // A root shell command that isn't one of the named shapes.
            (Sudo, "rm -rf /usr/local/lib/sparkbox".into(), "other"),
            (Sudo, "apt-get -y install nvidia-driver-580".into(), "other"),
            (Sudo, "systemd-run --unit sparkbox-upgrade-x /bin/sh -c 'rm -rf /'".into(), "other"),
            (Sudo, "systemd-run --unit other-thing /bin/sh -c 'apt-get -y install x'".into(), "other"),
            (Sudo, "sh -c 'echo 3 > /proc/sys/vm/drop_caches; reboot'".into(), "other"),
            (Sudo, root_sparkjob_command(LIB, SHA, "commit '/etc/sudoers' --sha256 ab"), "other"),
            (Sudo, root_sparkjob_command("/tmp/elsewhere", SHA, "read '/etc/shadow'"), "other"),
            (SparkjobRoot, "commit '/etc/hosts' --sha256 ab --from '/home/sparkuser/.sparkbox/tmp/x'".into(), "other"),
            (SudoInput, "tee /etc/sudoers.d/x".into(), "other"),
            (Sudo, "kill -9 4242".into(), "other"),
            // Docker work that isn't one of the shapes.
            (Run, "docker run --rm -v /:/host alpine chroot /host true".into(), "other"),
            (Run, "docker run -d --label sparkbox.deployment=d1 --label sparkbox.role=head img".into(), "other"),
            (Run, "docker run -d --name sparkbox-test-x alpine sleep 1".into(), "other"),
            (Run, "rm -rf ~/x && docker run -d --name sparkbox-test-x --label sparkbox.test=1 alpine true".into(), "other"),
            // A labelled test container that takes root, or whose own command is destructive, is `other`.
            (Run, "docker run --name sparkbox-test-x --label sparkbox.test=1 --privileged -v /:/host alpine chroot /host reboot".into(), "other"),
            (Run, "docker run -d --name sparkbox-test-y --label sparkbox.test=1 -v /:/host alpine true".into(), "other"),
            (Run, "docker run --name sparkbox-test-w --label sparkbox.test=1 --pid host alpine true".into(), "other"),
            // Run options that reach the host without --privileged or a host mount (the safety
            // review's probe), and any option off the short list: `other` (fail closed).
            (Run, "docker run -d --name sparkbox-test-x --label sparkbox.test=1 --security-opt systempaths=unconfined busybox sh -c 'echo b > /proc/sysrq-trigger'".into(), "other"),
            (Run, "docker run -d --name sparkbox-test-x --label sparkbox.test=1 --security-opt seccomp=unconfined busybox sleep 1".into(), "other"),
            (Run, "docker run -d --name sparkbox-test-x --label sparkbox.test=1 --security-opt=apparmor=unconfined busybox sleep 1".into(), "other"),
            (Run, "docker run -d --name sparkbox-test-x --label sparkbox.test=1 --userns=host busybox sleep 1".into(), "other"),
            (Run, "docker run -d --name sparkbox-test-x --label sparkbox.test=1 --cgroupns=host busybox sleep 1".into(), "other"),
            (Run, "docker run -d --name sparkbox-test-x --label sparkbox.test=1 --sysctl net.ipv4.ip_forward=1 busybox sleep 1".into(), "other"),
            (Run, "docker run -d --name sparkbox-test-x --label sparkbox.test=1 --volumes-from vllm-head busybox sleep 1".into(), "other"),
            (Run, "docker run -d --name sparkbox-test-x --label sparkbox.test=1 -v hostroot:/h busybox sleep 1".into(), "other"),
            (Run, "docker run -d --name sparkbox-test-x --label sparkbox.test=1 --mount source=hostroot,target=/h busybox sleep 1".into(), "other"),
            (Run, "docker run -d --name sparkbox-test-x --label sparkbox.test=1 --device-cgroup-rule 'c 1:1 rwm' busybox sleep 1".into(), "other"),
            (Run, "docker run -d --name sparkbox-test-x --label sparkbox.test=1 --network host busybox sleep 1".into(), "other"),
            (Run, "docker run -d --name sparkbox-test-x --label sparkbox.test=1 --network=container:vllm-head busybox sleep 1".into(), "other"),
            (Run, "docker run -d --name sparkbox-test-x --label sparkbox.test=1 --entrypoint /bin/sh busybox -c true".into(), "other"),
            (Run, "docker run -d --name sparkbox-test-x --label sparkbox.test=1 -dv /:/h busybox sleep 1".into(), "other"),
            (Run, "docker run -d --name sparkbox-test-x --label sparkbox.test=1".into(), "other"),
            (Run, "docker run -d --name sparkbox-test-x --label sparkbox.test=1 -e A=$(reboot) busybox sleep 1".into(), "other"),
            // File ops outside ~/sparkbox-test, or escaping it, are `other`.
            (Run, "chmod -- 600 '/home/sparkuser/models/x'".into(), "other"),
            (Run, "mkdir -- '/home/sparkuser/models/new'".into(), "other"),
            (Run, crate::files::rename_command("/home/sparkuser/sparkbox-test/a", "/home/sparkuser/models/b"), "other"),
            (Run, "chmod -- 600 '/home/sparkuser/sparkbox-test/../models/x'".into(), "other"),
            (ExecIo, "mkdir -p -- '/home/sparkuser/models/.sparkbox-incoming-1' && tar -C '/home/sparkuser/models/.sparkbox-incoming-1' -xf -".into(), "shell"),
            (Run, "docker system prune -af".into(), "other"),
            (Run, "docker stop $(docker ps -q)".into(), "other"),
            // Files outside ~/sparkbox-test, or escaping it.
            (Sparkjob, "rm '/home/sparkuser/models/glm'".into(), "other"),
            (Sparkjob, "trash '/home/sparkuser/sparkbox-test/../models'".into(), "other"),
            (Sparkjob, "commit '/home/sparkuser/sparkbox-test/a' --sha256 ab --backups '/etc'".into(), "other"),
            (Sparkjob, "cp '/home/sparkuser/sparkbox-test' '/home/sparkuser/sparkbox-test/b' 7 '../x'".into(), "other"),
            (Sparkjob, "start download j1".into(), "other"),
            // The full readiness check without the instance would start an unlabelled container.
            (Sparkjob, "check '~/models' 'nvcr.io/nvidia/vllm:26.05-py3' 'nvcr.io/nvidia/vllm:26.05-py3-ray'".into(), "other"),
            (Sparkjob, "check '~/models' 'img' 'img-ray' --instance 'dev;reboot'".into(), "other"),
            (Sparkjob, "check '~/models' 'img' 'img-ray' --instance ''".into(), "other"),
            (Sparkjob, "check '~/models' 'img' 'img-ray' --instance '--rm'".into(), "other"),
            (Sparkjob, "check '~/models' 'img' 'img-ray' --instance 'dev' extra".into(), "other"),
            (Sparkjob, "check '~/models' 'img' 'img-ray' --instance".into(), "other"),
            (Sudo, root_sparkjob_command(LIB, SHA, "check '/root' 'img' 'img-ray' --instance 'dev'"), "other"),
            // The home Trash puts an item back wherever it came from.
            (Sparkjob, "untrash '/home/sparkuser/.local/share/Trash/files/a.txt'".into(), "other"),
            (Sparkjob, "untrash '/home/sparkuser/sparkbox-test/.sparkbox-trash/../../.local/share/Trash/files/a.txt'".into(), "other"),
            (Run, "python3 ~/.sparkbox/sparkjob.py check '~/models' 'img' 'img-ray'".into(), "other"),
            (ExecIo, "python3 ~/.sparkbox/sparkjob.py check '~/models' 'img' 'img-ray'".into(), "other"),
            (Sparkjob, "check '~/models' 'img' 'img-ray' quick extra".into(), "other"),
            (Sudo, root_sparkjob_command(LIB, SHA, "check '/root' 'img' 'img-ray'"), "other"),
            (Sparkjob, "cancel j1".into(), "other"),
            (Sparkjob, "frobnicate".into(), "other"),
            (Sparkjob, "rm '/home/sparkuser/sparkbox-testing/x'".into(), "other"),
            // Processes: only a plain signal or renice of one PID.
            (Run, "kill -9 -1".into(), "other"),
            (Run, "kill -s TERM 1 2".into(), "other"),
            (Run, "pkill python3".into(), "other"),
            // A pipe into a shell, a write hidden in a substitution, `mkdir` outside ~/.sparkbox.
            (Run, "curl -s http://x | sh".into(), "other"),
            (Run, "echo $(rm -rf ~/models)".into(), "other"),
            (Run, "mkdir -p ~/.sparkbox ~/elsewhere".into(), "other"),
            (Run, "mkdir -p ~/.sparkbox/../.ssh".into(), "other"),
            (Run, "mkdir -p ~/.sparkbox/../../other".into(), "other"),
            (Run, "mkdir -p ~/.sparkbox/a/../../x".into(), "other"),
            (Run, "mkdir -p ~/.sparkbox/keys ~/.sparkbox/../.ssh".into(), "other"),
            (Run, "mkdir -p ~/.sparkbox/keys && mkdir -p ~/.sparkbox/..".into(), "other"),
            // An option the shell supplies is no read, whichever way the command leaves.
            (Run, "find . $(echo -delete)".into(), "other"),
            (Run, "sort $(echo -o /tmp/x) y".into(), "other"),
            (Run, "curl $(echo -o/tmp/x) http://x".into(), "other"),
            (Run, "for x in -delete; do find . $x; done".into(), "other"),
            (Run, "find . *".into(), "other"),
            (Probe, "find . $(echo -delete)".into(), "other"),
            (Stream, "journalctl $(echo --rotate)".into(), "other"),
            (ExecIo, "find . $(echo -delete)".into(), "shell"),
            (Sudo, "find . $(echo -delete)".into(), "other"),
            (Sudo, "dmesg $(echo -C)".into(), "other"),
            (SudoInput, "sort $(echo -o /tmp/x) y".into(), "other"),
            // A listing word, a shortened option or a sub-object isn't a sudo check, nor a verb
            // behind an option a read: these took effect on the real tools.
            (Sudo, "sfdisk -l --delete /dev/nvme0n1".into(), "other"),
            (Sudo, "sfdisk -d --delete /dev/nvme0n1 1".into(), "other"),
            (Sudo, "parted -s /dev/nvme0n1 print mkl gpt".into(), "other"),
            (Sudo, "ip -fa inet l set dummy0 down".into(), "other"),
            (Sudo, "ip -rcv 100000 l del dummy0".into(), "other"),
            (Sudo, "ip xfrm s flush".into(), "other"),
            (Sudo, "nmcli -fi radio connection delete c9".into(), "other"),
            (Run, "git --namespace log commit -qam two".into(), "other"),
            (Run, "docker compose --workdir ps rm -f".into(), "other"),
            (Run, "docker buildx --builder ls rm foo".into(), "other"),
            (Run, "docker --config /tmp/evil info".into(), "other"),
            (Run, "docker --context x ps".into(), "other"),
            (Run, "docker -H ssh://x ps".into(), "other"),
            (Run, "apt-get changelog -d jq".into(), "other"),
            (Run, "curl -w @- http://x".into(), "other"),
            (Run, "curl -sw@/tmp/fmt http://x".into(), "other"),
            (Pty, "rm -rf ~/models".into(), "shell"),
            // A read in a terminal takes keys: less's `!`, top's `k`. Only a follower stays one.
            (Pty, "less /etc/hosts".into(), "shell"),
            (Pty, "journalctl -f | less".into(), "shell"),
            (Pty, "journalctl -f -n 1 | less".into(), "shell"),
            (Pty, "journalctl -f x; less /etc/passwd".into(), "shell"),
            (Pty, "journalctl -f -n 1; top".into(), "shell"),
            (Pty, "journalctl -f -n 1 && top".into(), "shell"),
            (Pty, "journalctl -f -n 1 || top".into(), "shell"),
            (Pty, "journalctl -f -n 1 & top".into(), "shell"),
            (Pty, "journalctl -f -n 1 &".into(), "shell"),
            (Pty, "journalctl -f -n 1\ntop".into(), "shell"),
            (Pty, "docker logs -f x | less".into(), "shell"),
            (Pty, "docker logs -f x; less /etc/passwd".into(), "shell"),
            (Pty, "journalctl -f -n 1 2>/dev/null".into(), "shell"),
            (Pty, "docker logs -f x >/dev/null".into(), "shell"),
            (Pty, "journalctl -f </dev/null".into(), "shell"),
            // A pager's options, and anything else no log tab asks for.
            (Pty, "journalctl -f --list-boots".into(), "shell"),
            (Pty, "journalctl -f --help".into(), "shell"),
            (Pty, "journalctl -f --disk-usage".into(), "shell"),
            (Pty, "journalctl -f --header".into(), "shell"),
            (Pty, "docker logs -f --foo x".into(), "shell"),
            (Pty, "docker logs -f a b".into(), "shell"),
            (Pty, "journalctl -f _PID=1 _PID=2".into(), "shell"),
            (Pty, "journalctl -f $(rm -rf ~/models)".into(), "shell"),
            (Pty, "docker logs -f $(reboot)".into(), "shell"),
            (Pty, "docker logs -f `reboot`".into(), "shell"),
            (Pty, "more /etc/hosts".into(), "shell"),
            (Pty, "top".into(), "shell"),
            (Pty, "journalctl -u ssh".into(), "shell"),
            (Pty, "docker logs gemma".into(), "shell"),
            (Pty, "cat /etc/hosts".into(), "shell"),
            (Pty, "journalctl -f -u ssh; reboot".into(), "shell"),
            (Stream, "rm -rf /tmp/x".into(), "other"),
            (ExecIo, "cat > ~/x".into(), "shell"),
            // apt-one is exactly C1′: one more command anywhere, another folder
            // or id, more environment, another needrestart mode, a hook or an odd package name is
            // `other`.
            (Sudo, c1.replace("exit $code'", "exit $code; rm -rf /'"), "other"),
            (Sudo, c1.replace("trap : TERM; ", "trap : TERM; rm -rf /; "), "other"),
            (Sudo, c1.replace("; apt-get check;", "; apt-get check; reboot;"), "other"),
            (Sudo, format!("{c1}; rm -rf /"), "other"),
            (Sudo, format!("rm -rf / && {c1}"), "other"),
            (Sudo, c1.replace("/var/log/sparkbox", "/etc/sudoers.d"), "other"),
            (Sudo, c1.replace("dpkg-before-t1", "dpkg-before-t2"), "other"),
            (Sudo, c1.replace("append:/var/log/sparkbox/upgrade-t1.log -p StandardError", "append:/etc/x -p StandardError"), "other"),
            (Sudo, c1.replace("chmod 0755", "chmod 0777"), "other"),
            (Sudo, c1.replace("--collect", "--collect -p ExecStartPre=/sbin/reboot"), "other"),
            (Sudo, c1.replace("NEEDRESTART_SUSPEND=1", "NEEDRESTART_SUSPEND=1 LD_PRELOAD=/tmp/x.so"), "other"),
            (Sudo, c1.replace("NEEDRESTART_MODE=l", "NEEDRESTART_MODE=a"), "other"),
            (Sudo, c1.replace("needrestart -b -r l", "needrestart -b -r a"), "other"),
            (Sudo, c1.replace("-o DPkg::Lock::Timeout=300", "-o DPkg::Pre-Invoke::=/sbin/reboot"), "other"),
            (Sudo, c1.replace("libfoo", "libfoo$(reboot)"), "other"),
            (Sudo, c1.replace("libfoo", "-libfoo"), "other"),
            (Sudo, c1.replace("sparkbox-upgrade-t1", "sparkbox-upgrade-t1;reboot"), "other"),
            // The refresh is C3: no environment, and no setting outside the dry run's.
            (Sudo, "apt-get -o APT::Update::Pre-Invoke::=/sbin/reboot update".into(), "other"),
            (Sudo, "APT_CONFIG=/tmp/x.conf apt-get update".into(), "other"),
            (Sudo, "DEBIAN_FRONTEND=noninteractive apt-get -o Acquire::Retries=3 update".into(), "other"),
            (Sudo, "apt-get -c /tmp/apt.conf update".into(), "other"),
            // updates::cancel_command is `other` by shape (Cancel tags it apt-one where it's sent).
            (Sudo, crate::ops::updates::cancel_command("sparkbox-upgrade-t1"), "other"),
            // A substitution the shell expands runs a command the shape's words don't show.
            (Run, "mkdir -- /home/sparkuser/sparkbox-test/$(rm -rf ~/models)".into(), "other"),
            (Run, "mkdir -- '/home/sparkuser/sparkbox-test/'$(rm -rf ~/models)".into(), "other"),
            (Run, "mkdir -- \"/home/sparkuser/sparkbox-test/$(reboot)\"".into(), "other"),
            (Run, "mkdir -- /home/sparkuser/sparkbox-test/`reboot`".into(), "other"),
            (Run, "python3 ~/.sparkbox/sparkjob.py ls $(rm -rf ~/models)".into(), "other"),
            (Sparkjob, "ls $(rm -rf ~/models)".into(), "other"),
            (Sparkjob, "trash ~/sparkbox-test/$(reboot)".into(), "other"),
            (Sudo, "systemctl restart $(reboot)".into(), "other"),
            (Sudo, "apt-mark hold `reboot`".into(), "other"),
            (Sudo, "sh -c 'systemctl restart $(reboot)'".into(), "other"),
            (SparkjobRoot, "read $(reboot)".into(), "other"),
            (Run, "docker run -d --name sparkbox-test-x --label sparkbox.test=1 alpine $(rm -rf ~/models)".into(), "other"),
            (ExecIo, "tar -C '/home/sparkuser/sparkbox-test' -cvf - -- $(rm -rf ~)".into(), "shell"),
            (Run, "diff <(rm -rf ~/models) /etc/hosts".into(), "other"),
            // ~/sparkbox-test itself, and what's in its Trash already, have no Trash in the folder.
            (Sparkjob, "trash '/home/sparkuser/sparkbox-test'".into(), "other"),
            (Sparkjob, "trash '~/sparkbox-test'".into(), "other"),
            (Sparkjob, "trash '~/sparkbox-test/'".into(), "other"),
            (Sparkjob, "trash '~/sparkbox-test/.'".into(), "other"),
            (Sparkjob, "trash '/home/sparkuser/sparkbox-test/.sparkbox-trash/files/a.txt'".into(), "other"),
            (Sparkjob, "trash '~/sparkbox-test/.sparkbox-trash'".into(), "other"),
            (Sparkjob, "trash '~/sparkbox-test/./.sparkbox-trash/files/a'".into(), "other"),
            (Sparkjob, "trash '~/sparkbox-test/a' '~/sparkbox-test'".into(), "other"),
            // `sh -c` with operands after its script ($0…), or sourcing an rcfile, isn't a plain wrapper.
            (Sudo, "sh -c 'systemctl reboot' x".into(), "other"),
            (Sudo, "bash -ic 'systemctl daemon-reload'".into(), "other"),
            (Sudo, "bash -c -i 'systemctl daemon-reload'".into(), "other"),
            (Sudo, "bash --rcfile /tmp/x -c 'systemctl daemon-reload'".into(), "other"),
            (Sudo, "sh -c".into(), "other"),
        ]
    }

    /// Every log tab `term::command_for` can open is a follower, so it stays a read in a terminal.
    #[test]
    fn every_log_tab_is_a_follower() {
        use crate::term::{command_for, Target};
        for since in [None, Some("10m"), Some("1h"), Some("6h"), Some("24h"), Some("boot")] {
            let since = since.map(String::from);
            for target in [Target::Journal { unit: "ssh.service".into(), since: since.clone() }, Target::ContainerLogs { container: "gemma".into(), since: since.clone() }] {
                let cmd = command_for(&target).unwrap().unwrap();
                assert_eq!(class(ExecKind::Pty, &cmd), "read", "`{cmd}`");
            }
        }
    }

    #[test]
    fn negative_rows_the_fence_must_not_mistake() {
        use ExecKind::*;
        for (kind, cmd, want) in &negative_rows() {
            assert_eq!(class(*kind, cmd), *want, "{kind:?} `{cmd}`");
        }
        // The service's `only` item for `systemctl restart sshd` is sshd.
        assert_eq!(items(Sudo, "systemctl restart sshd"), vec![vec!["sshd".to_string()]]);
    }

    /// No shape names items under `other` (so exit::tagged, which keeps only the items of a
    /// shape of the tag's own class, has none to drop there).
    #[test]
    fn no_shape_names_items_under_other() {
        for (kind, cmd, _) in positive_rows().into_iter().chain(negative_rows()) {
            if let Shape::Change(FenceClass::Other, items) = shape(kind, &cmd) {
                assert!(items.is_empty(), "{kind:?} `{cmd}`: {items:?}");
            }
        }
    }

    /// Typed Wi-Fi is the whole command, in Fleet's wrapper or not (C9), with nmcli's prefixes of
    /// its words, its wait option and sudo (C-W).
    #[test]
    fn typed_wifi_is_the_whole_command() {
        use crate::ops::fleet::wrap;
        for cmd in [
            "nmcli radio wifi off",
            "nmcli radio wifi on",
            "nmcli  radio wifi off",
            "nmcli r wifi off",
            "nmcli radio wi on",
            "nmcli ra wif off",
            "nmcli radi wifi on",
            "nmcli -w 10 radio wifi off",
            "nmcli --wait 5 r wi on",
            "sudo nmcli radio wifi off",
            "sudo -n nmcli r wif on",
            "sudo -n nmcli -w 10 rad wi off",
        ] {
            for sent in [cmd.to_string(), wrap(cmd, 30, false), wrap(cmd, 600, true)] {
                assert!(typed_wifi(&sent), "`{sent}`");
            }
        }
        // `w` is wwan's prefix too, `radio all` turns off more, a compound command is a Fleet
        // command, and anything else around it isn't Wi-Fi alone.
        for cmd in [
            "nmcli r w off",
            "nmcli radio all off",
            "nmcli radio wwan off",
            "nmcli radio wifi off; reboot",
            "nmcli radio wifi",
            "nmcli radio wifi off > /tmp/x",
            "echo nmcli radio wifi off",
            "nmcli radio wifi off x",
            "sudo -u root nmcli radio wifi off",
            "sudo -E nmcli radio wifi off",
            "nmcli -w x radio wifi off",
            "nmcli -f all radio wifi off",
            "nmcli rx wifi off",
            "nmcli radio wifix off",
            "nmcli radio wifi of",
        ] {
            for sent in [cmd.to_string(), wrap(cmd, 30, false), wrap(cmd, 30, true)] {
                assert!(!typed_wifi(&sent), "`{sent}`");
            }
        }
        assert!(!typed_wifi("timeout --signal=TERM --kill-after=9s 30s sh -c 'nmcli radio wifi off'"));
        assert!(!typed_wifi("timeout 30s sh -c 'nmcli radio wifi off' x"));
    }

    /// A refused Files change's line says what it would have done.
    #[test]
    fn a_files_change_in_words() {
        let w = |a: &str| files_words(a);
        assert_eq!(w("commit '/home/sparkuser/sparkbox-test/notes.txt' --sha256 ab --expect '-'").as_deref(), Some("Save notes.txt"));
        assert_eq!(w(&contracts::c8("~/sparkbox-test/new.txt")).as_deref(), Some("New file new.txt"));
        assert_eq!(w("trash '/home/sparkuser/notes.txt'").as_deref(), Some("Move notes.txt to the Trash"));
        assert_eq!(w("trash 'a' 'b' 'c'").as_deref(), Some("Move 3 items to the Trash"));
        assert_eq!(w("untrash '~/.local/share/Trash/files/x'").as_deref(), Some("Put back x"));
        assert_eq!(w("rm '/home/sparkuser/models/glm/'").as_deref(), Some("Delete glm"));
        assert_eq!(w("delete '~/models' 'glm'").as_deref(), Some("Delete glm"));
        assert_eq!(w("place '~/x/.sparkbox-incoming-1' '~/x'").as_deref(), Some("Copy files to x"));
        assert_eq!(w("cp '~/a' '~/b' 7 'one' 'two'").as_deref(), Some("Copy 2 items to b"));
        assert_eq!(w("cleanup '~/b' 7").as_deref(), Some("Clean up a copy to b"));
        assert_eq!(w("sweep '~/b' 7").as_deref(), Some("Clean up copies in b"));
        assert_eq!(w("rm '/'").as_deref(), Some("Delete /"));
        for none in ["ls '~'", "read '/etc/hosts'", "lease claim 'dev' 'g' 'm' 45", "start download j1", "cancel j1", ""] {
            assert_eq!(w(none), None, "`{none}`");
        }
        assert_eq!(file_op_words(&crate::files::rename_command("/home/sparkuser/sparkbox-test/a.txt", "/home/sparkuser/sparkbox-test/b.txt")).as_deref(), Some("Rename a.txt to b.txt"));
        assert_eq!(file_op_words("mkdir -- '/home/sparkuser/sparkbox-test/new'").as_deref(), Some("New folder new"));
        assert_eq!(file_op_words("chmod -- 600 '/home/sparkuser/notes.txt'").as_deref(), Some("Change the permissions of notes.txt"));
        assert_eq!(file_op_words("uptime"), None);
    }

    #[test]
    fn daemon_reload_acts_on_no_item() {
        use ExecKind::*;
        assert!(acts_on_no_item(Sudo, "systemctl daemon-reload"));
        assert!(acts_on_no_item(Sudo, "env LC_ALL=C sh -c 'systemctl daemon-reload'"));
        for (kind, cmd) in [(Sudo, "systemctl restart -- 'earlyoom.service'"), (Sudo, "systemctl daemon-reload; reboot"), (Run, "systemctl daemon-reload"), (Sudo, "systemctl daemon-reexec")] {
            assert!(!acts_on_no_item(kind, cmd), "{kind:?} `{cmd}`");
        }
    }

    #[test]
    fn the_containers_a_command_names_in_any_of_its_parts() {
        use ExecKind::*;
        let containers = [container("0123456789abcdef0123", "glm53", &[]), container("aaaaaaaaaaaa1111", "gemma-cell2", &[("sparkbox.instance", "release")])];
        let ctx = Ctx { containers: &containers, home: Some("/home/sparkuser"), root_lib: LIB, sparkjob_sha: SHA };
        let names = |kind, cmd: &str| named_containers(kind, cmd, &ctx);
        let gemma = vec!["gemma-cell2".to_string(), "aaaaaaaaaaaa1111".to_string()];
        assert_eq!(names(Run, "docker rm gemma-cell2"), vec![gemma.clone()]);
        assert_eq!(names(ContainerAction, "docker stop aaaaaaaaaaaa1111"), vec![vec!["aaaaaaaaaaaa1111".to_string(), "gemma-cell2".to_string()]]);
        assert_eq!(names(Stream, "uptime; docker container stop -t 5 gemma-cell2 && echo ok"), vec![gemma.clone()]);
        assert_eq!(names(Sudo, "sudo -n docker kill -s KILL gemma-cell2"), vec![gemma.clone()]);
        assert_eq!(names(Run, "docker --config /tmp/c -l debug stop gemma-cell2"), vec![gemma.clone()]);
        assert_eq!(names(Run, "DOCKER_HOST=unix:///run/docker.sock docker rm -f glm53 gemma-cell2").len(), 2);
        assert_eq!(names(Run, "sh -c 'docker restart gemma-cell2'"), vec![gemma.clone()]);
        // The container a `docker exec` (or attach) runs in: a terminal's shell, a command in it.
        let shell = "docker exec -it -e TERM=xterm-256color gemma-cell2 sh -c 'if command -v bash >/dev/null 2>&1; then exec bash; else exec sh; fi'";
        assert_eq!(names(Pty, shell), vec![gemma.clone()]);
        for cmd in [
            "docker exec gemma-cell2 id",
            "docker exec -u 0 gemma-cell2 sh",
            "docker exec --user root -w /tmp gemma-cell2 sh",
            "docker exec --user=0 --env-file /tmp/e gemma-cell2 sh",
            "docker exec -itu0 gemma-cell2 sh",
            "docker exec -itu 0 gemma-cell2 sh",
            "docker exec -e=A=1 --detach-keys ctrl-x -d gemma-cell2 true",
            "docker container exec -it gemma-cell2 bash",
            "sudo docker exec -it gemma-cell2 bash",
            "docker exec -- gemma-cell2 true",
            "docker attach --detach-keys ctrl-x gemma-cell2",
            "docker -H unix:///run/docker.sock --tls exec -it gemma-cell2 sh",
            "docker --context default container exec gemma-cell2 true",
        ] {
            assert_eq!(names(ExecIo, cmd), vec![gemma.clone()], "`{cmd}`");
        }
        assert_eq!(names(Run, "docker exec aaaaaaaaaaaa1111 rm -rf /data"), vec![vec!["aaaaaaaaaaaa1111".to_string(), "gemma-cell2".to_string()]]);
        // Only the container, never its command's words.
        assert_eq!(names(Run, "docker exec glm53 docker rm gemma-cell2").len(), 1);
        // Wrapped, absolute-path and `/name` forms, and docker's management verbs (P1 r2).
        for cmd in [
            "/usr/bin/docker stop gemma-cell2",
            "env docker stop gemma-cell2",
            "timeout 5 docker stop gemma-cell2",
            "command docker stop gemma-cell2",
            "nice docker exec gemma-cell2 sh",
            "nohup docker kill gemma-cell2",
            "exec docker rm -f gemma-cell2",
            "sudo -u root docker stop gemma-cell2",
            "bash -lc 'docker stop gemma-cell2'",
            "docker update --restart=no gemma-cell2",
            "docker rename gemma-cell2 x",
            "docker commit gemma-cell2 img",
            "docker export gemma-cell2",
            "docker cp gemma-cell2:/etc/passwd /tmp/x",
            "docker cp /tmp/x gemma-cell2:/etc/passwd",
            "docker network disconnect bridge gemma-cell2",
            "docker stop /gemma-cell2",
        ] {
            assert_eq!(names(Run, cmd), vec![gemma.clone()], "`{cmd}`");
        }
        // A prune or `docker rm $(…)` names every container in the frame (fail closed).
        assert_eq!(names(ContainerAction, "docker container prune -f").len(), 2);
        assert_eq!(names(Run, "docker rm -f $(docker ps -aq)").len(), 2);
        // Not a container change, or a name SparkBox can't know before it runs.
        // A name the shell builds at run time names every container in the frame (fail closed);
        // a load's unload (C-U2, C-S) names none: its label filter is checked instead.
        for cmd in ["docker stop $(docker ps -q)", "docker exec $(docker ps -q) sh", "docker kill $X", "docker restart \"$C\"", "docker rm -f gemma-*", "docker cp `x`:/etc/passwd /tmp/x"] {
            assert_eq!(names(Run, cmd).len(), 2, "`{cmd}`");
        }
        assert!(names(Run, contracts::C_U).is_empty() && names(Run, contracts::C_S).is_empty());
        // Anything more, and its `$ids` could be any of them.
        let more = names(Run, &contracts::C_U.replace("echo ok", "docker rm -f gemma-cell2; echo ok"));
        assert!(more.contains(&gemma) && more.iter().any(|i| i[0] == "glm53"), "{more:?}");
        // Not a container change.
        for cmd in ["docker ps -a", "docker logs gemma-cell2", "echo docker rm gemma-cell2", "docker exec -it", "docker ps --format '{{.Names}}'"] {
            assert!(names(Run, cmd).is_empty(), "`{cmd}`");
        }
        assert!(names(DirectTcpip, "127.0.0.1:8000").is_empty());
    }

    /// Every command SparkBox's background checks send through `Live::probe` must read, or the
    /// exit refuses it ("background checks only read").
    #[test]
    fn every_background_check_reads() {
        let layout = crate::agent::Layout::system();
        let probes: Vec<String> = vec![
            "printf %s \"$HOME\"".into(),
            "id -u; id -g".into(),
            crate::ops::services::SHOW_UNITS.into(),
            crate::ops::services::LIST_FILES.into(),
            crate::ops::updates::STATUS_SCRIPT.into(),
            format!("systemctl show -p ActiveState,Result,LoadState -- {}", crate::live::sh_quote("sparkbox-upgrade-t1")),
            format!("tail -n 20 {}", crate::live::sh_quote("/var/log/sparkbox/upgrade-t1.log")),
            "sed 's/^.*) //' /proc/4242/stat 2>/dev/null | cut -d' ' -f20".into(),
            crate::agent::status_command(&layout),
            layout.cli("export --since 1790000000000"),
            layout.cli("export --since 1790000000000 --until 1790003600000"),
            layout.cli("alerts --since 1790000000001"),
            crate::live::lease::SHOW.into(),
            "cat /etc/machine-id".into(),
            crate::models::ollama::list_command(11434),
            crate::models::plan::probe_command("sparkbox-d1-cell2", "127.0.0.1", 8000),
            "docker logs --tail 40 glm53 2>&1 | tail -n 40".into(),
            "cat /proc/4242/cmdline | tr '\\0' ' '".into(),
        ];
        // An engine's readiness and metrics (models/watch.rs `curl`): reads, so look-only keeps them.
        let curl = |path: &str, port: u16, secs: u32| format!("curl -s -m {secs} -w '\\n%{{http_code}}' http://127.0.0.1:{port}{path} 2>/dev/null || printf '\\n000'");
        let sep = "printf '\\n--sparkbox--\\n'";
        let probes: Vec<String> = probes
            .into_iter()
            .chain([
                format!("{}; {sep}; {}", curl("/v1/models", 8000, 3), curl("/health", 8000, 3)),
                format!("{}; {sep}; {}; {sep}; systemctl show -p ActiveState --value ollama.service 2>/dev/null", curl("/api/tags", 11434, 3), curl("/api/ps", 11434, 3)),
                curl("/metrics", 8000, 2),
            ])
            .collect();
        // Evidence capture (evidence.rs) reads every node through probe too.
        let probes: Vec<String> = probes.into_iter().chain(crate::evidence::STEPS.iter().map(|(_, c)| c.to_string())).collect();
        for cmd in &probes {
            assert_eq!(class(ExecKind::Probe, cmd), "read", "`{cmd}`");
        }
    }

    #[test]
    fn model_load_covers_a_load_on_one_node_and_never_the_other_way() {
        assert_eq!(FenceClass::parse("model-load"), Some(FenceClass::ModelLoad));
        assert_eq!(FenceClass::ModelLoad.words(), "model loads on several nodes");
        assert_eq!(FenceClass::ModelLoadOne.wider(), Some(FenceClass::ModelLoad));
        assert_eq!(FenceClass::ModelLoad.wider(), None);
        for c in FenceClass::OPENABLE {
            assert!(c == FenceClass::ModelLoadOne || c.wider().is_none(), "{c:?}");
        }
    }

    /// Settings › Fence offers what the fence can name: src/lib/fence.ts lists the same 20 classes.
    #[test]
    fn the_settings_list_has_every_class() {
        let ts = include_str!("../../../src/lib/fence.ts");
        let list = |name: &str| -> Vec<String> {
            let from = ts.find(&format!("export const {name} = [")).unwrap_or_else(|| panic!("{name} in fence.ts"));
            let body = &ts[from..ts[from..].find("] as const").unwrap() + from];
            body.split('"').skip(1).step_by(2).map(String::from).collect()
        };
        let mut offered = list("OWNER_CLASSES");
        offered.extend(list("AUTONOMY_CLASSES"));
        offered.push("any".into());
        offered.sort();
        let mut classes: Vec<String> = FenceClass::OPENABLE.iter().map(|c| c.id().to_string()).collect();
        classes.sort();
        assert_eq!(offered, classes);
        assert_eq!(classes.len(), 20);
    }

    #[test]
    fn ids_names_and_paths() {
        assert!(same_id("0123456789abcdef", "0123456789ab"));
        assert!(!same_id("0123456789a", "0123456789a"), "under 12 hex isn't an id");
        assert!(!same_id("glm53", "glm53"));
        assert_eq!(FenceClass::parse("container-owner"), Some(FenceClass::ContainerOwner));
        assert_eq!(FenceClass::parse("other"), Some(FenceClass::Other));
        assert_eq!(FenceClass::parse("everything"), None);
        for c in FenceClass::OPENABLE {
            assert_eq!(FenceClass::parse(c.id()), Some(c));
            assert_eq!(serde_json::to_value(c).unwrap(), c.id(), "serde writes the same kebab-case id");
        }
        assert_eq!(segments("a 'b c' \"d\\\"e\" f\\ g; h && i || j | k").unwrap(), vec![
            vec!["a".to_string(), "b c".into(), "d\"e".into(), "f g".into()],
            vec!["h".into()],
            vec!["i".into()],
            vec!["j".into()],
            vec!["k".into()]
        ]);
        assert!(segments("echo 'x").is_none(), "unbalanced quotes");
        assert_eq!(segments("a $(b; c) 2>&1").unwrap(), vec![vec!["a".to_string(), "$(b; c)".into(), "\u{1}2>&1".into()]]);
    }

    /// A parameter expansion or glob the shape's words don't show — `${IFS}` splitting a path into
    /// a second one, a `$HOME` or a `*`/`.?` glob — makes a file op `other`, so a path written
    /// under ~/sparkbox-test can't reach ~/models. Single-quoted (inert) expansions are fine.
    #[test]
    fn an_expansion_in_a_path_is_other() {
        use ExecKind::*;
        for (kind, cmd) in [
            (Run, "chmod -- 000 ~/sparkbox-test/x${IFS}$HOME/victim.txt"),
            (Run, "chmod -- 000 ~/sparkbox-test/.?/victim.txt"),
            (Run, "mkdir -- ~/sparkbox-test/a${IFS}$HOME/newdir"),
            (Run, "mkdir -- ~/sparkbox-test/*"),
            (Sparkjob, "rm ~/sparkbox-test/y.txt${IFS}$HOME/models"),
            (Sparkjob, "trash ~/sparkbox-test/$HOME"),
        ] {
            assert_eq!(class(kind, cmd), "other", "{kind:?} `{cmd}`");
        }
        // A single-quoted `$`/glob is a literal character in the name, still files-test.
        assert_eq!(class(Run, "mkdir -- '/home/sparkuser/sparkbox-test/a*b'"), "files-test");
        assert_eq!(class(Run, "mkdir -- '/home/sparkuser/sparkbox-test/${IFS}'"), "files-test");
    }

    /// C-L2 is a load only exactly as plan.rs writes it: one more option, mount, word or step, a
    /// mount that isn't the load's own, an image by name, or a word after the launcher is `other`.
    /// So is a load whose `sparkbox` paths (its key, its files, its id) aren't its own.
    #[test]
    fn a_load_is_exactly_c_l2() {
        use ExecKind::*;
        let (cl, head, worker) = (contracts::C_L, contracts::C_L_HEAD, contracts::C_L_WORKER);
        assert_eq!(items(Run, cl), vec![vec!["sparkbox-d1".to_string()]]);
        assert_eq!(items(Run, &contracts::c_l_no_key()), vec![vec!["sparkbox-d1".to_string()]]);
        assert_eq!(items(Run, head), vec![vec!["sparkbox-d1-r0".to_string()]]);
        assert_eq!(items(Run, worker), vec![vec!["sparkbox-d1-r1".to_string()]]);
        let id = format!("sha256:{}", "a".repeat(64));
        let model = "/home/sparkuser/models/google/gemma-3-27b-it";
        let with_model = |m: &str| cl.replace(&format!("-v {model}:{model}:ro"), &format!("-v {m}:ro")).replace(&format!("sparkbox.path={model}"), &format!("sparkbox.path={}", m.split(':').next().unwrap()));
        let key_mount = r#" -v "$HOME/.sparkbox/keys/d1:/run/sparkbox/key:ro""#;
        // A Hugging Face cache snapshot: the mount is the whole models--… folder, the label its path.
        let hub = "/home/sparkuser/.cache/huggingface/hub/models--google--gemma";
        let cached = |tail: &str| cl.replace(&format!("-v {model}:{model}:ro"), &format!("-v {hub}:{hub}:ro")).replace(&format!("sparkbox.path={model}"), &format!("'sparkbox.path={hub}/snapshots/{tail}'"));
        assert_eq!(class(Run, &cached("abc123")), "model-load-one");
        let bad: Vec<(&str, String)> = vec![
            ("a snapshot path with a space", cached("abc 123")),
            ("a snapshot path with a variable", cached("$x")),
            // The rules the fence owes C-L2.
            ("--privileged on one node", cl.replace(" --restart no", " --privileged --restart no")),
            ("--privileged in another place", cl.replace(" --network host", " --privileged --network host")),
            ("--privileged twice", head.replace(" --privileged", " --privileged --privileged")),
            ("a key mount on a worker", worker.replace(" --entrypoint python3", &format!("{key_mount} --entrypoint python3"))),
            ("a key on a worker", worker.replace(" --entrypoint python3", &format!("{key_mount} --entrypoint python3")).replace(" /run/sparkbox/launch.py", " /run/sparkbox/launch.py --key")),
            ("--key on a worker", format!("{worker} --key")),
            ("a key mount and no --key", cl.replace(" /run/sparkbox/launch.py --key", " /run/sparkbox/launch.py")),
            ("--key and no key mount", cl.replace(key_mount, "")),
            ("a head's key mount and no --key", head.replace(" /run/sparkbox/launch.py --key", " /run/sparkbox/launch.py")),
            ("--key twice", format!("{cl} --key")),
            ("another deployment's launch files", cl.replace(".sparkbox/deploy/d1:", ".sparkbox/deploy/d2:")),
            ("a head with another deployment's launch files", head.replace(".sparkbox/deploy/d1:", ".sparkbox/deploy/d2:")),
            ("a worker with another deployment's launch files", worker.replace(".sparkbox/deploy/d1:", ".sparkbox/deploy/d2:")),
            ("writable launch files", cl.replace("/run/sparkbox/deploy:ro", "/run/sparkbox/deploy")),
            ("launch files from elsewhere", cl.replace("$HOME/.sparkbox/deploy/d1:", "$HOME/.ssh:")),
            ("no launch files", cl.replace(r#" -v "$HOME/.sparkbox/deploy/d1:/run/sparkbox/deploy:ro""#, "")),
            ("another key", cl.replace(".sparkbox/keys/d1:", ".sparkbox/keys/d2:")),
            ("another launcher", cl.replace(".sparkbox/launch-12b1d2f4c5b5565459585085c89dab0171be5d22c0618d3f7a219b871733ddac.py:", ".sparkbox/evil.py:")),
            ("an unversioned launcher", cl.replace(&crate::models::plan::launcher_name(), "launch.py")),
            ("a writable launcher", cl.replace("/run/sparkbox/launch.py:ro", "/run/sparkbox/launch.py")),
            ("no launcher mount", cl.replace(r#" -v "$HOME/.sparkbox/launch-12b1d2f4c5b5565459585085c89dab0171be5d22c0618d3f7a219b871733ddac.py:/run/sparkbox/launch.py:ro""#, "")),
            ("a host root mount", cl.replace(" --entrypoint python3", " -v /:/host --entrypoint python3")),
            ("a host root mount on a worker", worker.replace(" --entrypoint python3", " -v /:/host --entrypoint python3")),
            ("a host root mount first", cl.replace(" --network host", " -v /:/host --network host")),
            ("--entrypoint sh", cl.replace("--entrypoint python3", "--entrypoint sh")),
            ("--entrypoint sh on a worker", worker.replace("--entrypoint python3", "--entrypoint sh")),
            ("--entrypoint bash", cl.replace("--entrypoint python3", "--entrypoint bash")),
            ("a shell after the launcher", format!("{cl} -c 'reboot'")),
            ("a shell after a worker's launcher", format!("{worker} sh -c reboot")),
            ("a word after the launcher", format!("{} x", contracts::c_l_no_key())),
            ("another script", cl.replace(" /run/sparkbox/launch.py --key", " /tmp/x.py --key")),
            ("python's -c", cl.replace(" /run/sparkbox/launch.py --key", " -c 'import os'")),
            ("a second segment", format!("{cl}; reboot")),
            ("a second segment on a worker", format!("{worker} && touch /tmp/x")),
            ("a step before", format!("rm -rf ~/x && {cl}")),
            ("a pipe after", format!("{cl} | tee /tmp/x")),
            ("in the background", format!("{cl} &")),
            ("the name of another role", cl.replace("--name sparkbox-d1", "--name sparkbox-d1-r0")),
            ("a head named as one node", head.replace("--name sparkbox-d1-r0", "--name sparkbox-d1")),
            ("a head named for another rank", head.replace("--name sparkbox-d1-r0", "--name sparkbox-d1-r1")),
            ("a worker named for another rank", worker.replace("--name sparkbox-d1-r1", "--name sparkbox-d1-r2")),
            ("a worker named for the head", worker.replace("--name sparkbox-d1-r1", "--name sparkbox-d1-r0")),
            ("a worker of rank 0", worker.replace("--name sparkbox-d1-r1", "--name sparkbox-d1-r0").replace("sparkbox.rank=1", "sparkbox.rank=0")),
            ("rank 01", worker.replace("--name sparkbox-d1-r1", "--name sparkbox-d1-r01").replace("sparkbox.rank=1", "sparkbox.rank=01")),
            ("a head of rank 1", head.replace("sparkbox.rank=0", "sparkbox.rank=1")),
            ("a one-node load of rank 1", cl.replace("sparkbox.rank=0", "sparkbox.rank=1")),
            ("a name for another deployment", cl.replace("--name sparkbox-d1", "--name sparkbox-d2")),
            ("a role that isn't one", cl.replace("sparkbox.role=single", "sparkbox.role=boss")),
            ("a role that isn't its name's", cl.replace("sparkbox.role=single", "sparkbox.role=head")),
            ("parallel with no x", cl.replace("sparkbox.parallel=1x1", "sparkbox.parallel=11")),
            ("parallel with a word", cl.replace("sparkbox.parallel=1x1", "sparkbox.parallel=1xn")),
            ("parallel empty", cl.replace("sparkbox.parallel=1x1", "sparkbox.parallel=x")),
            ("share left out", cl.replace(" --label sparkbox.gpu-share=0.75", "")),
            ("share is not finite", cl.replace("sparkbox.gpu-share=0.75", "sparkbox.gpu-share=NaN")),
            ("share is zero", cl.replace("sparkbox.gpu-share=0.75", "sparkbox.gpu-share=0.00")),
            ("share is the whole node", cl.replace("sparkbox.gpu-share=0.75", "sparkbox.gpu-share=1.00")),
            ("share is negative", cl.replace("sparkbox.gpu-share=0.75", "sparkbox.gpu-share=-0.75")),
            ("share is a variable", cl.replace("sparkbox.gpu-share=0.75", "sparkbox.gpu-share=$SHARE")),
            ("duplicate share", cl.replace("sparkbox.gpu-share=0.75", "sparkbox.gpu-share=0.75 --label sparkbox.gpu-share=0.87")),
            ("a port that's no number", cl.replace("sparkbox.port=8000", "sparkbox.port=80a0")),
            ("a port too big", cl.replace("sparkbox.port=8000", "sparkbox.port=70000")),
            ("no port on a head", head.replace(" --label sparkbox.port=8000", "")),
            ("no listen on a head", head.replace(" --label sparkbox.listen=192.168.50.154", "")),
            ("a port on a worker", worker.replace(" --label sparkbox.parallel=2x1", " --label sparkbox.parallel=2x1 --label sparkbox.port=8000 --label sparkbox.listen=192.168.50.154")),
            ("maxlen that's no number", cl.replace("sparkbox.maxlen=32768", "sparkbox.maxlen=lots")),
            ("a label with a space", cl.replace("sparkbox.model=gemma-3-27b-it", "'sparkbox.model=gemma 3'")),
            ("a label with a quote", cl.replace("sparkbox.model=gemma-3-27b-it", "'sparkbox.model=a\"b'")),
            ("a label with a semicolon", cl.replace("sparkbox.recipe=gemma-3-27b-it", "'sparkbox.recipe=a;b'")),
            ("a recipe with a space", cl.replace("sparkbox.recipe=gemma-3-27b-it", "'sparkbox.recipe=a b'")),
            ("a recipe with a slash", cl.replace("sparkbox.recipe=gemma-3-27b-it", "sparkbox.recipe=a/b")),
            ("a listen with a space", cl.replace("sparkbox.listen=192.168.50.154", "'sparkbox.listen=a b'")),
            ("an image label with a space", cl.replace("sparkbox.image=vllm/vllm-openai:glm53-flash-arm64-cu130", "'sparkbox.image=a b'")),
            ("a label with a variable", cl.replace("sparkbox.image=vllm/vllm-openai:glm53-flash-arm64-cu130", "sparkbox.image=$IMAGE")),
            ("a path with a dollar", cl.replace(&format!("sparkbox.path={model}"), "'sparkbox.path=/home/$x'")),
            ("a path that isn't the mount", cl.replace(&format!("sparkbox.path={model}"), "sparkbox.path=/home/sparkuser/models/other/x")),
            ("a group with /", cl.replace("sparkbox.group=d1", "sparkbox.group=a/b")),
            ("a group left out", cl.replace(" --label sparkbox.group=d1", "")),
            ("a recipe left out", cl.replace(" --label sparkbox.recipe=gemma-3-27b-it", "")),
            ("a path left out", cl.replace(&format!(" --label sparkbox.path={model}"), "")),
            ("parallel left out", cl.replace(" --label sparkbox.parallel=1x1", "")),
            ("an image label left out", cl.replace(" --label sparkbox.image=vllm/vllm-openai:glm53-flash-arm64-cu130", "")),
            ("another label", cl.replace(" --network host", " --label sparkbox.test=1 --network host")),
            ("labels out of order", cl.replace("--label sparkbox.instance=release --label sparkbox.deployment=d1", "--label sparkbox.deployment=d1 --label sparkbox.instance=release")),
            ("another instance", cl.replace("sparkbox.instance=release", "sparkbox.instance=other")),
            ("an image by name", cl.replace(&id, "vllm/vllm-openai:glm53-flash-arm64-cu130")),
            ("a short id", cl.replace(&id, &format!("sha256:{}", "a".repeat(63)))),
            ("an id in capitals", cl.replace(&id, &format!("sha256:{}", "A".repeat(64)))),
            ("--shm-size", cl.replace(" --gpus all", " --shm-size 16g --gpus all")),
            ("--cap-add", cl.replace(" --gpus all", " --gpus all --cap-add SYS_ADMIN")),
            ("--pid host", cl.replace(" --gpus all", " --gpus all --pid host")),
            ("-u 0", cl.replace(" --gpus all", " --gpus all -u 0")),
            ("-e", cl.replace(" --gpus all", " --gpus all -e LD_PRELOAD=/tmp/x.so")),
            ("-e on a worker", worker.replace(" --gpus all", " --gpus all -e VLLM_API_KEY=x")),
            ("another ulimit", cl.replace(" --ulimit stack=67108864", " --ulimit stack=67108864 --ulimit core=0")),
            ("a ulimit twice", cl.replace(" --ulimit stack=67108864", " --ulimit stack=67108864 --ulimit stack=67108864")),
            ("a ulimit left out", cl.replace(" --ulimit nofile=1048576:1048576", "")),
            ("--gpus twice", cl.replace(" --gpus all", " --gpus all --gpus all")),
            ("--restart always", cl.replace("--restart no", "--restart always")),
            ("Keep running on a head", head.replace("--restart no", "--restart unless-stopped")),
            ("Keep running on a worker", worker.replace("--restart no", "--restart unless-stopped")),
            ("the bridge network", cl.replace("--network host", "--network bridge")),
            ("an unquoted cache mount", cl.replace("-v \"$HOME/.cache/vllm:/root/.cache/vllm\"", "-v $HOME/.cache/vllm:/root/.cache/vllm")),
            ("an unquoted launcher mount", cl.replace(r#"-v "$HOME/.sparkbox/launch-12b1d2f4c5b5565459585085c89dab0171be5d22c0618d3f7a219b871733ddac.py:/run/sparkbox/launch.py:ro""#, "-v $HOME/.sparkbox/launch-12b1d2f4c5b5565459585085c89dab0171be5d22c0618d3f7a219b871733ddac.py:/run/sparkbox/launch.py:ro")),
            ("an unquoted deploy mount", cl.replace(r#"-v "$HOME/.sparkbox/deploy/d1:/run/sparkbox/deploy:ro""#, "-v $HOME/.sparkbox/deploy/d1:/run/sparkbox/deploy:ro")),
            ("an unquoted key mount", cl.replace(r#"-v "$HOME/.sparkbox/keys/d1:/run/sparkbox/key:ro""#, "-v $HOME/.sparkbox/keys/d1:/run/sparkbox/key:ro")),
            ("a cache mount from elsewhere", cl.replace("\"$HOME/.cache/vllm:/root/.cache/vllm\"", "\"$HOME/.ssh:/root/.cache/vllm\"")),
            ("no cache mount", cl.replace(" -v \"$HOME/.triton:/root/.triton\"", "")),
            ("mounts out of order", cl.replace(r#"-v "$HOME/.cache/vllm:/root/.cache/vllm" -v "$HOME/.cache/flashinfer:/root/.cache/flashinfer""#, r#"-v "$HOME/.cache/flashinfer:/root/.cache/flashinfer" -v "$HOME/.cache/vllm:/root/.cache/vllm""#)),
            ("a writable key", cl.replace("/run/sparkbox/key:ro\"", "/run/sparkbox/key\"")),
            ("a model under /etc", with_model("/etc/x/models:/etc/x/models")),
            ("a model under /var/run", with_model("/var/run/x/y:/var/run/x/y")),
            ("a model with ..", with_model("/home/sparkuser/../../etc:/home/sparkuser/../../etc")),
            ("a model with .", with_model("/home/sparkuser/./models:/home/sparkuser/./models")),
            ("a model two parts deep", with_model("/home/sparkuser:/home/sparkuser")),
            ("Docker's socket", with_model("/home/sparkuser/docker.sock:/home/sparkuser/docker.sock")),
            ("a model mounted elsewhere", with_model(&format!("{model}:/models"))),
            ("a writable model", cl.replace(&format!("{model}:ro"), &format!("{model}:rw"))),
            ("a second model mount", cl.replace(" --entrypoint python3", &format!(" -v {model}/x:{model}/x:ro --entrypoint python3"))),
            ("a relative model", with_model("home/sparkuser/models/x:home/sparkuser/models/x")),
            ("a substitution in a label", cl.replace("sparkbox.model=gemma-3-27b-it", "sparkbox.model=$(reboot)")),
            ("a deployment id with /", cl.replace("d1", "a/b")),
            ("a deployment id ..", cl.replace("d1", "..")),
            ("a deployment id .", cl.replace("d1", ".")),
            ("a redirection into a file", format!("{cl} >/tmp/x")),
            ("a quiet redirection", format!("{cl} >/dev/null")),
            ("a redirection on a worker", format!("{worker} 2>&1")),
            // What nothing sends any more.
            ("C-L, with its key", r4_load(true)),
            ("C-L, without a key", r4_load(false)),
            ("C-L, its docker run alone", r4_load(false).replace("mkdir -p ~/.cache/vllm ~/.cache/flashinfer ~/.triton && ", "")),
            ("round 3's form with --privileged", r3_load(true).replace(" --network host", " --privileged --network host")),
            ("round 3's form with caches", r3_load(true).replace(" --gpus all", " --gpus all -v \"$HOME/.cache/vllm:/root/.cache/vllm\"")),
        ];
        for (why, cmd) in &bad {
            assert_eq!(class(Run, cmd), "other", "{why}: `{cmd}`");
            // A tag can't make one of these a load, and neither can sudo's kind.
            assert_eq!(class(Sudo, cmd), "other", "{why} as sudo: `{cmd}`");
        }
        // The classifier agrees it's root, in every shape it might come in.
        for c in [cl, head, worker] {
            assert!(classify::verdict(c).root, "`{c}`");
        }
    }

    /// `deploy-files` by role, and nothing but a plain id and one of three roles.
    #[test]
    fn launch_files_are_named_by_the_role_they_start() {
        use ExecKind::*;
        for (kind, cmd) in [
            (Sparkjob, "deploy-files"),
            (Sparkjob, "deploy-files d1"),
            (Sparkjob, "deploy-files d1 boss"),
            (Sparkjob, "deploy-files d1 single extra"),
            (Sparkjob, "deploy-files .. single"),
            (Sparkjob, "deploy-files . worker"),
            (Sparkjob, "deploy-files ../x head"),
            (Sparkjob, "deploy-files 'a/b' head"),
            (Sparkjob, "deploy-files '$(reboot)' head"),
            (Sparkjob, "deploy-files d1 head > ~/.bashrc"),
            (Sparkjob, "deploy-files d1 head; reboot"),
            (Sparkjob, "deploy-files d1 'head worker'"),
            (SparkjobRoot, "deploy-files d1 head"),
        ] {
            assert_eq!(class(kind, cmd), "other", "{kind:?} `{cmd}`");
        }
        assert_eq!(class(Sparkjob, &format!("deploy-files {} head", "a".repeat(41))), "other", "an id over 40");
    }

    /// `deploy-files` never reads as a read, whichever way it's sent.
    #[test]
    fn launch_files_are_no_read_to_the_classifier() {
        let cmd = format!("{SPARKJOB}deploy-files d1 worker");
        assert_ne!(classify(&cmd), Kind::Read, "`{cmd}`");
        assert!(!classify::read_only(&cmd), "`{cmd}`");
    }

    /// The spec the contracts' literals describe.
    fn contract_spec(nodes: &[&str]) -> crate::models::plan::DeploySpec {
        crate::models::plan::DeploySpec {
            id: "d1".into(),
            engine: "vllm".into(),
            served_name: "gemma-3-27b-it".into(),
            model_path: "/home/sparkuser/models/google/gemma-3-27b-it".into(),
            nodes: nodes.iter().map(|n| n.to_string()).collect(),
            tensor_parallel: nodes.len() as u64,
            pipeline_parallel: 1,
            host: "192.168.50.154".into(),
            port: 8000,
            gpu_memory_utilization: 0.75,
            max_model_len: Some(32768),
            image: "vllm/vllm-openai:glm53-flash-arm64-cu130".into(),
            image_ref: "vllm/vllm-openai:glm53-flash-arm64-cu130".into(),
            image_id: format!("sha256:{}", "a".repeat(64)),
            recipe: "gemma-3-27b-it".into(),
            require_key: true,
            ..Default::default()
        }
    }

    /// plan.rs writes each contract word for word: the fence's shapes are checked against literals,
    /// so a literal that drifted from what's sent would prove nothing.
    #[test]
    fn plan_writes_each_contract_word_for_word() {
        use crate::models::plan::{jumbo_script, preflight_script, rank_command_for, stop_role_command_for, unload_command_for};
        let one = contract_spec(&["cell2"]);
        assert_eq!(rank_command_for(&one, 0, "release"), contracts::C_L);
        assert_eq!(rank_command_for(&crate::models::plan::DeploySpec { require_key: false, ..one.clone() }, 0, "release"), contracts::c_l_no_key());
        let two = crate::models::plan::DeploySpec { docker_access: "privileged".into(), ..contract_spec(&["cell2", "cell3"]) };
        assert_eq!(rank_command_for(&two, 0, "release"), contracts::C_L_HEAD);
        assert_eq!(rank_command_for(&two, 1, "release"), contracts::C_L_WORKER);
        assert_eq!(unload_command_for("d1", "release"), contracts::C_U);
        assert_eq!(stop_role_command_for("d1", "head", "release"), contracts::C_S);
        assert_eq!(preflight_script(&two), contracts::C_P);
        let halves = [("enp1s0f1np1".to_string(), "192.168.100.1".to_string()), ("enP2p1s0f1np1".to_string(), "192.168.101.1".to_string())];
        assert_eq!(jumbo_script(&halves), contracts::C_J);
    }

    /// C-P2 and C-J only read, to the shapes and to the classifier, however Live sends them.
    #[test]
    fn the_preflight_and_the_jumbo_ping_are_reads() {
        use ExecKind::*;
        let one = preflight_script_for_one();
        for cmd in [contracts::C_P, contracts::C_J, one.as_str()] {
            assert_eq!(classify(cmd), Kind::Read, "`{cmd}`");
            assert!(reads(cmd), "`{cmd}`");
            for kind in [Run, Probe, Cancellable, Stream] {
                assert_eq!(class(kind, cmd), "read", "{kind:?} `{cmd}`");
            }
        }
    }

    fn preflight_script_for_one() -> String {
        crate::models::plan::preflight_script(&contract_spec(&["cell2"]))
    }

    #[test]
    fn an_unload_is_exactly_c_u2_or_c_s() {
        use ExecKind::*;
        let (cu, cs) = (contracts::C_U, contracts::C_S);
        for (why, cmd) in [
            ("rm -f", cu.replace("docker rm $ids", "docker rm -f $ids")),
            ("C-S with rm -f", cs.replace("docker rm $ids", "docker rm -f $ids")),
            ("no stop", cu.replace("[ -z \"$ids\" ] || docker stop -t 30 $ids >/dev/null; ", "")),
            ("no time to finish", cu.replace("-t 30", "-t 0")),
            ("docker kill", cu.replace("docker stop -t 30", "docker kill")),
            ("rm -rf ~", cu.replace("rm -rf ~/.sparkbox/keys/d1 ~/.sparkbox/deploy/d1", "rm -rf ~")),
            ("rm -rf ~ beside them", cu.replace("~/.sparkbox/deploy/d1;", "~/.sparkbox/deploy/d1 ~;")),
            ("rm -rf of the whole folder", cu.replace("~/.sparkbox/keys/d1", "~/.sparkbox/keys")),
            ("rm -rf of the keys and deploy folders", cu.replace("~/.sparkbox/keys/d1 ~/.sparkbox/deploy/d1", "~/.sparkbox/keys ~/.sparkbox/deploy")),
            ("a key traversal", cu.replace("~/.sparkbox/keys/d1", "~/.sparkbox/keys/../../.ssh")),
            ("a deploy traversal", cu.replace("~/.sparkbox/deploy/d1", "~/.sparkbox/deploy/../..")),
            ("another load's key", cu.replace("rm -rf ~/.sparkbox/keys/d1", "rm -rf ~/.sparkbox/keys/d2")),
            ("another load's files", cu.replace("~/.sparkbox/deploy/d1", "~/.sparkbox/deploy/d2")),
            ("C-U's rm -f", cu.replace("rm -rf ~/.sparkbox/keys/d1 ~/.sparkbox/deploy/d1", "rm -f ~/.sparkbox/keys/d1")),
            ("rm -rf with a third path", cu.replace("~/.sparkbox/deploy/d1;", "~/.sparkbox/deploy/d1 ~/models;")),
            ("the paths swapped", cu.replace("~/.sparkbox/keys/d1 ~/.sparkbox/deploy/d1", "~/.sparkbox/deploy/d1 ~/.sparkbox/keys/d1")),
            ("C-S removing the files", cs.replace("; echo ok", "; rm -rf ~/.sparkbox/keys/d1 ~/.sparkbox/deploy/d1; echo ok")),
            ("C-U2 keeping the files", cu.replace(" rm -rf ~/.sparkbox/keys/d1 ~/.sparkbox/deploy/d1;", "")),
            ("another instance", cu.replace("instance=release", "instance=x")),
            ("no instance", cu.replace("--filter label=sparkbox.instance=release ", "")),
            ("another filter", cu.replace("deployment=d1)", "deployment=d1 --filter label=sparkbox.test=1)")),
            ("another role", cs.replace("role=head", "role=x")),
            ("a step after", format!("{cu}; reboot")),
            ("a deployment id with /", cu.replace("d1", "a/b")),
            ("a deployment id ..", cu.replace("d1", "..")),
            ("a deployment id .", cu.replace("d1", ".")),
            ("a deployment id with ../", cu.replace("d1", "../x")),
            ("a deployment id in the rm only", cu.replace("keys/d1", "keys/..").replace("deploy/d1", "deploy/..")),
            ("a deployment id with a glob", cu.replace("d1", "*")),
            ("a deployment id with a variable", cu.replace("d1", "$HOME")),
            ("two spaces", cu.replace("ps -aq", "ps  -aq")),
        ] {
            assert_eq!(class(Run, &cmd), "other", "{why}: `{cmd}`");
        }
    }

    /// T2 blocker 2: the shape parser refuses what it doesn't follow, outside single quotes, so a
    /// command that would be files-test or process-test without it is `other` with it. Inside
    /// single quotes each is plain text.
    #[test]
    fn the_shape_parser_refuses_syntax_it_doesnt_follow() {
        use ExecKind::*;
        let k = "kill -s TERM 4242";
        let mut rows: Vec<String> = vec![
            format!("({k})"),
            format!("{k} )"),
            format!("{{ {k}; }}"),
            format!("{k} }}"),
            format!("{k} #; reboot"),
            format!("{k} <<EOF\nx\nEOF"),
            format!("{k} <<< x"),
            format!("{k} <<<x"),
            format!("{k} 2<<EOF\nx\nEOF"),
            format!("{k} <> /tmp/x"),
            format!("{k} >| /tmp/x"),
            format!("{k} |& cat"),
            format!("{k} &> /tmp/x"),
            format!("{k};; x"),
            format!("{k} <(x)"),
            format!("{k} >(x)"),
            "kill -s TERM $((4242))".into(),
            "kill -s TERM $[4242]".into(),
            "kill -s $'TERM' 4242".into(),
            "kill -s $\"TERM\" 4242".into(),
            "kill -s ${S:-TERM} 4242".into(),
            "kill -s ${#S} 4242".into(),
            "kill -s \"${S:-TERM}\" 4242".into(),
            "kill -s \"$((1))\" 4242".into(),
            "kill -s \"(TERM)\" 4242".into(),
            "kill -s \"{TERM}\" 4242".into(),
            format!("if true; then {k}; fi"),
            format!("for x in 1; do {k}; done"),
            format!("while true; do {k}; done"),
            format!("until false; do {k}; done"),
            format!("! {k}"),
            format!("time {k}"),
            format!("coproc {k}"),
            format!("function f {{ {k}; }}"),
            format!("select x in 1; do {k}; done"),
            format!("[[ -f x ]] && {k}"),
            format!("case x in x) {k};; esac"),
            format!("then {k}"),
            format!("else {k}"),
            format!("elif {k}"),
            format!("fi; {k}"),
            format!("do {k}"),
            format!("done; {k}"),
            format!("esac; {k}"),
            format!("in {k}"),
            format!("]] ; {k}"),
            format!("{k}\r"),
            "kill -s TERM\u{b}4242".into(),
            "kill -s TERM\u{1b}4242".into(),
            "mkdir -- /home/sparkuser/sparkbox-test/{a,b}".into(),
            "mkdir -- /home/sparkuser/sparkbox-test/a # /home/sparkuser/models/x".into(),
        ];
        for ws in ['\u{85}', '\u{a0}', '\u{1680}', '\u{2000}', '\u{2005}', '\u{200a}', '\u{2028}', '\u{2029}', '\u{202f}', '\u{205f}', '\u{3000}', '\u{feff}'] {
            rows.push(format!("kill -s TERM{ws}4242"));
            rows.push(format!("mkdir --{ws}/home/sparkuser/sparkbox-test/new"));
        }
        for cmd in &rows {
            assert!(segments(cmd).is_none(), "`{cmd}`");
            assert_eq!(class(Run, cmd), "other", "`{cmd}`");
        }
        // Inside single quotes it's text: the same names are still files-test.
        for name in ["(a)", "{a,b}", "#a", "a<<b", "a|&b", "$((1))", "${X:-y}", "if", "a\rb", "a\u{a0}b"] {
            let cmd = format!("mkdir -- '/home/sparkuser/sparkbox-test/{name}'");
            assert_eq!(class(Run, &cmd), "files-test", "`{cmd}`");
        }
        // What it keeps: words, both quotes, escapes and a continued line, `$(…)` and backticks,
        // `$NAME`, `${NAME}`, `$?`, `$1`, the separators and the redirections, each one marked
        // word with its target (spaced or not), a quoted `>` an argument.
        let w = |cmd: &str| segments(cmd).unwrap();
        assert_eq!(w("a \\\n b"), vec![vec!["a".to_string(), "b".into()]]);
        assert_eq!(w("echo $X ${HOME} $? $1 \"${A}b\""), vec![vec!["echo".to_string(), "$X".into(), "${HOME}".into(), "$?".into(), "$1".into(), "${A}b".into()]]);
        let r = |t: &str| format!("{REDIRECT}{t}");
        assert_eq!(w("a > f; b >> f; c < f; d 2> f; e 2>> f; g >&2; h 2>&1 & i"), vec![
            vec!["a".to_string(), r(">f")],
            vec!["b".into(), r(">>f")],
            vec!["c".into(), r("<f")],
            vec!["d".into(), r("2>f")],
            vec!["e".into(), r("2>>f")],
            vec!["g".into(), r(">&2")],
            vec!["h".into(), r("2>&1")],
            vec!["i".into()],
        ]);
        assert_eq!(w("a >'f g' 2>\"$H\"/x 1>&2 3< f"), vec![vec!["a".to_string(), r(">f g"), r("2>$H/x"), r("1>&2"), r("3<f")]]);
        assert_eq!(w("a '>f' \\>f \">\" 'x>f'"), vec![vec!["a".to_string(), ">f".into(), ">f".into(), ">".into(), "x>f".into()]]);
        assert_eq!(w("a `b c` \"$(d \"e\")\""), vec![vec!["a".to_string(), "`b c`".into(), "$(d \"e\")".into()]]);
    }

    /// Every command SparkBox sends with a specific class keeps it under the fail-closed parser
    /// (round 3's and this round's Contracts, the lease, T2's shapes); the two with `if … fi`
    /// (Files › Rename and Updates' Cancel) are recognised by their exact text.
    #[test]
    fn every_literal_keeps_its_class() {
        use ExecKind::*;
        let rename = crate::files::rename_command("/home/sparkuser/sparkbox-test/a b.txt", "/home/sparkuser/sparkbox-test/it's.txt");
        let rows: Vec<(ExecKind, String, &str)> = vec![
            (Sudo, contracts::C1_PINNED.into(), "apt-one"),
            (Sudo, contracts::C3.into(), "apt-refresh"),
            (Sudo, contracts::C4_HOLD.into(), "apt-hold"),
            (Sudo, contracts::C4_UNHOLD.into(), "apt-hold"),
            (Run, contracts::c7("dev"), "container-test"),
            (Sparkjob, contracts::c8("~/sparkbox-test/new.txt"), "files-test"),
            (Run, contracts::C_L.into(), "model-load-one"),
            (Run, contracts::c_l_no_key(), "model-load-one"),
            (Run, contracts::C_U.into(), "model-unload"),
            (Run, contracts::C_S.into(), "model-unload"),
            (Sudo, "sync; echo 3 > /proc/sys/vm/drop_caches".into(), "drop-caches"),
            (Sudo, "sync && echo 3 > /proc/sys/vm/drop_caches".into(), "drop-caches"),
            (Sparkjob, "lease claim 'dev' 'g0' 'Studio' 45".into(), "bookkeeping"),
            (Sparkjob, "lease release 'dev'".into(), "bookkeeping"),
            (Sudo, "nmcli radio wifi off".into(), "wifi"),
            (Run, "kill -s TERM 4242".into(), "process-test"),
            (Run, "renice -n 5 -p 4242".into(), "process-test"),
            (Run, "mkdir -- '/home/sparkuser/sparkbox-test/new'".into(), "files-test"),
            (Run, "chmod -- 600 '/home/sparkuser/sparkbox-test/notes.txt'".into(), "files-test"),
            (Run, rename.clone(), "files-test"),
        ];
        for (kind, cmd, want) in &rows {
            assert_eq!(class(*kind, cmd), *want, "{kind:?} `{cmd}`");
        }
        assert_eq!(file_op_words(&rename).as_deref(), Some("Rename a b.txt to it's.txt"));
        // A rename one character off isn't Files' rename, and one out of the folder isn't files-test.
        for bad in [rename.replace("exit 1", "exit 2"), rename.replacen("[ -e", "[ -f", 1), format!("{rename}; reboot"), crate::files::rename_command("/home/sparkuser/sparkbox-test/a", "/home/sparkuser/models/a")] {
            assert_eq!(class(Run, &bad), "other", "`{bad}`");
        }
        // Updates' Cancel: its unit, exactly; a look-alike or another unit isn't it.
        let cancel = crate::ops::updates::cancel_command("sparkbox-upgrade-t1");
        assert_eq!(upgrade_cancel_unit(&cancel).as_deref(), Some("sparkbox-upgrade-t1"));
        assert_eq!(upgrade_cancel_unit(&crate::ops::updates::cancel_command("sparkbox-upgrade-a1-B2")).as_deref(), Some("sparkbox-upgrade-a1-B2"));
        for bad in [
            cancel.replace("exit 3", "exit 4"),
            cancel.replace("-TERM", "-KILL"),
            cancel.replacen("apt-get", "apt-gex", 1),
            format!("{cancel} "),
            format!(" {cancel}"),
            format!("{cancel}; reboot"),
            crate::ops::updates::cancel_command("sparkbox-upgrade-"),
            crate::ops::updates::cancel_command("other-unit"),
            crate::ops::updates::cancel_command("sparkbox-upgrade-t1;reboot"),
            crate::ops::updates::cancel_command("sparkbox-upgrade-t1 x"),
        ] {
            assert_eq!(upgrade_cancel_unit(&bad), None, "`{bad}`");
        }
    }

    // ---- P1 round 4 fixes (R4-SAFE)

    /// The shapes read a redirection as dash does (blocker 2): one with any operator and target,
    /// added to any shape's command, opens a file the shape never named, so the command is
    /// `other` (a shell over exec_io): no class, and never a read. A read row stays a read only
    /// with an input redirection. The literals' own redirections (C-U2's `>/dev/null`, C1′'s package
    /// list, the drop's target) still match, compared whole.
    #[test]
    fn every_shape_refuses_a_redirection_it_doesnt_name() {
        use ExecKind::*;
        // (A login shell's empty command with a redirection is another command, not its shape.)
        for (kind, cmd, want) in positive_rows().into_iter().filter(|(_, c, _)| !c.is_empty()) {
            for tail in [" >/home/sparkuser/.bashrc", " > x", " 2>x", " >>/home/sparkuser/.ssh/authorized_keys", " </x"] {
                let c = format!("{cmd}{tail}");
                let got = class(kind, &c);
                // Input from a file is still a read (as root too).
                if matches!(want, "read" | "sudo-probe") && tail == " </x" {
                    assert!(matches!(got, "read" | "sudo-probe" | "other" | "shell"), "{kind:?} `{c}`: {got}");
                } else {
                    assert!(got == "other" || (got == "shell" && matches!(kind, ExecIo | Pty | DirectTcpip)), "{kind:?} `{c}`: {got}");
                }
            }
        }
        // The verifiers' rows: a write to any file through each shape that takes free words.
        let h = "/home/sparkuser";
        for (kind, cmd, want) in [
            (ExecIo, "tar -C / -cvf - -- etc/hostname >~/victim".to_string(), "shell"),
            (ExecIo, format!("tar -C '{h}' -cvf - -- 'x' >'{h}/victim'"), "shell"),
            (ExecIo, format!("tar -C >'{h}/victim' -cvf - -- 'x'"), "shell"),
            (Run, "mkdir -- ~/sparkbox-test/x>~/.bashrc".into(), "other"),
            (Run, "mkdir -- ~/sparkbox-test/x >~/.bashrc".into(), "other"),
            (Run, "chmod -- 644 ~/sparkbox-test/x>~/.ssh/authorized_keys".into(), "other"),
            (Run, "mkdir -p ~/.sparkbox/x>~/.bashrc".into(), "other"),
            (Run, "mkdir -p ~/.sparkbox/x >~/.sparkbox/y".into(), "other"),
            (Run, "python3 ~/.sparkbox/sparkjob.py list > ~/.bashrc".into(), "other"),
            (Sparkjob, "read ~/x 2>~/.ssh/authorized_keys".into(), "other"),
            (Sparkjob, "list >~/.bashrc".into(), "other"),
            (Sparkjob, "check '~/models' 'img' 'img-ray' quick >~/.bashrc".into(), "other"),
            (Sparkjob, format!("{} --nonce 7 >{h}/.bashrc", contracts::c8("~/sparkbox-test/new.txt")), "other"),
            (Sparkjob, "commit '/home/sparkuser/sparkbox-test/a' --sha256 >/home/sparkuser/.bashrc ab".into(), "other"),
            (Run, format!("{} >{h}/.bashrc", contracts::c_l_no_key()), "other"),
            (Run, format!("{} 2>{h}/.profile", contracts::c_l_no_key()), "other"),
            (Run, format!("{} >{h}/.bashrc", contracts::C_L), "other"),
            (Sudo, "systemctl restart earlyoom.service >/etc/x".into(), "other"),
            (Sudo, "apt-mark hold 'libfoo' >/etc/x".into(), "other"),
            (Sudo, "shutdown -c >/etc/passwd".into(), "other"),
            (Sudo, "nmcli radio wifi off >/etc/x".into(), "other"),
            (Sudo, "systemctl daemon-reload 2>/etc/x".into(), "other"),
            (Run, format!("{} >'{h}/victim2'", contracts::c7("dev")), "other"),
            (Run, "docker stop -t >x sparkbox-test-sleep".into(), "other"),
            (Run, "docker stop sparkbox-test-sleep >x".into(), "other"),
            (ContainerAction, "docker stop sparkbox-test-sleep >x".into(), "other"),
            (Run, "kill -s TERM 4242 >x".into(), "other"),
            (Run, "renice -n 5 -p 4242 >x".into(), "other"),
            (Run, "docker run -d --name sparkbox-test-x --label sparkbox.test=1 >x busybox sleep 1".into(), "other"),
            (Run, "SPARKBOX_AGENT_STATE=/var/lib/sparkbox-agent python3 -I /opt/sparkbox-agent/sparkbox_agent.py status >~/.bashrc".into(), "other"),
            (Run, "python3 -I /opt/sparkbox-agent/sparkbox_agent.py status; read x </tmp/o; git log \"$x b\"".into(), "other"),
        ] {
            assert_eq!(class(kind, &cmd), want, "{kind:?} `{cmd}`");
        }
        // Quieting or merging output is no file: the free-word shapes keep their class.
        for (kind, cmd, want) in [
            (Sparkjob, "ls '/home/sparkuser' 2>/dev/null", "read"),
            (Sparkjob, "read '/home/sparkuser/notes.txt' 2>&1", "read"),
            (Run, "docker stop sparkbox-test-sleep >/dev/null 2>&1", "container-test"),
            (ExecIo, "tar -C '/home/sparkuser/sparkbox-test' -cvf - -- 'notes.txt' 2>/dev/null", "read"),
            (Run, "mkdir -- '/home/sparkuser/sparkbox-test/new' 2>&1", "files-test"),
        ] {
            assert_eq!(class(kind, cmd), want, "{kind:?} `{cmd}`");
        }
    }

    /// What the shape parser won't read as a redirection, and refuses: one run into the word before
    /// it (dash writes the file whatever the shape makes of the word), bash's `>&file`, a target
    /// that would read as part of its operator, a redirection with no target.
    #[test]
    fn a_glued_or_bash_only_redirection_isnt_followed() {
        for cmd in [
            "mkdir -- ~/sparkbox-test/x>~/.bashrc",
            "kill -s TERM 4242>x",
            "kill -s TERM 4242 '2'>x",
            "kill -s TERM 4242 \\2>x",
            "kill -s TERM 4242 >&/tmp/x",
            "kill -s TERM 4242 >& /tmp/x",
            "kill -s TERM 4242 1>&/tmp/x",
            "kill -s TERM 4242 >&2x",
            "kill -s TERM 4242 >>&2",
            "kill -s TERM 4242 > '>x'",
            "kill -s TERM 4242 > \\&x",
            "kill -s TERM 4242 >",
            "kill -s TERM 4242 >; true",
            "kill -s TERM 4242 > >x",
        ] {
            assert!(segments(cmd).is_none(), "`{cmd}`");
            assert_eq!(class(ExecKind::Run, cmd), "other", "`{cmd}`");
        }
    }

    /// The build a C-L2 labels its container for: what the exit's check (d) compares with its own.
    #[test]
    fn a_load_names_the_build_it_labels_its_container_for() {
        use ExecKind::*;
        assert_eq!(load_instance(Run, contracts::C_L).as_deref(), Some("release"));
        assert_eq!(load_instance(Run, &contracts::c_l_no_key().replace("instance=release", "instance=dev")).as_deref(), Some("dev"));
        assert_eq!(load_instance(Run, &format!("sh -c {}", crate::live::sh_quote(contracts::C_L))).as_deref(), Some("release"));
        assert_eq!(load_instance(Run, contracts::C_L_HEAD).as_deref(), Some("release"));
        assert_eq!(load_instance(Run, &contracts::C_L_WORKER.replace("instance=release", "instance=dev")).as_deref(), Some("dev"));
        assert_eq!(load_instance(Cancellable, contracts::C_L_WORKER).as_deref(), Some("release"));
        for (kind, cmd) in [(Run, r3_load(true)), (Run, r4_load(true)), (Run, r4_load(false)), (Run, contracts::C_U.to_string()), (Run, contracts::c7("release")), (Run, format!("{}; true", contracts::C_L)), (Sudo, contracts::C_L.to_string()), (DirectTcpip, contracts::C_L.to_string()), (Run, "uptime".into())] {
            assert_eq!(load_instance(kind, &cmd), None, "{kind:?} `{cmd}`");
        }
    }

    /// The `other` commands a call site's tag may name (`tag_names`), each exactly as SparkBox
    /// writes it, under its own class and way of running; a look-alike or another class isn't one.
    #[test]
    fn a_tag_names_only_its_own_literals() {
        use ExecKind::*;
        let cancel = crate::ops::updates::cancel_command("sparkbox-upgrade-t1");
        let guard = crate::ops::pid_guard_cmd(4242, "TERM", Some(98_765_432), "/proc");
        for (kind, cmd, c) in [(Sudo, cancel.clone(), FenceClass::AptOne), (Run, guard.clone(), FenceClass::ProcessTest), (Sudo, crate::live::DOCTOR_SSHD_T.into(), FenceClass::SudoProbe)] {
            assert_eq!(class(kind, &cmd), "other", "`{cmd}`: its shape alone");
            assert!(tag_names(kind, &cmd, c), "{c:?} `{cmd}`");
        }
        for sig in ["TERM", "INT", "HUP", "KILL", "STOP", "CONT"] {
            assert!(guarded_signal(&crate::ops::pid_guard_cmd(77, sig, Some(1), "/proc")), "{sig}");
        }
        assert!(guarded_signal(&crate::ops::pid_guard_cmd(77, "TERM", Some(1), "/tmp/sparkbox-x-1/proc")), "a test's copy of /proc");
        for bad in [
            guard.replace("kill -s TERM 4242", "kill -s TERM 4242; reboot"),
            guard.replace("exit 3", "exit 4"),
            format!("{guard} "),
            guard.replace("4242", "1"),
            crate::ops::pid_guard_cmd(4242, "USR1", Some(1), "/proc"),
            crate::ops::pid_guard_cmd(4242, "TERM", Some(1), "/proc/../tmp"),
            crate::ops::pid_guard_cmd(4242, "TERM", Some(1), "/proc;reboot"),
            crate::ops::pid_guard_cmd(4242, "TERM", None, "/proc"),
            "kill -s TERM 4242".into(),
        ] {
            assert!(!guarded_signal(&bad), "`{bad}`");
            assert!(!tag_names(Run, &bad, FenceClass::ProcessTest), "`{bad}`");
        }
        for (kind, cmd, c) in [
            (Run, cancel.clone(), FenceClass::AptOne),
            (Sudo, cancel.clone(), FenceClass::Service),
            (Sudo, cancel.replace("exit 3", "exit 4"), FenceClass::AptOne),
            (Sudo, guard.clone(), FenceClass::ProcessTest),
            (Run, guard.clone(), FenceClass::Fleet),
            (Run, crate::live::DOCTOR_SSHD_T.into(), FenceClass::SudoProbe),
            (Sudo, format!("{} ; reboot", crate::live::DOCTOR_SSHD_T), FenceClass::SudoProbe),
            (Sudo, "touch /tmp/x".into(), FenceClass::AptOne),
            (Run, "touch /tmp/x".into(), FenceClass::ModelLoadOne),
        ] {
            assert!(!tag_names(kind, &cmd, c), "{kind:?} {c:?} `{cmd}`");
        }
    }

    /// Check (d) peels each wrapper with its own options (`classify::wrapped`), so a flag that
    /// takes no word doesn't swallow `docker` (`command -p docker stop …`).
    #[test]
    fn check_d_peels_each_wrapper_with_its_own_options() {
        use ExecKind::*;
        let containers = [container("aaaaaaaaaaaa1111", "gemma-cell2", &[("sparkbox.instance", "release")])];
        let ctx = Ctx { containers: &containers, home: Some("/home/sparkuser"), root_lib: LIB, sparkjob_sha: SHA };
        let gemma = vec!["gemma-cell2".to_string(), "aaaaaaaaaaaa1111".to_string()];
        for cmd in [
            "command -p docker stop gemma-cell2",
            "exec -c docker rm gemma-cell2",
            "exec -a x docker rm gemma-cell2",
            "taskset -c 0 docker stop gemma-cell2",
            "taskset 0x1 docker stop gemma-cell2",
            "chrt -i 0 docker kill gemma-cell2",
            "nice -n 5 docker stop gemma-cell2",
            "nice -5 docker stop gemma-cell2",
            "ionice -c 3 -t docker stop gemma-cell2",
            "stdbuf -o L docker stop gemma-cell2",
            "numactl -N 0 docker stop gemma-cell2",
        ] {
            assert_eq!(named_containers(Run, cmd, &ctx), vec![gemma.clone()], "`{cmd}`");
        }
    }

    /// Check (d) still finds the containers a command names in syntax `segments` refuses.
    #[test]
    fn check_d_looks_through_syntax_the_shapes_refuse() {
        use ExecKind::*;
        let containers = [container("aaaaaaaaaaaa1111", "gemma-cell2", &[("sparkbox.instance", "release")])];
        let ctx = Ctx { containers: &containers, home: Some("/home/sparkuser"), root_lib: LIB, sparkjob_sha: SHA };
        let gemma = ["gemma-cell2".to_string(), "aaaaaaaaaaaa1111".to_string()];
        for cmd in ["{ docker rm gemma-cell2; }", "if true; then docker stop gemma-cell2; fi", "docker kill gemma-cell2 # x", "(docker restart gemma-cell2)", "! docker pause gemma-cell2"] {
            assert!(segments(cmd).is_none(), "`{cmd}`");
            assert!(named_containers(Run, cmd, &ctx).iter().any(|i| *i == gemma), "`{cmd}`");
        }
    }
}
