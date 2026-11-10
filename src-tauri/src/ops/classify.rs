//! Does a shell command change anything, and could it destroy data or take a node down? Used
//! by the activity log (to hide reads) and by Fleet Command (its Read / Change / Destructive
//! confirmation tiers). The command is split into its parts — each piece of a pipeline, a `;`,
//! `&&` or `||` list, every `$(…)`, backtick and subshell — and the most severe part decides:
//! - read-only is an explicit allowlist: every part is a known read-only command used in a
//!   read-only way, and nothing is redirected into a file;
//! - destructive is a denylist: disk writers on block devices, power-off and reboot, killing
//!   PID 1, Docker volume and system prunes, fork bombs, writes to /dev/sd* or /dev/nvme*;
//! - "cuts access" is destructive too, and also means SparkBox may not get back in: stopping
//!   or removing SSH, taking the network down, firewall flushes and drop policies, writes to
//!   /proc/sysrq-trigger or to /etc/passwd, shadow, group, sudoers, fstab or sshd_config,
//!   password and account locks, and moving, deleting or recursively re-owning the system
//!   folders or a home. Fleet runs these on one node first and waits until it can log in to
//!   that node again over SSH (power verbs on several nodes count too);
//! - everything else, and anything unknown, is a change. A command whose quoting doesn't
//!   balance can't be read, so it counts as destructive (fail closed).
//!
//! What runs is decided by the command word (T2 blocker 1). One the node makes after SparkBox
//! has looked — a `$(…)` or backtick, a variable in any form, an unquoted glob, a brace, a leading
//! `~` — is destructive ("The command it runs comes from a substitution or a variable…"); one that
//! is a path outside /bin, /sbin, /usr/bin, /usr/sbin, /usr/local/bin and /usr/local/sbin names
//! some other file, so it's never a read. Syntax neither this nor the fence's shape parser follows
//! (`parse`: arithmetic, `$'…'`, process substitution, `|&`, brace expansion, coproc, function,
//! select, control characters, non-ASCII spaces) is destructive too, so a power verb hidden that
//! way is a destructive Fleet command, never power-one (the named limit). Builtins that change
//! what later commands run (alias, hash, trap, eval, source, set…) are never reads; a trap's
//! action and an alias's text are judged as scripts.
//!
//! Wrappers are peeled to the command they run: sudo, env, nice, timeout, xargs, `sh -c` (and
//! `bash -lc`; the script is the first operand after all the shell's options, wherever `-c`
//! sits among them: `sh -c -e reboot`, `bash -c -- reboot`), `su -c`, chroot, nsenter, unshare, setsid, flock (and `flock … -c`), systemd-run
//! (with -H it runs on another machine: destructive) and the job `at`/`batch` schedule (a
//! here-string or what echo/printf pipe in; one SparkBox can't read is destructive). A shell
//! reading its script from stdin runs what it's fed the same way (`echo reboot | sh`,
//! `bash <<< reboot`); fed anything else it's a change. `ssh <host> <cmd>` is the command, run on
//! another machine: never Read, its power and severity carried through. Power is the verbs,
//! logind's and systemd's power methods over busctl, gdbus or dbus-send (…WithFlags,
//! ScheduleShutdown, StartUnit of a power target) and starting or isolating
//! reboot/poweroff/halt/kexec targets. Docker is root (`Verdict::root`): `docker run`/`create`/
//! `exec`, also as `docker container …` and with docker's short options read the way its CLI
//! reads them (`-v/:/host`, `-itu0`), and `docker compose run|exec|up|create`, whose compose file
//! SparkBox doesn't see.
//!
//! Read-only is an allowlist of the tools, and the ways of using them, these rules know only
//! look: `sed` scripts with `w`, `e` or `s///e`, awk programs with `system`, pipes, `print >` or
//! any `@` (gawk directives and indirect calls), interpreters with `-e`/`-c`, `nvidia-smi -f`,
//! `ss -K`, `ip -batch`, `git --output`, `git grep -O`/`--open-files-in-pager` however spelled,
//! `curl -w %output` and `-w @file`, `less -o`, `sparkjob.py check` other than the quick one, and
//! every variable assignment except LC_*, LANG, TZ, COLUMNS, TERM and NO_COLOR (and `sudo -E`, which
//! keeps the caller's) all count as changes. Options are read the way getopt reads them (`getopt`):
//! `-s2030-01-01` is `-s` with a value and `-Hc` is `-H -c`, so a write can't hide in an
//! attached value or a cluster, and getopt_long takes any unique start of a long option, so one
//! that's barred is barred however it's shortened (`names_option`: `sed --in`, `sort --o=f`).
//! The tools that can write or run something by an option are allowlists instead, where an option
//! these rules don't know isn't a read: sed, sort, tar (a listing with only -f, -v and compression
//! letters), awk (only -F, -v and the program), curl (only http, https and file), wget, journalctl,
//! nvidia-smi, apt-cache and apt-config (their looking verbs, with no -c, -o, -p or -s), blkid (its
//! cache only /dev/null), rg, tree, lspci, ethtool, tc, rdma, the mlx tools, and systemctl and its
//! family, whose options are read the way their getopt reads them (`known_options`), so a value
//! can't pass for the verb (`systemctl -p status stop x` stops x). git, docker, docker compose and
//! buildx, ip, nmcli and tc take only options spelled out before their verb (`past_exact_options`):
//! `git --namespace log commit` commits, and ip and nmcli take the start of a name (`ip -fa inet l
//! del x`, `nmcli -fi radio connection delete x`). apt's `changelog -d` writes a file. Partition
//! tools list only when every option is a listing one.
//!
//! A rule reads words, and a word the shell builds at run time (`Built`: a `$(…)`, a variable, a
//! glob) isn't there to read, so with one a tool with a rule isn't a read (`built_word_counts`):
//! `find . $(echo -delete)` deletes. READ_ANY's tools with no rule stay reads.
//!
//! `READ_ANY` is the rest: tools with no option that writes or runs anything, each one read in its
//! man page and tried on the real binary on Ubuntu 24.04. `read_tier_every_tool_is_accounted_for`
//! (ops/tests.rs) holds the table of what was found, one row per tool, and fails on a tool without
//! a row. The real tools' half is `staging_read_tier_options`.
//!
//! SparkBox's own background checks are tagged `Probe` where they run (`audit::as_probe`), never
//! inferred from the command's text.

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// SparkBox's own background checks (latency ping, collector install, job polling).
    Probe,
    /// Only looks.
    Read,
    /// May change something on the node.
    Change,
    /// Can destroy data or take the node down.
    Destructive,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Probe => "probe",
            Kind::Read => "read",
            Kind::Change => "change",
            Kind::Destructive => "destructive",
        }
    }
}

const UNPARSEABLE: &str = "SparkBox can't read this command's quoting, so it treats it as destructive.";
/// A command word that is a substitution, a variable, a glob, a brace or a `~` path: what runs
/// is decided on the node, after SparkBox has looked (T2 blocker 1).
const HIDDEN: &str = "The command it runs comes from a substitution or a variable, so SparkBox can't tell what it is; it treats it as destructive.";
/// In every reason `syntax_reason` writes.
const UNFOLLOWED: &str = "shell syntax SparkBox doesn't follow";
/// What a `$(…)` or backtick substitution leaves in its word (its commands are checked on their own).
const SUB: &str = "$(…)";

/// Why `parse` couldn't read a command: its quoting doesn't balance, or it uses shell syntax
/// SparkBox doesn't follow (what, in a few words).
enum Unparsed {
    Quoting,
    Syntax(&'static str),
}

fn syntax_reason(what: &str) -> String {
    format!("This command uses {UNFOLLOWED} ({what}), so it treats it as destructive.")
}

/// A reason that says SparkBox couldn't follow the text, not what it does (`embedded` skips these:
/// a program's string literals aren't shell commands).
fn unfollowed(v: &Verdict) -> bool {
    v.reason.as_deref().is_some_and(|r| r == UNPARSEABLE || r == HIDDEN || r.contains(UNFOLLOWED))
}

/// Everything Fleet's confirmation needs to know about a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub kind: Kind,
    /// Why it's destructive (Fleet's red banner).
    pub reason: Option<String>,
    /// It may leave SparkBox unable to log in to the node again (a kind of destructive).
    pub cuts_access: bool,
    /// It restarts or switches off the node (on several nodes at once, that cuts access too).
    pub power: Option<Power>,
    /// It runs with the node's root through Docker (Docker is root-equivalent): `docker run`/
    /// `create`/`exec` with `--privileged`, host pid/network, `--cap-add`, `--device`, a host
    /// bind mount, or uid 0. Fleet treats it as root (the typed count, the canary, one node at
    /// a time) and the activity log tags it. `escalates()` is true for these too (P1.42).
    pub root: bool,
}

/// What a power verb does to the node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Power {
    Restart,
    Off,
}

impl Verdict {
    fn of(kind: Kind) -> Verdict {
        Verdict { kind, reason: None, cuts_access: false, power: None, root: false }
    }

    fn destructive(why: String) -> Verdict {
        Verdict { kind: Kind::Destructive, reason: Some(why), cuts_access: false, power: None, root: false }
    }

    fn cuts(why: String) -> Verdict {
        Verdict { kind: Kind::Destructive, reason: Some(why), cuts_access: true, power: None, root: false }
    }
}

pub fn classify(cmd: &str) -> Kind {
    verdict(cmd).kind
}

/// The class, why, and whether it cuts SparkBox's access or powers the node down.
pub fn verdict(cmd: &str) -> Verdict {
    verdict_at(cmd, 0)
}

fn verdict_at(cmd: &str, depth: usize) -> Verdict {
    // Only the shell's own blanks: a trailing CR or NBSP is part of the command (`parse`).
    let c = cmd.trim_matches([' ', '\t', '\n']);
    let c = c.strip_prefix("LC_ALL=C ").unwrap_or(c).trim_start_matches([' ', '\t']);
    // Exact, compiled-in one-shot identification only; no arbitrary Python exemption.
    if c == crate::discovery::read_command() { return Verdict::of(Kind::Read); }
    if depth > 8 {
        return Verdict::destructive(UNPARSEABLE.into());
    }
    if let Some(name) = fork_bomb(c) {
        return Verdict::destructive(format!("A fork bomb ({name}): it starts copies of itself until the node stops responding."));
    }
    let segments = match parse(c, 0) {
        Ok(s) => s,
        Err(Unparsed::Quoting) => return Verdict::destructive(UNPARSEABLE.into()),
        // A power verb hidden in syntax SparkBox can't follow is destructive, never power-one.
        Err(Unparsed::Syntax(what)) => return Verdict::destructive(syntax_reason(what)),
    };
    let mut out = Verdict::of(Kind::Read);
    for (i, seg) in segments.iter().enumerate() {
        let words: Vec<&str> = seg.words.iter().map(String::as_str).collect();
        // The command word is a substitution, a variable or a glob (`$(echo uptime)`, `$X`,
        // `c?t`): the node decides what runs, so it's destructive, whatever runs around it.
        if command_word(seg, &words).is_some_and(|(w, glob)| hides_command(w, glob)) {
            if out.kind != Kind::Destructive {
                out = Verdict { power: out.power, root: out.root, ..Verdict::destructive(HIDDEN.into()) };
            }
            continue;
        }
        // So does a wrapper with an option SparkBox can't read (`nice --foo X …`): X may be the
        // option's value or the command.
        if let Core::Unread(name) = core(&words).0 {
            if out.kind != Kind::Destructive {
                let what = format!("{} with an option it doesn't know", name.rsplit('/').next().unwrap_or(name));
                out = Verdict { power: out.power, root: out.root, ..Verdict::destructive(syntax_reason(&what)) };
            }
            continue;
        }
        // Docker is root (P1.42): a `docker run`/`create`/`exec` with host access runs with the
        // node's root, and its own command (host bind paths mapped back) is classified too.
        if let Some(dc) = docker_invocation(&words) {
            if dc.root {
                out.root = true;
                if out.kind == Kind::Read {
                    out.kind = Kind::Change;
                }
                if !dc.cmd.is_empty() {
                    let mapped = map_host_paths(&dc.cmd, &dc.binds);
                    let joined = mapped.iter().map(|w| squote(w)).collect::<Vec<_>>().join(" ");
                    let v = verdict_at(&joined, depth + 1);
                    let power = out.power.or(v.power);
                    if v.cuts_access {
                        return Verdict { root: true, power, ..v };
                    }
                    if v.kind == Kind::Destructive && out.kind != Kind::Destructive {
                        out = Verdict { root: true, power, ..v };
                        continue;
                    }
                    out.power = power;
                }
                continue;
            }
        }
        // A wrapper that runs a script (`sh -c`, `su -c`, `watch`, `timeout … sh -c '…'`), or
        // `at`/`batch` scheduling a job: fold the inner verdict, so root, power and severity
        // carry through the wrapping (P1.42). A job SparkBox can't read fails closed. The outer
        // segment still decides Read vs Change through `segment_read_only`, so a hijacking
        // assignment on it (BASH_ENV=…, LD_PRELOAD=…) is still a change.
        let prev = i.checked_sub(1).map(|j| &segments[j]);
        // `xargs CMD ARGS` appends the words it's fed as more operands. When SparkBox can read
        // what's fed (echo/printf, or a here-string), it runs `CMD ARGS <fed words>`, so classify
        // that: power and severity carry through (`echo reboot | xargs systemctl` is a restart).
        // When it can't read the input, `severe` and `segment_read_only` judge `CMD ARGS` alone.
        if let Some(job) = xargs_job(seg, prev) {
            let v = verdict_at(&job, depth + 1);
            let power = out.power.or(v.power);
            let root = out.root || v.root;
            if v.cuts_access {
                return Verdict { power, root, ..v };
            }
            if v.kind == Kind::Destructive && out.kind != Kind::Destructive {
                out = Verdict { power, root, ..v };
            } else if out.kind == Kind::Read && v.kind == Kind::Change {
                out.kind = Kind::Change;
            }
            out.power = power;
            out.root = root;
            continue;
        }
        let cored = core(&words).0;
        // A shell that sources an rcfile (-i, --rcfile, --init-file) still runs its `-c` script
        // after it: that script's power and severity carry through. The segment stays a change
        // (`words_read_only` refuses such a shell), whatever the script is.
        let rc_script = match &cored {
            Core::Words(w) => rcfile_script(w),
            Core::Script(_) | Core::Unread(_) => None,
        };
        let inner = match (at_job(seg, prev), cored) {
            (Some(AtJob::Known(job)), _) => Some(verdict_at(&job, depth + 1)),
            (Some(AtJob::Unknown(name)), _) => Some(Verdict::destructive(format!("Schedules a command ({name}) SparkBox can't check."))),
            (None, Core::Script(script)) => Some(match env_fed(seg, prev) {
                Some(fed) => verdict_at(&format!("{fed}\n{script}"), depth + 1),
                None => verdict_at(&script, depth + 1),
            }),
            // A shell reading its script from stdin (`echo reboot | sh`, `bash <<< reboot`): the
            // text it's fed, before the `-c` script of a shell whose rcfile is stdin. Text
            // SparkBox can't read leaves it a change.
            _ => match (stdin_script(seg, prev), rc_script) {
                (Some(Some(text)), Some(script)) => Some(verdict_at(&format!("{text}\n{script}"), depth + 1)),
                (_, Some(script)) => Some(verdict_at(&script, depth + 1)),
                (Some(Some(text)), None) => Some(verdict_at(&text, depth + 1)),
                _ => None,
            },
        };
        if let Some(v) = inner {
            let power = out.power.or(v.power);
            out.root |= v.root;
            if v.cuts_access {
                return Verdict { power, root: out.root, ..v };
            }
            if v.kind == Kind::Destructive && out.kind != Kind::Destructive {
                out = Verdict { power, root: out.root, ..v };
            } else if out.kind != Kind::Destructive && !segment_read_only(seg) {
                out.kind = Kind::Change;
            }
            out.power = power;
            continue;
        }
        if let Some(v) = severe(seg, depth) {
            // Cutting access outranks any other destructive reason; power is noted either way.
            let power = out.power.or(v.power);
            let root = out.root || v.root;
            if v.cuts_access {
                return Verdict { power, root, ..v };
            }
            if out.kind != Kind::Destructive {
                out = v;
            }
            out.power = power;
            out.root = root;
            continue;
        }
        if out.kind != Kind::Destructive && !segment_read_only(seg) {
            out.kind = Kind::Change;
        }
    }
    out
}

/// True when every part of `cmd` only reads.
pub fn read_only(cmd: &str) -> bool {
    if cmd == crate::discovery::read_command() || cmd == super::protected_updates::BEFORE { return true; }
    match split(cmd, 0) {
        Some(segments) => segments.iter().all(segment_read_only),
        None => false,
    }
}

/// The power `cmd` takes when it does *nothing but* that one power action: every part is either a
/// read or a power verb (reboot/poweroff/halt and the rest, however a shell, sudo, `su -c`,
/// `env -S`, a fed stdin or an rcfile wraps it), and at least one is. None when the command also
/// changes the node some other way (`touch x; reboot`), or isn't a power verb at all. Fleet gives
/// such a pure power command its own class (`power-one`); a compound one stays a Fleet command.
pub fn power_alone(cmd: &str) -> Option<Power> {
    power_alone_at(cmd, 0)
}

fn power_alone_at(cmd: &str, depth: usize) -> Option<Power> {
    if depth > 8 {
        return None;
    }
    let c = cmd.trim_matches([' ', '\t', '\n']);
    let c = c.strip_prefix("LC_ALL=C ").unwrap_or(c).trim_start_matches([' ', '\t']);
    if fork_bomb(c).is_some() {
        return None;
    }
    let segments = split(c, 0)?;
    let mut power = None;
    for (i, seg) in segments.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| &segments[j]);
        // None: a read, which rides under power-one harmlessly.
        if let Some(p) = seg_power_only(seg, prev, depth)? {
            power = power.or(Some(p));
        }
    }
    power
}

/// A segment's contribution to `power_alone`: Some(Some(power)) when it is exactly a power action,
/// Some(None) when it only reads, None when it changes the node any other way (so the whole
/// command isn't power-alone).
fn seg_power_only(seg: &Segment, prev: Option<&Segment>, depth: usize) -> Option<Option<Power>> {
    if segment_read_only(seg) {
        return Some(None);
    }
    // A redirection or here-string on the segment itself does more than the power verb.
    if seg.writes || !seg.targets.is_empty() || seg.here_string.is_some() {
        return None;
    }
    let words: Vec<&str> = seg.words.iter().map(String::as_str).collect();
    // A scheduled job, a container run or an xargs that can't be read isn't a bare power verb.
    if at_job(seg, prev).is_some() || docker_invocation(&words).is_some() {
        return None;
    }
    if let Some(job) = xargs_job(seg, prev) {
        return power_alone_at(&job, depth + 1).map(Some);
    }
    let cored = core(&words).0;
    let rc_script = match &cored {
        Core::Words(w) => rcfile_script(w),
        Core::Script(_) | Core::Unread(_) => None,
    };
    match cored {
        // A wrapper whose options SparkBox can't read isn't a bare power verb.
        Core::Unread(_) => None,
        // A wrapper (sudo, `sh -c`, `su -c`, `env -S`, eval, exec…): its script, with what a fed
        // BASH_ENV/ENV sources first, judged on its own.
        Core::Script(script) => {
            let text = match env_fed(seg, prev) {
                Some(fed) => format!("{fed}\n{script}"),
                None => script,
            };
            power_alone_at(&text, depth + 1).map(Some)
        }
        Core::Words(w) => {
            // A shell fed its script on stdin, or one sourcing an rcfile then running `-c`.
            match (stdin_script(seg, prev), rc_script) {
                (Some(Some(text)), Some(s)) => return power_alone_at(&format!("{text}\n{s}"), depth + 1).map(Some),
                (_, Some(s)) => return power_alone_at(&s, depth + 1).map(Some),
                (Some(Some(text)), None) => return power_alone_at(&text, depth + 1).map(Some),
                (Some(None), None) => return None, // fed text SparkBox can't read
                (None, None) => {}
            }
            let (&first, raw) = w.split_first()?;
            // `/tmp/x/reboot` is some file called reboot, not the power verb.
            if hides_command(first, looks_globbed(first)) || !system_program(first) {
                return None;
            }
            let name = first.rsplit('/').next().unwrap_or(first);
            let normal = getopt(name, raw);
            let args: Vec<&str> = normal.iter().map(String::as_str).collect();
            // `find … -exec CMD …`: its only change is CMD, when it also deletes nothing.
            if name == "find" {
                if args.iter().any(|a| matches!(*a, "-delete")) {
                    return None;
                }
                let cmd = find_exec(&args)?;
                let joined = cmd.iter().map(|w| squote(w)).collect::<Vec<_>>().join(" ");
                return power_alone_at(&joined, depth + 1).map(Some);
            }
            // A simple command: a power verb alone, or some other change.
            match powers_down(name, &args) {
                Some(p) if destructive(name, &args).is_some() && cuts_access(name, &args, seg).is_none() => Some(Some(p)),
                _ => None,
            }
        }
    }
}

/// Any part of the command runs through sudo, doas, pkexec, run0 or `su -c`.
pub fn escalates(cmd: &str) -> bool {
    escalates_at(cmd, 0)
}

fn escalates_at(cmd: &str, depth: usize) -> bool {
    if depth > 8 {
        return true;
    }
    let Some(segments) = split(cmd.trim(), 0) else { return cmd.contains("sudo") || cmd.contains("doas") || cmd.contains("pkexec") };
    segments.iter().enumerate().any(|(i, seg)| {
        let words: Vec<&str> = seg.words.iter().map(String::as_str).collect();
        let (core, root) = core(&words);
        // An at job runs later with the same rights; one that sudos escalates too.
        let prev = i.checked_sub(1).map(|j| &segments[j]);
        let job = matches!(at_job(seg, prev), Some(AtJob::Known(j)) if escalates_at(&j, depth + 1));
        // So does a script a shell is fed on stdin (`echo 'sudo reboot' | sh`).
        let fed = matches!(stdin_script(seg, prev), Some(Some(t)) if escalates_at(&t, depth + 1)) || env_fed(seg, prev).is_some_and(|t| escalates_at(&t, depth + 1));
        // Docker-as-root gains root without sudo, so Fleet's as-root path (the typed count)
        // still applies to it (P1.42).
        // A shell that sources an rcfile runs its `-c` script too.
        let rc = matches!(&core, Core::Words(w) if rcfile_script(w).is_some_and(|s| escalates_at(&s, depth + 1)));
        root || job || fed || rc || docker_invocation(&words).is_some_and(|d| d.root) || matches!(core, Core::Script(s) if escalates_at(&s, depth + 1))
    })
}

#[derive(Debug, Default, Clone, PartialEq)]
struct Segment {
    words: Vec<String>,
    /// For each of `words`: it had an unquoted expansion (`$X`, `${X}`) or glob (`*`, `?`,
    /// `[…]`), so the shell makes it into something SparkBox never saw.
    globs: Vec<bool>,
    /// For each of `words`: what the shell still has to build of it (`Built`).
    built: Vec<Built>,
    /// Output goes into a file (`>`/`>>` to anything but /dev/null, bash's `>&file`), or the
    /// segment changes something its words don't show (a here-string, `${x:=…}`).
    writes: bool,
    /// Where output is redirected to.
    targets: Vec<String>,
    /// Its stdin is the output of the command before it (`a | b`).
    piped: bool,
    /// The text of a here-string (`<<< text`), given to it on stdin.
    here_string: Option<String>,
}

/// What the shell builds of a word once SparkBox has read it, which no rule that reads the word can
/// see: a `$(…)`, a backtick, a variable or a glob. $HOME and $USER don't count: nothing the read
/// tier allows can set them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Built {
    /// Nothing: its text is what the program gets.
    #[default]
    Fixed,
    /// Part or all of it, in double quotes: still one word, with any text.
    OneWord,
    /// Unquoted, or a glob: any number of words, each with any text.
    Split,
}

/// `parse`, for the callers that only need to know whether it could.
fn split(cmd: &str, depth: usize) -> Option<Vec<Segment>> {
    parse(cmd, depth).ok()
}

/// One word as `parse` builds it: its text (quotes removed), whether it's started, whether it
/// had an unquoted expansion or glob, and the braces open in it (true: a `${`).
#[derive(Default)]
struct Word {
    text: String,
    started: bool,
    glob: bool,
    built: Built,
    bracket: bool,
    braces: Vec<(bool, bool)>,
}

impl Word {
    /// An expansion in double quotes.
    fn quoted_expansion(&mut self) {
        if self.built == Built::Fixed {
            self.built = Built::OneWord;
        }
    }
}

/// The variable after the `$` at `chars[at - 1]` is $HOME, $USER, ${HOME} or ${USER}: the login's,
/// which no read can set (`hijacks`, `sets_script_variables`). Any other `${…}` form can make any
/// text out of them (`${HOME:+-delete}`).
fn login_variable(chars: &[char], at: usize) -> bool {
    let name: String = chars[at.min(chars.len())..].iter().skip_while(|c| **c == '{').take_while(|c| c.is_ascii_alphanumeric() || **c == '_').collect();
    let closed = match chars.get(at) {
        Some('{') => chars.get(at + 1 + name.len()) == Some(&'}'),
        _ => true,
    };
    closed && matches!(name.as_str(), "HOME" | "USER")
}

/// Splits a command line into simple commands: on `;`, `&&`, `||`, `|`, `&`, newlines and
/// parentheses, with each `$(…)` and backtick substitution as commands of its own. Quotes are
/// honoured and removed. Fails when the quoting doesn't balance, and on syntax neither this nor
/// the fence's shapes follow (T2 blocker 2), outside single quotes: arithmetic (`$((`, `$[`),
/// `$'…'` and `$"…"`, bash's `${x[…]}`, `${x:n}`, `${!x}` and `${x@op}` (`brace_expansion`),
/// process substitution, `|&`, `;&`, `;;&`, a here-document, coproc, function and select,
/// brace expansion (`{a,b}`, `{1..3}`), an ASCII control character other than tab and newline,
/// and a non-ASCII space (dash and bash don't split words there; `ls<NBSP>-la` is one word).
fn parse(cmd: &str, depth: usize) -> Result<Vec<Segment>, Unparsed> {
    if depth > 8 {
        return Err(Unparsed::Quoting);
    }
    if cmd.chars().any(|c| c.is_ascii_control() && !matches!(c, '\t' | '\n')) {
        return Err(Unparsed::Syntax("a control character"));
    }
    let chars: Vec<char> = cmd.chars().collect();
    let mut out = Vec::new();
    let mut subs: Vec<String> = Vec::new();
    let mut seg = Segment::default();
    let mut word = Word::default();
    // After `>`: the next word is where output goes.
    let mut redirect_target = false;
    // After `<`: the next word is an input file, not an argument; after `<<<`, the text itself.
    let mut input_target = false;
    let mut here_target = false;

    fn end_word(word: &mut Word, seg: &mut Segment, redirect: &mut bool, input: &mut bool, here: &mut bool) {
        let w = std::mem::take(word);
        if !w.started {
            return;
        }
        if *redirect {
            *redirect = false;
            if !(w.text == "/dev/null" || w.text.starts_with('&')) {
                seg.writes = true;
                seg.targets.push(w.text);
            }
        } else if *input {
            *input = false;
        } else if *here {
            *here = false;
            seg.here_string = Some(w.text);
        } else {
            seg.words.push(w.text);
            seg.globs.push(w.glob);
            seg.built.push(w.built);
        }
    }

    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        match ch {
            '\\' => {
                if let Some(&next) = chars.get(i + 1) {
                    if next != '\n' {
                        word.text.push(next);
                        word.started = true;
                    }
                }
                i += 2;
                continue;
            }
            '\'' => {
                let end = chars[i + 1..].iter().position(|&c| c == '\'').ok_or(Unparsed::Quoting)? + i + 1;
                word.text.extend(&chars[i + 1..end]);
                word.started = true;
                i = end + 1;
                continue;
            }
            '"' => {
                let mut j = i + 1;
                loop {
                    let c = *chars.get(j).ok_or(Unparsed::Quoting)?;
                    match c {
                        '"' => break,
                        '\\' => {
                            if let Some(&n) = chars.get(j + 1) {
                                word.text.push(n);
                            }
                            j += 2;
                        }
                        '`' => {
                            let end = chars[j + 1..].iter().position(|&c| c == '`').ok_or(Unparsed::Quoting)? + j + 1;
                            subs.push(chars[j + 1..end].iter().collect());
                            word.text.push_str(SUB);
                            word.quoted_expansion();
                            j = end + 1;
                        }
                        // Arithmetic runs inside double quotes too.
                        '$' if chars.get(j + 1) == Some(&'(') && chars.get(j + 2) == Some(&'(') => return Err(Unparsed::Syntax("$((…)) arithmetic")),
                        '$' if chars.get(j + 1) == Some(&'[') => return Err(Unparsed::Syntax("$[…] arithmetic")),
                        '$' if chars.get(j + 1) == Some(&'(') => {
                            let end = matching_paren(&chars, j + 1).ok_or(Unparsed::Quoting)?;
                            subs.push(chars[j + 2..end].iter().collect());
                            word.text.push_str(SUB);
                            word.quoted_expansion();
                            j = end + 1;
                        }
                        // So do bash's `${x[…]}`, `${x:n}`, `${!x}` and `${x@P}`, and an assignment.
                        '$' if chars.get(j + 1) == Some(&'{') => {
                            let (bash, assigns) = brace_expansion(&chars, j + 2);
                            if bash {
                                return Err(Unparsed::Syntax("a bash-only ${…} expansion"));
                            }
                            seg.writes |= assigns;
                            if !login_variable(&chars, j + 1) {
                                word.quoted_expansion();
                            }
                            word.text.push('$');
                            j += 1;
                        }
                        '$' if chars.get(j + 1).is_some_and(|c| c.is_ascii_alphanumeric() || "_@*#?$!-".contains(*c)) => {
                            if !login_variable(&chars, j + 1) {
                                word.quoted_expansion();
                            }
                            word.text.push('$');
                            j += 1;
                        }
                        _ => {
                            word.text.push(c);
                            j += 1;
                        }
                    }
                }
                word.started = true;
                i = j + 1;
                continue;
            }
            '`' => {
                let end = chars[i + 1..].iter().position(|&c| c == '`').ok_or(Unparsed::Quoting)? + i + 1;
                subs.push(chars[i + 1..end].iter().collect());
                word.text.push_str(SUB);
                word.started = true;
                word.built = Built::Split;
                i = end + 1;
                continue;
            }
            '$' => match chars.get(i + 1).copied() {
                Some('(') if chars.get(i + 2) == Some(&'(') => return Err(Unparsed::Syntax("$((…)) arithmetic")),
                Some('(') => {
                    let end = matching_paren(&chars, i + 1).ok_or(Unparsed::Quoting)?;
                    subs.push(chars[i + 2..end].iter().collect());
                    word.text.push_str(SUB);
                    word.started = true;
                    word.built = Built::Split;
                    i = end + 1;
                    continue;
                }
                Some('[') => return Err(Unparsed::Syntax("$[…] arithmetic")),
                Some('\'') => return Err(Unparsed::Syntax("$'…' quoting")),
                Some('"') => return Err(Unparsed::Syntax("$\"…\" quoting")),
                // `${x[…]}`, `${x:n}`, `${!x}`, `${x@P}`: bash evaluates or expands a value again,
                // so a `$(…)` in it runs (`brace_expansion`).
                Some('{') if brace_expansion(&chars, i + 2).0 => return Err(Unparsed::Syntax("a bash-only ${…} expansion")),
                // $X, ${X}, $1, $@…: an expansion the shell splits and globs.
                Some(c) if c.is_ascii_alphanumeric() || "_{@*#?$!-".contains(c) => {
                    word.glob = true;
                    if !login_variable(&chars, i + 1) {
                        word.built = Built::Split;
                    }
                    word.text.push('$');
                    word.started = true;
                    if c == '{' {
                        // `${x=…}` and `${x:=…}` set x as they expand: not a read.
                        seg.writes |= brace_expansion(&chars, i + 2).1;
                        word.braces.push((true, false));
                        word.text.push('{');
                        i += 1;
                    }
                    i += 1;
                    continue;
                }
                _ => {
                    word.text.push('$');
                    word.started = true;
                    i += 1;
                    continue;
                }
            },
            '>' => {
                if chars.get(i + 1) == Some(&'(') {
                    return Err(Unparsed::Syntax(">(…) process substitution"));
                }
                // "2>" and "1>>": the digit names a stream, it isn't an argument. Only one unquoted
                // digit: dash reads `12>f` and `'1'>f` as an argument and `>f`.
                let fd = word.text.len() == 1 && chars[i - 1].is_ascii_digit() && (i < 2 || matches!(chars[i - 2], ' ' | '\t' | '\n' | ';' | '|' | '&' | '('));
                if word.started && fd {
                    word = Word::default();
                }
                end_word(&mut word, &mut seg, &mut redirect_target, &mut input_target, &mut here_target);
                let mut j = i + 1;
                if matches!(chars.get(j), Some('>') | Some('|')) {
                    j += 1;
                }
                if chars.get(j) == Some(&'&') {
                    // 2>&1, >&-: duplicates or closes a stream; nothing is written. Any other word
                    // after `>&` is a file bash writes (`>&file` is `>file 2>&1`; dash refuses it).
                    let mut k = j + 1;
                    while matches!(chars.get(k), Some(' ' | '\t')) {
                        k += 1;
                    }
                    let digits = chars[k.min(chars.len())..].iter().take_while(|c| c.is_ascii_digit()).count();
                    let end = if digits == 0 && chars.get(k) == Some(&'-') { k + 1 } else { k + digits };
                    if end > k && chars.get(end).is_none_or(|c| matches!(c, ' ' | '\t' | '\n' | ';' | '|' | '&' | ')' | '<' | '>')) {
                        i = end;
                    } else {
                        redirect_target = true;
                        i = j + 1;
                    }
                    continue;
                }
                redirect_target = true;
                i = j;
                continue;
            }
            '<' => {
                if chars.get(i + 1) == Some(&'(') {
                    return Err(Unparsed::Syntax("<(…) process substitution"));
                }
                end_word(&mut word, &mut seg, &mut redirect_target, &mut input_target, &mut here_target);
                if chars.get(i + 1) == Some(&'<') && chars.get(i + 2) == Some(&'<') {
                    // A here-string: its text is the command's stdin (`at now <<< reboot`).
                    seg.writes = true;
                    here_target = true;
                    i += 3;
                } else if chars.get(i + 1) == Some(&'<') {
                    // A here-document: its body is text, not commands, and an odd quote in it
                    // would carry this parser past the delimiter. Not followed.
                    return Err(Unparsed::Syntax("a here-document"));
                } else {
                    input_target = true;
                    i += 1;
                }
                continue;
            }
            '&' if chars.get(i + 1) == Some(&'>') => {
                end_word(&mut word, &mut seg, &mut redirect_target, &mut input_target, &mut here_target);
                redirect_target = true;
                i += 2;
                if chars.get(i) == Some(&'>') {
                    i += 1;
                }
                continue;
            }
            '|' if chars.get(i + 1) == Some(&'&') => return Err(Unparsed::Syntax("|& pipe")),
            ';' if chars.get(i + 1) == Some(&'&') || (chars.get(i + 1) == Some(&';') && chars.get(i + 2) == Some(&'&')) => return Err(Unparsed::Syntax("a case fall-through")),
            ';' | '&' | '|' | '\n' | '(' | ')' => {
                end_word(&mut word, &mut seg, &mut redirect_target, &mut input_target, &mut here_target);
                if redirect_target || input_target || here_target {
                    return Err(Unparsed::Quoting); // "cmd >" with no file
                }
                out.push(std::mem::take(&mut seg));
                // A single `|` pipes into the next command; `||` is "or else".
                if ch == '|' && chars.get(i + 1) == Some(&'|') {
                    i += 2;
                    continue;
                }
                if ch == '|' {
                    seg.piped = true;
                }
                i += 1;
                continue;
            }
            // dash and bash split words at space, tab and newline only: a non-ASCII space is part
            // of a word, so `ls<NBSP>-la` isn't `ls -la`.
            c if !c.is_ascii() && (c.is_whitespace() || c == '\u{feff}') => return Err(Unparsed::Syntax("a non-ASCII space")),
            c if c.is_whitespace() => {
                end_word(&mut word, &mut seg, &mut redirect_target, &mut input_target, &mut here_target);
                i += 1;
                continue;
            }
            '#' if !word.started => {
                // A comment runs to the end of the line.
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            c => {
                // Globs and brace expansion, outside quotes. A `[` globs only with its `]`; a `{`
                // expands only with a `,` or `..` in it (`find -exec … {}` doesn't).
                match c {
                    '*' | '?' => {
                        word.glob = true;
                        word.built = Built::Split;
                    }
                    '[' => word.bracket = true,
                    ']' if word.bracket => {
                        word.glob = true;
                        word.built = Built::Split;
                    }
                    '{' => word.braces.push((false, false)),
                    ',' => {
                        if let Some(b) = word.braces.last_mut().filter(|b| !b.0) {
                            b.1 = true;
                        }
                    }
                    '.' if chars.get(i + 1) == Some(&'.') => {
                        if let Some(b) = word.braces.last_mut().filter(|b| !b.0) {
                            b.1 = true;
                        }
                    }
                    '}' => {
                        let closed = word.braces.pop();
                        if closed == Some((false, true)) {
                            return Err(Unparsed::Syntax("{…} brace expansion"));
                        }
                    }
                    _ => {}
                }
                word.text.push(c);
                word.started = true;
                i += 1;
            }
        }
    }
    end_word(&mut word, &mut seg, &mut redirect_target, &mut input_target, &mut here_target);
    if redirect_target || here_target {
        return Err(Unparsed::Quoting);
    }
    out.push(seg);
    for s in subs {
        out.extend(parse(&s, depth + 1)?);
    }
    // Words that open a block SparkBox doesn't follow, where a command would start.
    for s in &out {
        if let Some(w) = s.words.iter().map(String::as_str).find(|w| !(KEYWORDS.contains(w) || CLOSERS.contains(w) || is_assignment(w))) {
            if matches!(w, "coproc" | "function" | "select") {
                return Err(Unparsed::Syntax("coproc, function or select"));
            }
        }
    }
    Ok(out.into_iter().filter(|s| !s.words.is_empty() || s.writes).collect())
}

/// A `${…}` whose text starts at `from` (up to its first `}`), as bash reads it: (bash only,
/// assigns). Bash only: `${!x}` (x's value names the variable), `${x[…]}` (a subscript),
/// `${x:offset}` and `${@:n}` (a substring) evaluate a value as arithmetic or expand it again,
/// and `${x@P}` expands it as a prompt, so a `$(…)` in the value runs (`y='a[$(reboot)]'; echo
/// ${x[y]}`); dash refuses each ("Bad substitution"). Assigns: `${x=…}`, `${x:=…}`.
fn brace_expansion(chars: &[char], from: usize) -> (bool, bool) {
    let Some(n) = chars.get(from..).and_then(|rest| rest.iter().position(|&c| c == '}')) else { return (false, false) };
    let inner = &chars[from..from + n];
    // Past a leading `#` (the length) and the name: a word, digits, or one special parameter.
    let mut k = usize::from(inner.len() > 1 && inner[0] == '#');
    let name = k;
    while inner.get(k).is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_') {
        k += 1;
    }
    if k == name && inner.get(k).is_some_and(|c| "@*#?$!-".contains(*c)) {
        k += 1;
    }
    let (op, next) = (inner.get(k).copied(), inner.get(k + 1).copied());
    let bash = (inner.len() > 1 && inner[0] == '!') || inner.contains(&'[') || op == Some('@') || (op == Some(':') && !matches!(next, Some('-' | '=' | '?' | '+')));
    (bash, op == Some('=') || (op == Some(':') && next == Some('=')))
}

/// The commands of each `$(…)` and backtick left in a word once its quoting is gone. The SUB
/// placeholder of one the shell runs anyway is skipped: its commands are segments of their own.
fn quoted_substitutions(word: &str) -> Vec<String> {
    let chars: Vec<char> = word.replace(SUB, "").chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && chars.get(i + 1) == Some(&'(') {
            if let Some(end) = matching_paren(&chars, i + 1) {
                out.push(chars[i + 2..end].iter().collect());
                i = end + 1;
                continue;
            }
        } else if chars[i] == '`' {
            if let Some(n) = chars[i + 1..].iter().position(|&c| c == '`') {
                out.push(chars[i + 1..i + 1 + n].iter().collect());
                i += n + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Index of the `)` closing the `(` at `open`, skipping quoted text.
fn matching_paren(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0;
    let mut quote: Option<char> = None;
    let mut i = open;
    while i < chars.len() {
        let c = chars[i];
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => i += 1,
            (Some(_), _) => {}
            (None, '\'') | (None, '"') => quote = Some(c),
            (None, '\\') => i += 1,
            (None, '(') => depth += 1,
            (None, ')') => {
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

/// Commands that only read, whatever their arguments, unless a rule in `words_read_only` says
/// otherwise. A tool goes on this list with a row in `read_tier_every_tool_is_accounted_for`.
pub(crate) const READ_ANY: &[&str] = &[
    "cat", "head", "tail", "less", "more", "ls", "df", "du", "free", "uptime", "uname", "id", "whoami", "groups", "nproc",
    "lscpu", "lsblk", "lspci", "lsusb", "lsmod", "lsof", "lshw", "lsns", "ps", "pgrep", "pidof", "top", "w", "who", "last",
    "ss", "netstat", "echo", "printf", "true", "false", "pwd", "stat", "file", "readlink", "realpath", "basename",
    "dirname", "which", "whereis", "type", "getent", "wc", "cut", "tr", "column", "grep", "egrep", "fgrep",
    "zgrep", "rg", "jq", "sha256sum", "sha1sum", "md5sum", "cksum", "xxd", "hexdump", "od", "strings", "diff", "cmp",
    "comm", "locale", "lsb_release", "nvcc", "sensors", "vmstat", "iostat", "mpstat", "pidstat", "sar", "ibstat",
    "ibstatus", "ibv_devinfo", "ibv_devices", "dig", "nslookup", "host", "ping", "traceroute",
    "tracepath", "findmnt", "blkid", "test", "[", "[[", "]]", "]", "sleep", "wait", "read", "seq", "cd", "exit",
    "apt-cache", "dpkg-query", "getconf", "ulimit", "dmidecode", "nl", "fold", "paste", "join", "expand", "tac", "rev",
    "zcat", "bzcat", "xzcat", "numfmt", "iconv", "base64", "lslogins", "lsipc", "tree", "printenv", "arch", "nvidia-ctk",
    "apt-config", "modinfo",
];

/// READ_ANY's tools with a rule in `words_read_only`: a read only for what their words say, so a word
/// the shell builds gets past it (`built_word_counts`). Each has a `Rule` row in `READ_TIER`.
pub(crate) const OPTIONS_MATTER: &[&str] = &[
    "less", "more", "lspci", "lshw", "ss", "printf", "file", "getent", "rg", "xxd", "nvcc", "sensors", "pidstat", "sar", "blkid", "test", "[", "[[", "read", "apt-cache", "ulimit", "dmidecode", "iconv", "tree",
    "nvidia-ctk", "apt-config",
];

/// Shell words that open or close a block; what follows them (if anything) is the command.
const KEYWORDS: &[&str] = &["do", "then", "else", "elif", "if", "while", "until", "!", "{", "time"];
const CLOSERS: &[&str] = &["done", "fi", "esac", "}", "in", ";;"];

/// Read-tier builtins bash re-evaluates its operands for (an array subscript or an arithmetic
/// context runs any `$(…)` inside, however it's quoted), with `printf -v`: a `[` or a command
/// substitution in one of their operands isn't a read (`reevaluates`), and its commands count
/// (`severe`).
const REEVALUATES: &[&str] = &["test", "[", "[[", "read", "ulimit"];

/// Builtins that change what later commands run or how the shell reads them (an alias, a hashed
/// path, a trap, an option, a declaration), or run text of their own: never reads. `[[` too: its
/// -eq, -lt… and -v evaluate a variable's *value* as an expression (`read y; [[ $y -eq 0 ]]`, `[[
/// y -eq 0 ]]` run a `$(…)` in y), which no word shows.
const NEVER_READ: &[&str] = &["[[", "alias", "unalias", "hash", "trap", "eval", ".", "source", "set", "shopt", "enable", "builtin", "declare", "typeset", "let"];

/// The folders whose programs these rules know by name: a command word that is a path anywhere
/// else names some other file (`/tmp/x/cat` isn't cat).
const SYSTEM_BIN: &[&str] = &["/bin", "/sbin", "/usr/bin", "/usr/sbin", "/usr/local/bin", "/usr/local/sbin"];

/// The word a segment runs, once `core` has peeled keywords, assignments and wrappers, and
/// whether `parse` saw an unquoted glob or expansion in it. None when it runs a script of its
/// own (`sh -c`…, checked on its own) or nothing.
fn command_word<'a>(seg: &'a Segment, words: &[&'a str]) -> Option<(&'a str, bool)> {
    let Core::Words(w) = core(words).0 else { return None };
    let first = *w.first()?;
    // `core` hands back the segment's own words, so its flag is found by identity.
    let glob = seg.words.iter().position(|s| std::ptr::eq(s.as_str(), first)).and_then(|i| seg.globs.get(i).copied());
    Some((first, glob.unwrap_or_else(|| looks_globbed(first))))
}

/// A command word whose program the node decides after SparkBox has looked: a `$(…)` or
/// backtick substitution (its SUB placeholder), a `$` in any other form (a variable, `${…}`, `$1`),
/// a backtick, a brace, a leading `~`, or (`glob`) an unquoted glob.
fn hides_command(word: &str, glob: bool) -> bool {
    word.contains(SUB) || word.replace(SUB, "").contains('$') || word.contains('`') || glob || word.contains(['{', '}']) || word.starts_with('~')
}

/// A glob in a word whose quoting is gone: `*`, `?`, or a `[` closed later by `]`.
fn looks_globbed(word: &str) -> bool {
    word.contains(['*', '?']) || word.find('[').is_some_and(|i| word[i + 1..].contains(']'))
}

/// A program named by its name, or by its path in one of the system's own folders.
fn system_program(word: &str) -> bool {
    match word.rsplit_once('/') {
        None => true,
        Some((dir, name)) => !name.is_empty() && SYSTEM_BIN.contains(&dir),
    }
}

/// `name` re-evaluates its operands `args`: the REEVALUATES builtins, and printf with `-v`.
fn reevaluates(name: &str, args: &[&str]) -> bool {
    REEVALUATES.contains(&name) || (name == "printf" && args.iter().take_while(|a| a.starts_with('-') && **a != "--").any(|a| a.starts_with("-v")))
}

/// The variables `read NAME…`, `printf -v NAME` and `for NAME in` set are lowercase names: a
/// script's own, never the shell's (PATH, IFS, ENV and the rest are uppercase).
fn sets_script_variables(name: &str, args: &[&str]) -> bool {
    let plain = |v: &str| !v.is_empty() && v.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    match name {
        "for" => args.first().is_none_or(|v| plain(v)),
        "read" => args.iter().filter(|a| !a.starts_with('-')).all(|a| plain(a)),
        "printf" => match args.iter().take_while(|a| a.starts_with('-')).position(|a| a.starts_with("-v")) {
            Some(i) if args[i] == "-v" => args.get(i + 1).is_some_and(|v| plain(v)),
            Some(i) => plain(&args[i][2..]),
            None => true,
        },
        _ => true,
    }
}

fn segment_read_only(seg: &Segment) -> bool {
    !seg.writes && words_read_only(&seg.words) && !built_word_counts(seg)
}

/// A tool's rule reads its words, and a word the shell builds (`Built`) isn't there to read:
/// `find . $(echo -delete)` deletes, `sort "$x" f` can be `sort -o f`, `curl -w "a$x"` can write a
/// file. So a segment with one is a read only with a tool that has no rule (READ_ANY less
/// `OPTIONS_MATTER`), a shell word whose words are its own (`for`, `case`, `export`…) or where the
/// tool's rule leaves the word no say (`built_word_harmless`). A `-c` script with one in it, or a
/// wrapper's own option, is text no rule read either.
fn built_word_counts(seg: &Segment) -> bool {
    if seg.built.iter().all(|b| *b == Built::Fixed) {
        return false;
    }
    let words: Vec<&str> = seg.words.iter().map(String::as_str).collect();
    let Core::Words(peeled) = core(&words).0 else { return true };
    let Some(&tool) = peeled.first() else { return false };
    let Some(at) = seg.words.iter().position(|w| std::ptr::eq(w.as_str(), tool)) else { return true };
    let start = words.iter().position(|w| !(CLOSERS.contains(w) || KEYWORDS.contains(w) || is_assignment(w))).unwrap_or(at);
    let fixed = |built: &[Built]| built.iter().all(|b| *b == Built::Fixed);
    if !fixed(&seg.built[start..at]) {
        return true;
    }
    let name = tool.rsplit('/').next().unwrap_or(tool);
    let (args, built) = (&words[at + 1..], &seg.built[at + 1..]);
    let free = (READ_ANY.contains(&name) && !OPTIONS_MATTER.contains(&name)) || matches!(name, "for" | "case" | "export" | "local" | "readonly");
    !(free || fixed(built) || built_word_harmless(name, args, built))
}

/// Where a rule leaves a built word no say. `printf` reads options only before its format, and
/// `nmcli` none that write after `show`. `ethtool` given one word alone has no device for an option
/// to act on. `test` takes an operand from it next to an operator written out, so -v, the one
/// operator bash runs anything for, isn't one it can be. Built operands must remain one word:
/// an unquoted expansion can split into additional operators, including bash's `-v`.
fn built_word_harmless(name: &str, args: &[&str], built: &[Built]) -> bool {
    let fixed = |i: usize| built.get(i) == Some(&Built::Fixed);
    let one = |i: usize| matches!(built.get(i), Some(Built::Fixed | Built::OneWord));
    match name {
        "printf" => fixed(0) && args.first().is_some_and(|f| !f.starts_with('-')),
        "ethtool" => matches!(built, [Built::OneWord]),
        "nmcli" => {
            let rest = nmcli_command(args);
            let verb = args.len() - rest.len() + 1;
            rest.len() > 1 && (0..=verb).all(fixed)
        }
        "test" | "[" => match args.iter().copied().take_while(|a| *a != "]").count() {
            1 => matches!(built.first(), Some(Built::OneWord)),
            2 => fixed(0) && args.first() != Some(&"!") && one(1),
            3 => fixed(1) && one(0) && one(2),
            _ => false,
        },
        _ => false,
    }
}

fn words_read_only(words: &[String]) -> bool {
    let mut w: Vec<&str> = words.iter().map(String::as_str).collect();
    loop {
        let Some(&first) = w.first() else { return true };
        if CLOSERS.contains(&first) {
            w.remove(0);
            continue;
        }
        if KEYWORDS.contains(&first) {
            w.remove(0);
            continue;
        }
        // NAME=value before a command (or on its own) only sets a variable, unless the
        // variable makes the next program load or run something (LD_PRELOAD, PATH, BASH_ENV…).
        if is_assignment(first) {
            if hijacks(first) {
                return false;
            }
            w.remove(0);
            continue;
        }
        // `export -f` exports a function; any other option isn't a plain assignment either.
        if first == "export" || first == "local" || first == "readonly" {
            return w[1..].iter().all(|a| !a.starts_with('-') && !hijacks(a) && (is_assignment(a) || a.chars().all(|c| c.is_alphanumeric() || c == '_')));
        }
        break;
    }
    // A command word that is a substitution, a variable, a glob or a path outside the system's
    // folders: whatever its name says, it isn't the program these rules know (T2 blocker 1).
    if hides_command(w[0], looks_globbed(w[0])) || !system_program(w[0]) {
        return false;
    }
    let name = w[0].rsplit('/').next().unwrap_or(w[0]);
    if NEVER_READ.contains(&name) {
        return false;
    }
    // A read-tier builtin that re-evaluates its operands (bash runs the contents of an array
    // subscript or an arithmetic context, so `[[ 'a[$(touch x)]' -eq 0 ]]` fires the `$(…)` even
    // though it's single-quoted): a `[` or a command substitution in any of its operands, quoted
    // or not (the SUB placeholder too), makes it a change.
    if reevaluates(name, &w[1..]) && w[1..].iter().any(|a| a.contains('[') || a.contains("$(") || a.contains('`')) {
        return false;
    }
    // bash's `test -v NAME` evaluates a subscript in NAME however it got there (`test -v "$y"`
    // with y set to `a[$(…)]` runs it), so -v isn't a read with any operand.
    if matches!(name, "test" | "[") && w[1..].contains(&"-v") {
        return false;
    }
    // A builtin that sets a variable reads only when it's one of the script's own: setting the
    // shell's (PATH, IFS, ENV…) changes what later commands run (`read PATH </tmp/p; cat x`).
    if !sets_script_variables(name, &w[1..]) {
        return false;
    }
    let normal = getopt(name, &w[1..]);
    let args: &[&str] = &normal.iter().map(String::as_str).collect::<Vec<_>>();
    // What bars a read, however it's shortened; `exact` is for what makes a command one, which a
    // shortened word mustn't pass for.
    let has = |flags: &[&str]| args.iter().any(|a| flags.iter().any(|f| names_option(a, f)));
    let exact = |flags: &[&str]| args.iter().any(|a| flags.iter().any(|f| a == f || (f.starts_with("--") && a.starts_with(&format!("{f}=")))));
    match name {
        // A loop header is just a list of words; the body is checked on its own.
        "for" | "case" => true,
        // Prefixes that run the rest as a command.
        // -E and --preserve-env keep the caller's environment, whatever it holds; -i and -s
        // start a shell; -e edits a file; -R runs in another root. Only sudo's own options count (up to the
        // command it runs), and only the long ones that say who it runs as and how it asks.
        "sudo" if args.iter().take_while(|a| a.starts_with('-')).any(|a| match a.strip_prefix("--") {
            Some(long) => !matches!(long.split('=').next().unwrap_or(long), "" | "user" | "group" | "non-interactive" | "stdin" | "prompt" | "reset-timestamp"),
            None => a[1..].chars().take_while(|c| !"ugpChUrtD".contains(*c)).any(|c| matches!(c, 'E' | 'e' | 'i' | 's')),
        }) => false,
        "sudo" => words_read_only(&skip_options(args, &["-u", "-g", "-p", "-C", "-h", "-U", "-r", "-t", "-D"]).iter().map(|s| s.to_string()).collect::<Vec<_>>()),
        // The command a wrapper runs, past its own options (`wrapped`); an option SparkBox can't
        // read, or a process's priority, affinity or I/O class set, isn't a read.
        "nice" | "ionice" | "nohup" | "stdbuf" | "command" | "exec" | "chrt" | "taskset" | "numactl" | "timeout" if !args.is_empty() => match wrapped(name, args) {
            Some(Wrapped::Runs(rest)) => words_read_only(&to_owned(&rest)),
            Some(Wrapped::Shows) => true,
            Some(Wrapped::Sets) | None => false,
        },
        // Assignments may hijack the program. -S, wherever it sits in a cluster (`-iS`), splits a
        // string into a command line of its own, which `core` follows: never a read.
        "env" => match past_options(args, &ENV) {
            Some((seen, rest)) if !seen.contains(&'S') && !args.iter().any(|a| is_assignment(a) && hijacks(a)) => {
                words_read_only(&to_owned(&rest.into_iter().filter(|a| !is_assignment(a)).collect::<Vec<_>>()))
            }
            _ => false,
        },
        // xargs appends the words it's fed as more operands, so it reads only when it runs no
        // command (echo) or a program that keeps reading whatever operands it gets.
        "xargs" => match past_options(args, &XARGS) {
            Some((_, rest)) => match rest.first() {
                None => true,
                Some(inner) => XARGS_READERS.contains(&inner.rsplit('/').next().unwrap_or(inner)) && words_read_only(&to_owned(&rest)),
            },
            None => false,
        },
        // With -x the words are watch's command; without, it joins them with spaces and runs the
        // line with `sh -c` (`watch ls ';' reboot` reboots).
        "watch" => match past_options(args, &WATCH) {
            Some((seen, rest)) if seen.contains(&'x') => words_read_only(&to_owned(&rest)),
            Some((_, rest)) => read_only(&rest.join(" ")),
            None => false,
        },
        // Its `-c` script, wherever `-c` sat among the options (`ShellArgs`). -i, --rcfile or
        // --init-file source extra code (an rcfile, from /dev/stdin too), so a `-c` script isn't
        // all it runs; without `-c` it reads a script from stdin or a file.
        "sh" | "bash" | "dash" | "zsh" => {
            let sh = ShellArgs::of(args);
            !sh.sources_rcfile && !sh.operands.is_empty() && sh.script().is_some_and(|script| read_only(&script))
        }
        "nvidia-smi" => nvidia_smi_read_only(args),
        "docker" => docker_read_only(args),
        // The verb is the first word that isn't an option or an option's value: `-p status stop x`
        // stops x. An option off these lists might take the next word, so it isn't a read.
        "systemctl" => match known_options(args, SYSTEMCTL_FLAGS, SYSTEMCTL_VALUED).as_deref() {
            Some([]) => true,
            Some([v, ..]) => v.starts_with("list-") || v.starts_with("is-") || matches!(*v, "status" | "show" | "cat" | "get-default" | "show-environment"),
            None => false,
        },
        // Viewing only: the other long options rotate, flush, vacuum or write a file.
        "journalctl" => long_options(args).all(|o| JOURNALCTL_LONG.contains(&o)),
        "ip" => ip_read_only(args),
        // Every option one that shows: the setters, the tests and -w (`-w eth0 data f` writes f).
        "ethtool" => args.iter().filter(|a| a.starts_with('-') && a.len() > 1).all(|a| ETHTOOL_LOOKS.contains(a)),
        "rdma" => rdma_read_only(args),
        "tc" => tc_read_only(args),
        // -o and -c can set hooks (DPkg::Pre-Invoke) that run as root: only the -o settings
        // SparkBox's own dry run passes leave it a read (`apt_option_reads`). `update` writes the
        // package lists even with -s.
        "apt" | "apt-get" if !apt_options_read_only(args) => false,
        "apt" | "apt-get" if apt_verb(args) == Some("update") => false,
        // -d and --download-only write <package>.changelog into the folder it runs in.
        "apt" | "apt-get" if apt_verb(args) == Some("changelog") => known_options(args, &["-q", "--quiet", "--print-uris"], &["-o", "--option"]).is_some(),
        "apt" => apt_verb(args).is_some_and(|v| matches!(v, "list" | "show" | "search" | "policy" | "depends" | "rdepends")) || exact(&["-s", "--simulate", "--dry-run", "--just-print", "--no-act"]),
        // A dry run of named packages only (C2, C2′): a .deb path or a pattern isn't one.
        "apt-get" => (exact(&["-s", "--simulate", "--dry-run", "--just-print", "--no-act", "--recon"]) && apt_packages_named(args)) || apt_verb(args) == Some("check"),
        // Firmware updates and devices as fwupd knows them, never applied, refreshed or reported.
        "fwupdmgr" => {
            args.first().is_some_and(|v| matches!(*v, "get-updates" | "get-devices" | "get-history"))
                && args[1..].iter().all(|a| matches!(*a, "--json" | "--no-unreported-check" | "--no-metadata-check"))
        }
        // What snapd would refresh, and what's installed; a plain `snap refresh` installs.
        "snap" => matches!(args, ["refresh", "--list"] | ["list"]),
        "ollama" => matches!(args, ["--version"] | ["-v"]),
        // needrestart in batch mode listing what still runs old code (`-r l`); any other mode
        // restarts services, and its -c config file is code it runs.
        "needrestart" => {
            let mut a: Vec<&str> = args.to_vec();
            a.sort_unstable();
            a == ["-b", "-r", "l"] && args.windows(2).any(|w| w == ["-r", "l"])
        }
        // `-c showhold hold jq` holds jq: -c and -o take a value, so neither is a read.
        "apt-mark" => matches!(known_options(args, &["-q", "--quiet"], &[]).as_deref(), Some([v, ..]) if v.starts_with("show")),
        // Their looking verbs. -c and -o can set Dir::Bin::dpkg, a program apt runs as it starts
        // (`apt-config -o Dir::Bin::dpkg=/tmp/x dump` runs /tmp/x), and -p and -s, like
        // Dir::Cache::pkgcache, name a cache any verb writes; gencaches writes one too.
        "apt-cache" => match known_options(args, APT_CACHE_FLAGS, &[]).as_deref() {
            Some([]) => true,
            Some([v, ..]) => APT_CACHE_LOOKS.contains(v),
            None => false,
        },
        "apt-config" => match known_options(args, &["--empty", "--no-empty", "-h", "--help", "-v", "--version"], &["--format"]).as_deref() {
            Some([]) => true,
            Some([v, ..]) => matches!(*v, "dump" | "shell"),
            None => false,
        },
        // Every option must be one that looks (dpkg -l -i x.deb installs x.deb).
        "dpkg" => {
            const LOOKS: &[&str] = &["-l", "--list", "-C", "--audit", "-L", "--listfiles", "-s", "--status", "-S", "--search", "-p", "--print-avail", "--get-selections", "-c", "--contents", "-I", "--info", "--compare-versions", "--print-architecture", "--version"];
            let opts: Vec<&&str> = args.iter().filter(|a| a.starts_with('-')).collect();
            !opts.is_empty() && opts.iter().all(|a| LOOKS.contains(a) || a.starts_with("--admindir=") || a.starts_with("--showformat=") || a.starts_with("-f"))
        }
        "curl" => curl_read_only(args),
        "wget" => wget_read_only(args),
        "sed" => sed_read_only(args),
        "awk" | "gawk" | "mawk" | "nawk" => awk_read_only(args),
        // -o; the long options past this list are --output and --compress-program.
        "sort" => !has(&["-o"]) && long_options(args).all(|o| SORT_LONG.contains(&o)),
        // `ss -K` closes the sockets it lists; `-D` dumps them into a file.
        "ss" => !has(&["-K", "--kill", "-D", "--diag"]) && !args.iter().any(|a| a.starts_with('-') && !a.starts_with("--") && (a.contains('K') || a.contains('D'))),
        // With two file names they write the second.
        "xxd" => operands(args).len() <= 1,
        "iconv" => !has(&["-o", "--output"]),
        "tree" => tree_read_only(args),
        "sar" => !has(&["-o"]),
        "dmidecode" => !has(&["--dump-bin"]),
        "lshw" => !has(&["-dump", "--dump"]),
        "file" => !has(&["-C", "--compile"]),
        "sensors" => !has(&["-s", "--set"]),
        // --pre and --hostname-bin run a program. Neither is on the list.
        "rg" => long_options(args).all(|o| RG_LONG.contains(&o)),
        // Execute this program and watch it.
        "pidstat" => !has(&["-e"]),
        // -s names the service to load: `-s x/../evil` loads libnss_x/../evil.so.2 from where it runs.
        "getent" => !has(&["-s", "--service"]),
        // -c writes back the cache it names: only /dev/null is no cache.
        "blkid" => blkid_read_only(args),
        // -q, -Q and -O (net.cache_name): the names it looks up go in a cache file.
        "lspci" => known_options(args, LSPCI_FLAGS, LSPCI_VALUED).is_some(),
        // +cmd runs a command; -o/-O and --log-file copy what's shown into a file; -k loads key bindings.
        "less" | "more" => !args.iter().any(|a| a.starts_with('+')) && !has(&["-o", "-O", "--log-file", "--LOG-FILE", "-k", "--lesskey-file", "--lesskey-src"]),
        "nvcc" => args.iter().all(|a| matches!(*a, "--version" | "-V")),
        "nvidia-ctk" => args.iter().all(|a| matches!(*a, "--version" | "-v" | "cdi" | "list")) && !args.is_empty(),
        // `uniq in out` writes `out`.
        "uniq" => operands(args).len() <= 1,
        "find" => !has(&["-delete", "-exec", "-execdir", "-ok", "-okdir", "-fprint", "-fprint0", "-fprintf", "-fls"]),
        "date" => !has(&["-s", "--set"]) && !args.iter().any(|a| !a.starts_with('+') && !a.starts_with('-') && a.len() >= 8 && a.chars().all(|c| c.is_ascii_digit() || c == '.')),
        "hostname" => args.iter().all(|a| a.starts_with('-')) && !has(&["-F", "--file", "-b", "--boot"]),
        // Verbs that only look; verbs that show a setting with no arguments and set it with
        // some (resolvectl dns eth0 1.1.1.1, hostnamectl hostname x) are Read bare only.
        "hostnamectl" | "timedatectl" | "loginctl" | "networkctl" | "resolvectl" | "busctl" | "localectl" => {
            // As systemctl: `-p status set-ntp false`, `--match status call …` run their verb.
            let Some(words) = known_options(args, SYSTEMD_FLAGS, SYSTEMD_VALUED) else { return false };
            match words.split_first() {
                None => true,
                Some((v, rest)) if v.starts_with("list") || matches!(*v, "status" | "show" | "show-session" | "show-user" | "show-seat" | "timesync-status" | "show-timesync" | "query" | "statistics" | "lldp") => {
                    rest.iter().all(|a| !a.contains('='))
                }
                Some((v, rest)) => rest.is_empty() && matches!(*v, "dns" | "domain" | "default-route" | "llmnr" | "mdns" | "dnssec" | "dnsovertls" | "nta" | "hostname" | "icon-name" | "chassis" | "deployment" | "location"),
            }
        }
        "nmcli" => nmcli_read_only(args),
        // -f is -p ("the NetBSD way"): it loads a file of settings.
        "sysctl" => !has(&["-w", "--write", "-p", "-f", "--load", "--system"]) && !args.iter().any(|a| a.contains('=')),
        "dmesg" => !has(&["-c", "-C", "--clear", "--read-clear", "-D", "--console-off", "-E", "--console-on", "-n", "--console-level"]),
        "mount" => args.is_empty() || args.iter().all(|a| matches!(*a, "-l" | "--show-labels")),
        "python" | "python3" => {
            // SparkBox's node helper: these verbs only look (sparkjob.py's own tests check that),
            // except that `list` also marks a dead job failed and removes the secret files of jobs
            // that are over. `list --look`, which is what look-only sends, doesn't.
            // Only the exact path SparkBox runs it from counts, never any */.sparkbox/sparkjob.py.
            // `check` runs a container to read vLLM's version, unless it's the quick one (with
            // the instance named or not).
            let sparkjob = args.first().is_some_and(|a| *a == "~/.sparkbox/sparkjob.py");
            (args.len() == 1 && matches!(args[0], "--version" | "-V"))
                || (sparkjob && args.get(1).is_some_and(|v| matches!(*v, "list" | "ls" | "read" | "stat" | "du" | "df" | "summary" | "tree" | "backups" | "scan" | "size" | "manifest" | "hash")))
                || (sparkjob && matches!(args, [_, "recipe-import", _] | [_, "download-tools"]))
                || (sparkjob && args.get(1) == Some(&"check") && matches!(&args[2..], [_, _, "quick"] | [_, _, "quick", "--instance", _] | [_, _, _, "quick"] | [_, _, _, "quick", "--instance", _]))
        }
        "crontab" => args == ["-l"],
        "smartctl" => !has(&["-t", "--test", "-s", "--smart", "-o", "--offlineauto", "-S", "--saveauto", "-X", "--abort", "--set"]) && !smartctl_log_sets(args),
        "nvme" => args.first().is_some_and(|v| matches!(*v, "list" | "smart-log" | "id-ctrl" | "id-ns" | "error-log" | "fw-log" | "list-subsys" | "show-regs")),
        "tar" => tar_read_only(args),
        // -c sets config (core.pager, core.fsmonitor run programs); --output writes a file. Only
        // the read verbs, and branch, tag and remote only as listings: a name after them
        // creates, moves or prunes a ref.
        "git" => {
            let writes = has(&["-c", "-C", "--config-env", "--output", "-o", "--exec-path", "--ext-diff", "--git-dir", "--work-tree"])
                || args.iter().any(|a| a.starts_with("--output=") || a.starts_with("--config-env=") || a.starts_with("--exec-path=") || a.starts_with("--git-dir=") || a.starts_with("--work-tree="));
            // Only these before the verb: `--namespace` takes the next word, so `git --namespace log
            // commit` commits.
            let Some(words) = past_exact_options(args, GIT_FLAGS, &[]) else { return false };
            let Some((&verb, rest)) = words.split_first() else { return !writes };
            let operands: Vec<&&str> = rest.iter().filter(|a| !a.starts_with('-')).collect();
            let listing = |flags: &[&str]| rest.iter().filter(|a| a.starts_with('-')).all(|a| flags.contains(a) || flags.iter().any(|f| f.ends_with('=') && a.starts_with(f)));
            !writes
                && match verb {
                    "status" | "log" | "diff" | "show" | "rev-parse" | "describe" | "ls-files" | "ls-tree" | "cat-file" | "blame" | "shortlog" => true,
                    // `git grep -O`/`--open-files-in-pager` runs the pager (or a command) on the hits,
                    // however it's spelled: in a cluster (`-nO`), with a command (`-Ovim`,
                    // `--open-files-in-pager=vim`) or shortened (`--open`; git takes any
                    // unambiguous prefix, and no other git grep option starts with "op").
                    "grep" => !rest.iter().any(|a| {
                        let long = a.strip_prefix("--").map(|l| l.split('=').next().unwrap_or(l));
                        long.is_some_and(|l| !l.is_empty() && "open-files-in-pager".starts_with(l)) || (a.starts_with('-') && !a.starts_with("--") && a.contains('O'))
                    }),
                    "branch" => operands.is_empty() && listing(&["-a", "--all", "-r", "--remotes", "-v", "-vv", "--verbose", "--list", "-l", "--show-current", "--no-color", "--color", "--format="]),
                    "tag" => listing(&["-l", "--list", "-n", "--sort=", "--format=", "--contains", "--points-at"]) && (operands.is_empty() || rest.iter().any(|a| matches!(*a, "-l" | "--list"))),
                    "remote" => rest.iter().all(|a| matches!(*a, "-v" | "--verbose")) || (rest.first().is_some_and(|v| matches!(*v, "get-url" | "show")) && rest.len() <= 3),
                    _ => false,
                }
        }
        "arp" => !has(&["-d", "-s", "-f", "--delete", "--set", "--file"]),
        "route" => !args.iter().any(|a| matches!(*a, "add" | "del" | "delete" | "flush" | "change")),
        "mst" => args.first().is_some_and(|v| *v == "status"),
        "fdisk" | "sfdisk" | "parted" | "sgdisk" => partition_listing(name, args),
        // NVIDIA's own tools, not on the staging node: a query of one device and nothing else, so
        // `mlxlink -d x -q -a DN` isn't a read for having -q in it.
        "mlxlink" | "mlxconfig" | "mlxfwmanager" => mlx_read_only(name, args),
        n => READ_ANY.contains(&n),
    }
}

/// What `args` are to a tool that reads options as getopt does, `flags` alone and `valued` with a
/// value (the next word, or attached: `--name=v`, `-xv`): its other words, in order, wherever the
/// options sat among them. None at an option on neither list: it might take the next word, so what
/// the verb is can't be told, and it might be one that writes.
pub(crate) fn known_options<'a>(args: &[&'a str], flags: &[&str], valued: &[&str]) -> Option<Vec<&'a str>> {
    let mut words = Vec::new();
    let mut it = args.iter().copied();
    while let Some(a) = it.next() {
        if a == "--" {
            words.extend(it);
            break;
        }
        if !a.starts_with('-') || a == "-" {
            words.push(a);
        } else if valued.contains(&a) {
            it.next()?;
        } else if !flags.contains(&a) {
            let attached = a.split_once('=').is_some_and(|(n, _)| valued.contains(&n)) || (!a.starts_with("--") && a.len() > 2 && a.get(..2).is_some_and(|s| valued.contains(&s)));
            if !attached {
                return None;
            }
        }
    }
    Some(words)
}

/// What `args` are after a tool's leading options, each spelled as `flags` or `valued` list it, with
/// a value in the next word or after `=`. None at any other option, shortened ones and `--` among
/// them: it might take the next word, and then the verb is somewhere else (`-fi radio connection
/// delete x` is nmcli's -fields). For tools that take a start of a name, or read options of their
/// own after the verb, where `known_options` can't be used.
fn past_exact_options<'a, 'b>(args: &'b [&'a str], flags: &[&str], valued: &[&str]) -> Option<&'b [&'a str]> {
    let mut i = 0;
    while let Some(&a) = args.get(i) {
        if !a.starts_with('-') || a == "-" {
            break;
        }
        i += 1;
        if flags.contains(&a) || a.split_once('=').is_some_and(|(n, _)| valued.contains(&n)) {
            continue;
        }
        if !valued.contains(&a) {
            return None;
        }
        args.get(i)?;
        i += 1;
    }
    Some(&args[i..])
}

/// git's options before its verb that change nothing about what it does. Not -c, -C, --namespace,
/// --git-dir and the like, nor -p, which starts a pager.
const GIT_FLAGS: &[&str] = &["-P", "--no-pager", "--no-optional-locks", "--no-replace-objects", "--literal-pathspecs", "--glob-pathspecs", "--noglob-pathspecs", "--icase-pathspecs", "--version", "--help", "--html-path", "--man-path", "--info-path"];

/// systemctl's options that take nothing. No -H or -M (another machine), no --root.
const SYSTEMCTL_FLAGS: &[&str] = &[
    "-a", "--all", "-l", "--full", "-q", "--quiet", "--value", "--no-pager", "--no-legend", "--plain", "--failed", "--user", "--system", "-r", "--recursive", "--reverse", "--before", "--after",
    "--show-types", "--no-ask-password", "-h", "--help", "--version",
];
/// systemctl's options that take a value and only choose what's shown.
const SYSTEMCTL_VALUED: &[&str] = &["-p", "--property", "-P", "-t", "--type", "--state", "-n", "--lines", "-o", "--output", "--legend", "--timestamp"];
/// What hostnamectl, timedatectl, loginctl, networkctl, resolvectl, busctl and localectl take
/// alone and only to choose what's shown, and with a value. busctl's --match, --timeout, --address
/// and the like aren't here, nor -H and -M.
const SYSTEMD_FLAGS: &[&str] = &[
    "-a", "--all", "-l", "--full", "-q", "--quiet", "--value", "--no-pager", "--no-legend", "--no-ask-password", "--static", "--transient", "--pretty", "--monitor", "--user", "--system", "-j", "-4", "-6",
    "-h", "--help", "--version",
];
const SYSTEMD_VALUED: &[&str] = &["-p", "--property", "-P", "-n", "--lines", "-o", "--output", "--json", "--legend", "-i", "--interface", "-t", "--type", "-c", "--class"];

/// apt-cache 2.8's verbs, less gencaches. `dotty` and `xvcg` print a graph.
const APT_CACHE_LOOKS: &[&str] = &["showpkg", "showsrc", "stats", "dump", "dumpavail", "unmet", "search", "show", "depends", "rdepends", "pkgnames", "dotty", "xvcg", "policy", "madison"];
/// apt-cache 2.8's options that take nothing, less -c, -o, -p and -s. `--with-source` is out too:
/// it's a file apt reads as metadata.
const APT_CACHE_FLAGS: &[&str] = &[
    "-a", "--all-versions", "--no-all-versions", "-f", "--full", "-g", "--generate", "--no-generate", "-i", "--important", "--installed", "-n", "--names-only", "--all-names", "--no-all-names", "--recurse",
    "--only-source", "-q", "-qq", "--quiet", "-h", "--help", "-v", "--version",
];

/// util-linux 2.39's blkid, less -c, -g and -w.
const BLKID_FLAGS: &[&str] = &["-d", "--no-encoding", "-k", "--list-filesystems", "-l", "--list-one", "-p", "--probe", "-i", "--info", "-D", "--no-part-details", "-h", "--help", "-V", "--version", "-v"];
const BLKID_VALUED: &[&str] = &["-o", "--output", "-s", "--match-tag", "-t", "--match-token", "-L", "--label", "-U", "--uuid", "-H", "--hint", "-S", "--size", "-O", "--offset", "-u", "--usages", "-n", "--match-types"];

fn blkid_read_only(args: &[&str]) -> bool {
    let mut rest = Vec::with_capacity(args.len());
    let mut it = args.iter().copied();
    while let Some(a) = it.next() {
        match a {
            "-c" | "--cache-file" if it.next() != Some("/dev/null") => return false,
            "-c" | "--cache-file" | "--cache-file=/dev/null" => {}
            _ => rest.push(a),
        }
    }
    known_options(&rest, BLKID_FLAGS, BLKID_VALUED).is_some()
}

/// pciutils 3.10's lspci, as `getopt` leaves it, less -q, -Q and -O.
const LSPCI_FLAGS: &[&str] = &["-m", "-t", "-v", "-k", "-x", "-b", "-D", "-P", "-n", "-M", "-G", "--version", "--help"];
const LSPCI_VALUED: &[&str] = &["-s", "-d", "-i", "-p", "-A", "-H", "-F"];

/// tree 2.1's options that only show, less -o and -R (with -H it writes 00Tree.html into each
/// folder). tree isn't getopt: a value comes from the next word, whatever follows in the cluster
/// (`-LR 1` is `-L 1 -R`), so every letter of a cluster counts.
fn tree_read_only(args: &[&str]) -> bool {
    const SHORT: &str = "acdfghilnpqrstuvxACDFHIJLNPQSTUX0123456789";
    const LONG: &[&str] = &[
        "help", "version", "inodes", "device", "noreport", "nolinks", "dirsfirst", "filesfirst", "filelimit", "si", "du", "prune", "timefmt", "ignore-case", "matchdirs", "sort", "charset", "fromfile",
        "fromtabfile", "fflinks", "info", "gitignore", "gitfile", "infofile", "metafirst", "hintro", "houtro",
    ];
    args.iter().filter(|a| a.starts_with('-') && **a != "-" && **a != "--").all(|a| match a.strip_prefix("--") {
        Some(long) => LONG.contains(&long.split('=').next().unwrap_or(long)),
        None => a[1..].chars().all(|c| SHORT.contains(c)),
    })
}

/// ethtool 6.7's options that show, less -w (it writes a dump into a file). `--cable-test` and the
/// setters aren't here, nor -p (it blinks the port) or -t.
const ETHTOOL_LOOKS: &[&str] = &[
    "-a", "--show-pause", "-c", "--show-coalesce", "-g", "--show-ring", "-k", "--show-features", "--show-offload", "-i", "--driver", "-d", "--register-dump", "-e", "--eeprom-dump", "-S", "--statistics",
    "--phy-statistics", "-n", "-u", "--show-nfc", "--show-ntuple", "-T", "--show-time-stamping", "-x", "--show-rxfh-indir", "--show-rxfh", "-P", "--show-permaddr", "-l", "--show-channels",
    "--show-priv-flags", "-m", "--dump-module-eeprom", "--module-info", "--show-eee", "--get-phy-tunable", "--get-tunable", "--show-fec", "--show-tunnels", "--show-module", "--get-plca-cfg",
    "--get-plca-status", "--show-mm", "--show-pse", "-h", "--help", "--version", "--debug", "--json", "-I", "--include-statistics",
];

/// NetworkManager's looks (the Connect wizard's Wake-on read): device and connection show/status,
/// general status, a radio's state. Objects may be abbreviated (`d s`). nmcli takes a start of an
/// option's name too (`-fi` is -fields), so only these, spelled out: any other might take the next
/// word, and the command is then somewhere else.
fn nmcli_read_only(args: &[&str]) -> bool {
    const FLAGS: &[&str] = &["-t", "--terse", "-p", "--pretty", "-v", "--version", "-h", "--help"];
    const VALUED: &[&str] = &["-g", "--get-values", "-f", "--fields", "-m", "--mode", "-c", "--colors", "-e", "--escape", "-w", "--wait"];
    let Some(rest) = past_exact_options(args, FLAGS, VALUED) else { return false };
    let Some((obj, more)) = rest.split_first() else { return true };
    let is = |full: &str| !obj.is_empty() && full.starts_with(*obj);
    let verb = more.first().copied();
    ((is("device") || is("connection")) && matches!(verb, None | Some("show" | "status" | "s")))
        || (is("general") && matches!(verb, None | Some("status")))
        || (is("radio") && more.len() <= 1 && verb.is_none_or(|v| !v.is_empty() && ["all", "wifi", "wwan"].iter().any(|r| r.starts_with(v))))
}

/// nmcli's command, past its options as nmcli reads them: any start of a name, with one dash or two,
/// and for the ones that take a value the next word, unless it's attached with `=`.
fn nmcli_command<'a, 'b>(args: &'b [&'a str]) -> &'b [&'a str] {
    const VALUED: &[&str] = &["mode", "colors", "escape", "fields", "get-values", "wait"];
    let mut i = 0;
    while let Some(&a) = args.get(i) {
        if !a.starts_with('-') {
            break;
        }
        let name = a.trim_start_matches('-');
        i += if !a.contains('=') && !name.is_empty() && VALUED.iter().any(|v| v.starts_with(name)) { 2 } else { 1 };
    }
    &args[i.min(args.len())..]
}

/// `tc [options] <object> show|list|ls …`. -batch runs every line of a file, whatever follows it
/// (`tc -batch f show`), so the words after the options are read in their places.
fn tc_read_only(args: &[&str]) -> bool {
    const FLAGS: &[&str] = &["-s", "-stats", "-statistics", "-d", "-details", "-r", "-raw", "-p", "-pretty", "-i", "-iec", "-g", "-graph", "-j", "-json", "-nm", "-name", "-t", "-tshort"];
    match past_exact_options(args, FLAGS, &["-n", "-net", "-netns"]) {
        Some([object, verb, ..]) => matches!(*object, "qdisc" | "class" | "filter" | "chain" | "action" | "actions") && matches!(*verb, "show" | "list" | "ls"),
        _ => false,
    }
}

/// `rdma [options] <object> [show|list …]`. -b runs a file, as tc's does.
fn rdma_read_only(args: &[&str]) -> bool {
    const FLAGS: &[&str] = &["-V", "-Version", "-d", "-details", "-j", "-json", "-p", "-pretty", "-r", "-raw"];
    match known_options(args, FLAGS, &[]).as_deref() {
        Some([] | [_]) => true,
        Some([_, verb, ..]) => matches!(*verb, "show" | "list"),
        None => false,
    }
}

/// smartctl's `-l scterc,N,M` and `-l scttempint,N` set what the bare word shows ([ATA only]).
fn smartctl_log_sets(args: &[&str]) -> bool {
    let sets = |v: &str| v.starts_with("scterc,") || v.starts_with("scttempint");
    args.iter().enumerate().any(|(i, a)| match a.split_once('=') {
        Some((o, v)) => names_option(o, "--log") && sets(v),
        None => (*a == "-l" || names_option(a, "--log")) && args.get(i + 1).is_some_and(|v| sets(v)),
    })
}

/// A query of one device: `mlxlink -d <dev> -q [--json]`, `mlxconfig -d <dev> query [names]`,
/// `mlxfwmanager [-d <dev>] --query`.
fn mlx_read_only(name: &str, args: &[&str]) -> bool {
    const DEVICE: &[&str] = &["-d", "--dev", "--device"];
    let queried = |flags: &[&str]| args.iter().any(|a| flags.contains(a));
    match name {
        "mlxconfig" => matches!(known_options(args, &[], DEVICE).as_deref(), Some(["query", ..])),
        _ => queried(&["-q", "--query"]) && matches!(known_options(args, &["-q", "--query", "--json"], &["-d", "--dev", "--device", "-p", "--port"]).as_deref(), Some([])),
    }
}

/// apt's verb, past its options and the values of those that take one (`-o X`, `-t release`).
fn apt_verb<'a>(args: &[&'a str]) -> Option<&'a str> {
    let mut it = args.iter();
    while let Some(&a) = it.next() {
        match a {
            "-o" | "--option" | "-c" | "--config-file" | "-t" | "--target-release" | "-a" | "--host-architecture" => {
                it.next();
            }
            "--" => return it.next().copied(),
            a if a.starts_with('-') => {}
            verb => return Some(verb),
        }
    }
    None
}

/// Every operand after apt's verb names a package (`package_spec`), the way SparkBox's dry runs
/// write them (C2's `libfoo`, C2′'s `libfoo=1.2-3`).
fn apt_packages_named(args: &[&str]) -> bool {
    let mut it = args.iter();
    let mut verb = false;
    while let Some(&a) = it.next() {
        match a {
            "-o" | "--option" | "-c" | "--config-file" | "-t" | "--target-release" | "-a" | "--host-architecture" => {
                it.next();
            }
            a if a.starts_with('-') => {}
            _ if !verb => verb = true,
            p if !package_spec(p) => return false,
            _ => {}
        }
    }
    true
}

/// A package as apt takes it: `name` (`^[a-z0-9][a-z0-9+.-]*`, with an optional `:arch`) and an
/// optional `=version` (`^[A-Za-z0-9.+~:-]+`).
pub fn package_spec(p: &str) -> bool {
    let (name, version) = match p.split_once('=') {
        Some((n, v)) => (n, Some(v)),
        None => (p, None),
    };
    let (base, arch) = match name.split_once(':') {
        Some((b, a)) => (b, Some(a)),
        None => (name, None),
    };
    let lower = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    base.starts_with(lower)
        && base.chars().all(|c| lower(c) || "+.-".contains(c))
        && arch.is_none_or(|a| !a.is_empty() && a.chars().all(lower))
        && version.is_none_or(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || ".+~:-".contains(c)))
}

/// No config file, and every -o one of the settings SparkBox's own dry run passes (C2).
fn apt_options_read_only(args: &[&str]) -> bool {
    let mut it = args.iter();
    while let Some(&a) = it.next() {
        match a {
            "--" => break,
            "-c" | "--config-file" => return false,
            "-o" | "--option" => {
                if !it.next().is_some_and(|o| apt_option_reads(o)) {
                    return false;
                }
            }
            a if a.starts_with("--config-file=") => return false,
            a if a.starts_with("--option=") && !apt_option_reads(&a["--option=".len()..]) => return false,
            _ => {}
        }
    }
    true
}

/// The apt settings that only tune how a run waits, downloads and treats changed config files:
/// the dpkg lock wait, keeping downloaded packages, keeping config files, and the download speed
/// limit. Hooks (DPkg::Pre-Invoke…), folders and everything else aren't.
pub fn apt_option_reads(o: &str) -> bool {
    let digits = |v: &str| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit());
    match o.split_once('=') {
        Some(("DPkg::Lock::Timeout" | "Acquire::http::Dl-Limit" | "Acquire::https::Dl-Limit", v)) => digits(v),
        Some(("APT::Keep-Downloaded-Packages", v)) => matches!(v, "true" | "false"),
        _ => matches!(o, "Dpkg::Options::=--force-confdef" | "Dpkg::Options::=--force-confold"),
    }
}

/// An assignment that may change what the next program loads, runs or reads its settings from.
/// An allowlist: only the locale, the time zone, the terminal's width and type and NO_COLOR
/// leave a command Read; GIT_CONFIG_*, HOME, SYSTEMD_PAGER, GCONV_PATH, LD_* and anything
/// else make it a change.
fn hijacks(assignment: &str) -> bool {
    let name = assignment.split_once('=').map_or(assignment, |(n, _)| n);
    // These change how a shell reads what follows: IFS splits words, ENV and BASH_ENV source a
    // file, BASH_FUNC_* defines a function, SHELLOPTS and BASHOPTS set options, PS4 runs under -x.
    if matches!(name, "ENV" | "BASH_ENV" | "IFS" | "SHELLOPTS" | "BASHOPTS" | "PS4") || name.starts_with("BASH_FUNC_") {
        return true;
    }
    !(name.starts_with("LC_") || matches!(name, "LANG" | "TZ" | "COLUMNS" | "TERM" | "NO_COLOR"))
}

/// The word `a` is the option `flag`, however it's written. getopt_long takes any unique start of
/// a long option, with its value after `=` or in the next word, so `--o=f` and `--outp f` are
/// `--output`. A start that's another option's too reads as this one: that only makes a read a
/// change.
fn names_option(a: &str, flag: &str) -> bool {
    if a == flag {
        return true;
    }
    match (flag.strip_prefix("--"), a.strip_prefix("--")) {
        (Some(long), Some(word)) => {
            let word = word.split('=').next().unwrap_or(word);
            !word.is_empty() && long.starts_with(word)
        }
        _ => false,
    }
}

/// The long options among `args`, by name (`--` alone isn't one).
fn long_options<'a>(args: &'a [&'a str]) -> impl Iterator<Item = &'a str> {
    args.iter().filter_map(|a| a.strip_prefix("--")).filter(|l| !l.is_empty()).map(|l| l.split('=').next().unwrap_or(l))
}

/// journalctl's long options that only choose what's shown and how. Not --rotate, --flush, --sync,
/// --vacuum-*, --relinquish-var, --smart-relinquish-var, --setup-keys, --update-catalog (they
/// change the journal) or --cursor-file (it writes one).
const JOURNALCTL_LONG: &[&str] = &[
    "no-pager", "no-hostname", "no-full", "full", "all", "reverse", "output", "output-fields", "unit", "user-unit", "identifier", "exclude-identifier", "priority", "facility", "grep",
    "case-sensitive", "since", "until", "boot", "list-boots", "dmesg", "lines", "no-tail", "follow", "catalog", "utc", "merge", "field", "fields", "disk-usage", "header", "quiet", "system", "user",
    "machine", "namespace", "cursor", "after-cursor", "show-cursor", "pager-end", "verify", "list-catalog", "dump-catalog", "help", "version",
];

/// ripgrep 14.1's long options, less --pre and --hostname-bin, which run a program (--pre-glob only
/// narrows --pre). --search-zip runs gzip, xz and the like from PATH, and only reads.
const RG_LONG: &[&str] = &[
    "after-context", "auto-hybrid-regex", "before-context", "binary", "block-buffered", "byte-offset", "case-sensitive", "color", "colors", "column", "context", "context-separator", "count", "count-matches",
    "crlf", "debug", "dfa-size-limit", "encoding", "engine", "field-context-separator", "field-match-separator", "file", "files", "files-with-matches", "files-without-match", "fixed-strings", "follow",
    "generate", "glob", "glob-case-insensitive", "heading", "help", "hidden", "hyperlink-format", "iglob", "ignore", "ignore-case", "ignore-dot", "ignore-exclude", "ignore-file",
    "ignore-file-case-insensitive", "ignore-files", "ignore-global", "ignore-messages", "ignore-parent", "ignore-vcs", "include-zero", "invert-match", "json", "line-buffered", "line-number", "line-regexp",
    "max-columns", "max-columns-preview", "max-count", "max-depth", "max-filesize", "maxdepth", "messages", "mmap", "multiline", "multiline-dotall", "no-auto-hybrid-regex", "no-binary", "no-block-buffered",
    "no-byte-offset", "no-column", "no-config", "no-context-separator", "no-crlf", "no-encoding", "no-filename", "no-fixed-strings", "no-follow", "no-glob-case-insensitive", "no-heading", "no-hidden",
    "no-ignore", "no-ignore-dot", "no-ignore-exclude", "no-ignore-file-case-insensitive", "no-ignore-files", "no-ignore-global", "no-ignore-messages", "no-ignore-parent", "no-ignore-vcs", "no-include-zero",
    "no-invert-match", "no-json", "no-line-buffered", "no-line-number", "no-max-columns-preview", "no-messages", "no-mmap", "no-multiline", "no-multiline-dotall", "no-one-file-system", "no-pcre2",
    "no-pcre2-unicode", "no-pre", "no-require-git", "no-search-zip", "no-sort-files", "no-stats", "no-text", "no-trim", "no-unicode", "null", "null-data", "one-file-system", "only-matching", "passthrough",
    "passthru", "path-separator", "pcre2", "pcre2-unicode", "pcre2-version", "pre-glob", "pretty", "print0", "quiet", "regex-size-limit", "regexp", "replace", "require-git", "search-zip", "smart-case",
    "sort", "sort-files", "sortr", "stats", "stop-on-nonmatch", "text", "threads", "trace", "trim", "type", "type-add", "type-clear", "type-list", "type-not", "unicode", "unrestricted", "version", "vimgrep",
    "with-filename", "word-regexp",
];

/// GNU sort's long options, less --output and --compress-program.
const SORT_LONG: &[&str] = &[
    "ignore-leading-blanks", "dictionary-order", "ignore-case", "general-numeric-sort", "ignore-nonprinting", "month-sort", "human-numeric-sort", "numeric-sort", "random-sort", "random-source",
    "reverse", "sort", "version-sort", "batch-size", "check", "debug", "files0-from", "key", "merge", "stable", "buffer-size", "field-separator", "temporary-directory", "parallel", "unique",
    "zero-terminated", "help", "version",
];

/// Short options that take a value, per tool, so `getopt` can read `-o/etc/hosts` as `-o` plus
/// a value and `-Hc` as `-H -c`. Tools not listed are read as written.
const GETOPT: &[(&str, &str)] = &[
    ("sort", "oktST"),
    ("date", "sdfr"),
    ("dmesg", "nfFlsS"),
    ("wget", "oaOePTtwiBUQlADRIX"),
    ("iconv", "fto"),
    ("sar", "ofseiPnIm"),
    ("smartctl", "tsoSdTbrnflqvFPBg"),
    ("journalctl", "uptSUnDMgoFcI"),
    ("crontab", "u"),
    ("at", "qft"),
    ("batch", "qft"),
    ("less", "oOtTxbhyjkzP#"),
    ("more", ""),
    ("hostname", "F"),
    ("sysctl", "p"),
    ("git", "Cc"),
    ("ss", "fADFN"),
    ("apt", "oct"),
    ("apt-get", "octa"),
    ("uniq", "fsw"),
    ("xxd", "cglso"),
    ("dmidecode", "tsH"),
    ("arp", "iHf"),
    ("file", "mfFeP"),
    ("blkid", "cHLnoOsStuUw"),
    ("getent", "s"),
    ("lspci", "sdipAHFO"),
    ("sensors", "c"),
    ("usermod", "cdegGlpsuL"),
    ("pkill", "gGPstuUFn"),
    ("killall", "usnoy"),
    ("passwd", "nxwi"),
    ("iptables", "ADIRNXPLSFZtjgioscmpw"),
];

/// `args` as getopt reads them for `name` (see GETOPT): clusters split, attached values apart.
fn getopt(name: &str, args: &[&str]) -> Vec<String> {
    let Some((_, takes)) = GETOPT.iter().find(|(n, _)| *n == name) else { return to_owned(args) };
    let mut out = Vec::with_capacity(args.len());
    let mut options = true;
    for a in args {
        if *a == "--" {
            options = false;
        }
        let short = options && a.len() > 2 && a.starts_with('-') && !a.starts_with("--") && !a[1..].starts_with(|c: char| c.is_ascii_digit());
        if !short {
            out.push(a.to_string());
            continue;
        }
        for (j, c) in a[1..].char_indices() {
            out.push(format!("-{c}"));
            if takes.contains(c) {
                let rest = &a[1 + j + c.len_utf8()..];
                if !rest.is_empty() {
                    out.push(rest.to_string());
                }
                break;
            }
        }
    }
    out
}

/// `sed` that only prints: no -i, no script file, and no command in the script that writes a
/// file (`w`, `W`, `s///w`) or runs one (`e`, `s///e`).
fn sed_read_only(args: &[&str]) -> bool {
    let mut scripts: Vec<&str> = Vec::new();
    let mut explicit = false;
    let mut i = 0;
    let mut operands = Vec::new();
    while i < args.len() {
        let a = args[i];
        if a == "--" {
            operands.extend(&args[i + 1..]);
            break;
        }
        if a == "--expression" {
            explicit = true;
            scripts.extend(args.get(i + 1));
            i += 2;
            continue;
        }
        if a == "--line-length" {
            i += 2;
            continue;
        }
        if let Some(e) = a.strip_prefix("--expression=") {
            explicit = true;
            scripts.push(e);
        } else if let Some(long) = a.strip_prefix("--") {
            // Spelled out, from this list: --in-place and --file aren't, and sed takes `--in`.
            let name = long.split('=').next().unwrap_or(long);
            if !matches!(name, "quiet" | "silent" | "regexp-extended" | "null-data" | "zero-terminated" | "posix" | "debug" | "sandbox" | "separate" | "unbuffered" | "line-length" | "help" | "version") {
                return false;
            }
        } else if a.starts_with('-') && a.len() > 1 {
            // A cluster of short options: -n, -E, -r, -z, -s, -u; -e takes the rest or the next word.
            let flags = &a[1..];
            for (k, c) in flags.char_indices() {
                match c {
                    'i' | 'f' => return false,
                    'e' => {
                        explicit = true;
                        let rest = &flags[k + 1..];
                        if rest.is_empty() {
                            scripts.extend(args.get(i + 1));
                            i += 1;
                        } else {
                            scripts.push(rest);
                        }
                        break;
                    }
                    'l' => {
                        // -l N: line length.
                        if flags[k + 1..].is_empty() {
                            i += 1;
                        }
                        break;
                    }
                    'n' | 'E' | 'r' | 'z' | 's' | 'u' => {}
                    _ => return false,
                }
            }
        } else {
            operands.push(a);
        }
        i += 1;
    }
    if !explicit {
        match operands.first() {
            Some(s) => scripts.push(s),
            None => return true,
        }
    }
    scripts.iter().all(|s| sed_script_reads(s))
}

/// Walks a sed script command by command; false at anything that writes or runs, and at
/// anything it doesn't know (fail closed).
fn sed_script_reads(script: &str) -> bool {
    let c: Vec<char> = script.chars().collect();
    let mut i = 0;
    // Skips a delimited part (/regex/ or s's pieces); None when it never ends.
    fn delimited(c: &[char], mut i: usize, d: char) -> Option<usize> {
        while i < c.len() {
            match c[i] {
                '\\' => i += 2,
                x if x == d => return Some(i + 1),
                _ => i += 1,
            }
        }
        None
    }
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() || ch == ';' || ch == '}' || ch == '!' || ch == ',' || ch == '~' {
            i += 1;
            continue;
        }
        // Addresses: line numbers, $, /regex/ and \cregexc.
        if ch.is_ascii_digit() || ch == '$' || ch == '+' {
            i += 1;
            continue;
        }
        if ch == '/' {
            let Some(j) = delimited(&c, i + 1, '/') else { return false };
            i = j;
            if c.get(i).is_some_and(|x| *x == 'I' || *x == 'M') {
                i += 1;
            }
            continue;
        }
        if ch == '\\' {
            let Some(&d) = c.get(i + 1) else { return false };
            let Some(j) = delimited(&c, i + 2, d) else { return false };
            i = j;
            continue;
        }
        i += 1;
        match ch {
            '{' | '=' | 'd' | 'D' | 'g' | 'G' | 'h' | 'H' | 'l' | 'n' | 'N' | 'p' | 'P' | 'x' | 'z' | 'F' => {}
            'q' | 'Q' | 'L' => {
                while c.get(i).is_some_and(|x| x.is_ascii_digit() || *x == ' ') {
                    i += 1;
                }
            }
            // Labels, branches, and text to append, insert or change: up to the end of the line.
            ':' | 'b' | 't' | 'T' | 'a' | 'i' | 'c' | 'r' | 'R' => {
                // GNU sed ends a label at ";" too, so nothing can hide after one.
                let stop_at_semicolon = matches!(ch, ':' | 'b' | 't' | 'T');
                while i < c.len() && c[i] != '\n' && !(stop_at_semicolon && (c[i] == ';' || c[i] == '}')) {
                    i += 1;
                }
            }
            'y' => {
                let Some(&d) = c.get(i) else { return false };
                let Some(j) = delimited(&c, i + 1, d).and_then(|j| delimited(&c, j, d)) else { return false };
                i = j;
            }
            's' => {
                let Some(&d) = c.get(i) else { return false };
                let Some(j) = delimited(&c, i + 1, d).and_then(|j| delimited(&c, j, d)) else { return false };
                i = j;
                // Flags: g, p, i, I, m, M and a number; e runs the result, w writes a file.
                while let Some(&f) = c.get(i) {
                    match f {
                        'g' | 'p' | 'i' | 'I' | 'm' | 'M' | '0'..='9' => i += 1,
                        'e' | 'w' | 'W' => return false,
                        _ => break,
                    }
                }
            }
            // w and W write a file, e runs a command; anything else is unknown.
            _ => return false,
        }
    }
    true
}

/// `awk` that only prints: only -F and -v, then one program text with no `system(…)`, no pipe
/// (`cmd | getline`, `print | "cmd"`, `|&`) and no `print >`/`>>`. Every other option is out, in
/// any awk: a program file (-f, mawk's `-W exec`), an in-place edit, gawk's output files (-o, -d,
/// -p, -D) and its loaded code (-i, -l). After the program come only files and assignments.
fn awk_read_only(args: &[&str]) -> bool {
    let mut program: Option<&str> = None;
    let mut i = 0;
    while i < args.len() && program.is_none() {
        let a = args[i];
        match a {
            "-F" | "-v" => i += 1,
            "--" => {
                i += 1;
                program = args.get(i).copied();
            }
            _ if a.len() > 2 && (a.starts_with("-F") || a.starts_with("-v")) => {}
            _ if a.starts_with('-') && a.len() > 1 => return false,
            _ => program = Some(a),
        }
        i += 1;
    }
    args[i.min(args.len())..].iter().all(|a| !a.starts_with('-') || *a == "-") && program.is_none_or(awk_program_reads)
}

fn awk_program_reads(prog: &str) -> bool {
    // Strings and regex literals out of the way, so a "|" or ">" in them doesn't count.
    let c: Vec<char> = prog.chars().collect();
    let mut code = String::with_capacity(c.len());
    let mut i = 0;
    // The last character that isn't a space: a / after an operand divides, otherwise it starts a regex.
    let mut prev = '\n';
    while i < c.len() {
        match c[i] {
            '"' => {
                i += 1;
                while i < c.len() && c[i] != '"' {
                    i += if c[i] == '\\' { 2 } else { 1 };
                }
                code.push_str("\"\"");
                i += 1;
                prev = '"';
                continue;
            }
            '/' if matches!(prev, '(' | ',' | '~' | '!' | '{' | '}' | ';' | '&' | '|' | '\n') => {
                i += 1;
                while i < c.len() && c[i] != '/' {
                    i += if c[i] == '\\' { 2 } else { 1 };
                }
                code.push_str("//");
                i += 1;
                prev = '/';
                continue;
            }
            '#' => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            ch => {
                code.push(ch);
                if ch == '\n' || !ch.is_whitespace() {
                    prev = ch;
                }
                i += 1;
            }
        }
    }
    let words: String = code.chars().map(|ch| if ch.is_alphanumeric() || ch == '_' { ch } else { ' ' }).collect();
    if words.split_whitespace().any(|w| w == "system") {
        return false;
    }
    // gawk's @include/@load/@namespace and @fn() indirect calls load or run other code.
    if code.contains('@') {
        return false;
    }
    let b: Vec<char> = code.chars().collect();
    for (k, ch) in b.iter().enumerate() {
        // A single | is a pipe; || is "or".
        if *ch == '|' && b.get(k + 1) != Some(&'|') && (k == 0 || b[k - 1] != '|') {
            return false;
        }
    }
    // `>` inside a print or printf statement (outside parentheses) redirects its output.
    let mut rest = code.as_str();
    while let Some(at) = rest.find("print") {
        let after = &rest[at + 5..];
        let after = after.strip_prefix('f').unwrap_or(after);
        let starts_word = at == 0 || !rest[..at].ends_with(|ch: char| ch.is_alphanumeric() || ch == '_');
        let ends_word = !after.starts_with(|ch: char| ch.is_alphanumeric() || ch == '_');
        if starts_word && ends_word {
            let mut depth = 0i32;
            for ch in after.chars() {
                match ch {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    ';' | '}' | '\n' => break,
                    '>' if depth <= 0 => return false,
                    _ => {}
                }
            }
        }
        rest = after;
    }
    true
}

/// `tar` listing an archive: -t or --list (or `t` in a first word such as `tvzf`) with nothing but
/// -f, -v and the compression letters. Any other option may run a program (--use-compress-program,
/// --to-command, --checkpoint-action), write a file (--index-file, --volno-file) or, after -t,
/// make it something other than a listing (-x, --delete).
fn tar_read_only(args: &[&str]) -> bool {
    const LONG: &[&str] = &["list", "file", "verbose", "gzip", "gunzip", "ungzip", "bzip2", "xz", "zstd"];
    let mut lists = false;
    for (i, a) in args.iter().enumerate() {
        if *a == "--" {
            break;
        }
        if let Some(long) = a.strip_prefix("--") {
            let name = long.split('=').next().unwrap_or(long);
            if !LONG.contains(&name) {
                return false;
            }
            lists |= name == "list";
        } else if i == 0 || (a.starts_with('-') && a.len() > 1) {
            let letters = a.strip_prefix('-').unwrap_or(a);
            if !letters.chars().all(|c| "tvfzjJ".contains(c)) {
                return false;
            }
            lists |= letters.contains('t');
        }
    }
    // A `[user@]host:archive` is read over rsh/rmt, which runs a command on the node.
    lists && !args.iter().any(|a| tar_remote_archive(a))
}

/// `wget` saving nothing and logging nowhere: --spider, or -O to stdout or /dev/null, with only the
/// options that say how to ask. --post-data, --method, --delete-after, -o, -a, -x, -e and the rest
/// aren't on the list.
fn wget_read_only(args: &[&str]) -> bool {
    const LONG: &[&str] = &["spider", "quiet", "no-verbose", "verbose", "server-response", "timeout", "tries", "connect-timeout", "read-timeout", "dns-timeout", "no-check-certificate", "user-agent", "header", "output-document", "help", "version"];
    let nowhere = |v: Option<&&str>| v.is_some_and(|v| matches!(*v, "-" | "/dev/null"));
    let mut saves_nothing = false;
    for (i, a) in args.iter().enumerate() {
        match *a {
            "--spider" => saves_nothing = true,
            "-O" | "--output-document" => {
                if !nowhere(args.get(i + 1)) {
                    return false;
                }
                saves_nothing = true;
            }
            _ if a.starts_with("--output-document=") => {
                if !nowhere(a.split_once('=').map(|(_, v)| v).as_ref()) {
                    return false;
                }
                saves_nothing = true;
            }
            _ if a.len() == 2 && a.starts_with('-') && *a != "--" && !"qvnSTtwU".contains(&a[1..]) => return false,
            _ => {}
        }
    }
    saves_nothing && long_options(args).all(|o| LONG.contains(&o))
}

/// A tar archive argument that names a remote host (`[user@]host:path`, and `--file=host:path`):
/// tar reaches it over rsh/rmt, which runs a command on the node.
fn tar_remote_archive(a: &str) -> bool {
    let v = a.strip_prefix("--file=").or_else(|| a.strip_prefix("-f")).unwrap_or(a);
    match v.split_once(':') {
        // The part before the first colon is a host only when it has no slash (a local
        // path like /a:b or ./x:y isn't remote) and isn't empty.
        Some((host, _)) => !host.is_empty() && !host.starts_with('-') && !host.contains('/'),
        None => false,
    }
}

fn is_assignment(w: &str) -> bool {
    match w.split_once('=') {
        Some((name, _)) => !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.starts_with(|c: char| c.is_ascii_digit()),
        None => false,
    }
}

/// Drops leading options (and the values of those that take one) up to the first other word.
fn skip_options<'a>(args: &[&'a str], with_value: &[&str]) -> Vec<&'a str> {
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
        // "-u root" takes the next word; "-uroot" and "--user=root" don't.
        if with_value.contains(&a) {
            i += 2;
        } else {
            i += 1;
        }
    }
    args[i.min(args.len())..].to_vec()
}

fn to_owned(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| s.to_string()).collect()
}

/// What one of a wrapper's options takes.
#[derive(Clone, Copy, PartialEq)]
enum Takes {
    No,
    /// A value, attached or the next word (`-n 5`, `-n5`, `--adjustment=5`, `--adjustment 5`).
    Value,
    /// A value only when attached (`xargs -i{}`, `--replace={}`, `watch -d`): never the next word.
    Attached,
}

/// A wrapper's own options, as its getopt reads them up to its first operand or `--`: the
/// one-letter ones, and the long ones (spelled out) with the letter each stands for (`' '` for
/// none). `numbers`: nice's `-5`, `--5`, `-+5` adjustments.
struct Options {
    short: &'static [(char, Takes)],
    long: &'static [(&'static str, Takes, char)],
    numbers: bool,
}

use Takes::{Attached, Value};

const NICE: Options = Options { short: &[('n', Value)], long: &[("adjustment", Value, 'n')], numbers: true };
const NOHUP: Options = Options { short: &[], long: &[], numbers: false };
const STDBUF: Options = Options { short: &[('i', Value), ('o', Value), ('e', Value)], long: &[("input", Value, 'i'), ("output", Value, 'o'), ("error", Value, 'e')], numbers: false };
/// bash's `command` and `exec` builtins (no long options).
const COMMAND: Options = Options { short: &[('p', Takes::No), ('v', Takes::No), ('V', Takes::No)], long: &[], numbers: false };
const EXEC: Options = Options { short: &[('c', Takes::No), ('l', Takes::No), ('a', Value)], long: &[], numbers: false };
/// util-linux 2.39's chrt, taskset and ionice.
const CHRT: Options = Options {
    short: &[('a', Takes::No), ('b', Takes::No), ('d', Takes::No), ('f', Takes::No), ('i', Takes::No), ('m', Takes::No), ('o', Takes::No), ('p', Takes::No), ('r', Takes::No), ('R', Takes::No), ('v', Takes::No), ('T', Value), ('P', Value), ('D', Value)],
    long: &[
        ("all-tasks", Takes::No, 'a'), ("batch", Takes::No, 'b'), ("deadline", Takes::No, 'd'), ("fifo", Takes::No, 'f'), ("idle", Takes::No, 'i'), ("max", Takes::No, 'm'), ("other", Takes::No, 'o'),
        ("pid", Takes::No, 'p'), ("rr", Takes::No, 'r'), ("reset-on-fork", Takes::No, 'R'), ("verbose", Takes::No, 'v'), ("sched-runtime", Value, 'T'), ("sched-period", Value, 'P'), ("sched-deadline", Value, 'D'),
    ],
    numbers: false,
};
const TASKSET: Options = Options { short: &[('a', Takes::No), ('p', Takes::No), ('c', Takes::No)], long: &[("all-tasks", Takes::No, 'a'), ("pid", Takes::No, 'p'), ("cpu-list", Takes::No, 'c')], numbers: false };
const IONICE: Options = Options {
    short: &[('c', Value), ('n', Value), ('p', Value), ('P', Value), ('u', Value), ('t', Takes::No)],
    long: &[("class", Value, 'c'), ("classdata", Value, 'n'), ("pid", Value, 'p'), ("pgid", Value, 'P'), ("uid", Value, 'u'), ("ignore", Takes::No, 't')],
    numbers: false,
};
/// numactl's memory and CPU policies (its shared-memory options aren't read).
const NUMACTL: Options = Options {
    short: &[('i', Value), ('p', Value), ('m', Value), ('C', Value), ('N', Value), ('c', Value), ('l', Takes::No), ('a', Takes::No)],
    long: &[("interleave", Value, 'i'), ("preferred", Value, 'p'), ("membind", Value, 'm'), ("physcpubind", Value, 'C'), ("cpunodebind", Value, 'N'), ("cpubind", Value, 'c'), ("localalloc", Takes::No, 'l'), ("all", Takes::No, 'a')],
    numbers: false,
};
/// GNU time (`/usr/bin/time`; bash's `time` is a keyword).
const TIME: Options = Options {
    short: &[('o', Value), ('a', Takes::No), ('f', Value), ('p', Takes::No), ('q', Takes::No), ('v', Takes::No)],
    long: &[("output", Value, 'o'), ("append", Takes::No, 'a'), ("format", Value, 'f'), ("portability", Takes::No, 'p'), ("quiet", Takes::No, 'q'), ("verbose", Takes::No, 'v')],
    numbers: false,
};
/// sudo 1.9's long options, spelled out.
const SUDO_LONG: &[&str] = &[
    "askpass", "background", "bell", "close-from", "chdir", "preserve-env", "edit", "group", "set-home", "help", "host", "login", "remove-timestamp", "reset-timestamp", "list", "non-interactive",
    "preserve-groups", "prompt", "chroot", "role", "stdin", "shell", "type", "command-timeout", "other-user", "user", "version", "validate",
];
/// coreutils' timeout.
const TIMEOUT: Options = Options {
    short: &[('f', Takes::No), ('k', Value), ('p', Takes::No), ('s', Value), ('v', Takes::No)],
    long: &[("foreground", Takes::No, 'f'), ("kill-after", Value, 'k'), ("preserve-status", Takes::No, 'p'), ("signal", Value, 's'), ("verbose", Takes::No, 'v')],
    numbers: false,
};
/// coreutils 9.4's env (`-a` is 9.5's argv0, which takes a value).
const ENV: Options = Options {
    short: &[('i', Takes::No), ('0', Takes::No), ('v', Takes::No), ('u', Value), ('C', Value), ('S', Value), ('a', Value)],
    long: &[
        ("ignore-environment", Takes::No, 'i'), ("null", Takes::No, '0'), ("debug", Takes::No, 'v'), ("unset", Value, 'u'), ("chdir", Value, 'C'), ("split-string", Value, 'S'), ("argv0", Value, 'a'),
        ("block-signal", Attached, ' '), ("default-signal", Attached, ' '), ("ignore-signal", Attached, ' '), ("list-signal-handling", Takes::No, ' '),
    ],
    numbers: false,
};
/// GNU findutils' xargs: `-i`, `-e` and `-l` take a value only attached.
const XARGS: Options = Options {
    short: &[
        ('0', Takes::No), ('a', Value), ('d', Value), ('E', Value), ('e', Attached), ('I', Value), ('i', Attached), ('L', Value), ('l', Attached), ('n', Value), ('o', Takes::No), ('P', Value),
        ('p', Takes::No), ('r', Takes::No), ('s', Value), ('t', Takes::No), ('x', Takes::No),
    ],
    long: &[
        ("null", Takes::No, '0'), ("arg-file", Value, 'a'), ("delimiter", Value, 'd'), ("eof", Attached, 'e'), ("replace", Attached, 'i'), ("max-lines", Attached, 'l'), ("max-args", Value, 'n'),
        ("open-tty", Takes::No, 'o'), ("max-procs", Value, 'P'), ("interactive", Takes::No, 'p'), ("no-run-if-empty", Takes::No, 'r'), ("max-chars", Value, 's'), ("verbose", Takes::No, 't'),
        ("exit", Takes::No, 'x'), ("show-limits", Takes::No, ' '), ("process-slot-var", Value, ' '),
    ],
    numbers: false,
};
/// procps-ng 4.0's watch: `-d` takes a value only attached; `-x` runs its words without a shell.
const WATCH: Options = Options {
    short: &[
        ('b', Takes::No), ('c', Takes::No), ('C', Takes::No), ('d', Attached), ('e', Takes::No), ('g', Takes::No), ('n', Value), ('p', Takes::No), ('q', Value), ('r', Takes::No), ('t', Takes::No),
        ('w', Takes::No), ('x', Takes::No),
    ],
    long: &[
        ("beep", Takes::No, 'b'), ("color", Takes::No, 'c'), ("no-color", Takes::No, 'C'), ("differences", Attached, 'd'), ("errexit", Takes::No, 'e'), ("chgexit", Takes::No, 'g'), ("interval", Value, 'n'),
        ("precise", Takes::No, 'p'), ("equexit", Value, 'q'), ("no-rerun", Takes::No, 'r'), ("no-title", Takes::No, 't'), ("no-wrap", Takes::No, 'w'), ("exec", Takes::No, 'x'),
    ],
    numbers: false,
};

/// The words after a wrapper's options (its first operand on), and the letters of the options it
/// was given. Each wrapper's own table, because one list for all of them read flags as taking a
/// value and hid the real program (`command -p rm cat -rf ~/models` runs rm, not cat). None at an
/// option `o` doesn't list, a long one abbreviated, or a value missing: the next word may be the
/// option's value or the command, so what runs can't be told.
fn past_options<'a>(args: &[&'a str], o: &Options) -> Option<(Vec<char>, Vec<&'a str>)> {
    let mut seen = Vec::new();
    let mut i = 0;
    while let Some(&a) = args.get(i) {
        if a == "--" {
            i += 1;
            break;
        }
        if !a.starts_with('-') || a == "-" {
            break;
        }
        i += 1;
        // nice's adjustment written as an option: `-5`, `--5`, `-+5`.
        let n = a[1..].strip_prefix(['-', '+']).unwrap_or(&a[1..]);
        if o.numbers && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if let Some(long) = a.strip_prefix("--") {
            let (name, attached) = match long.split_once('=') {
                Some((n, _)) => (n, true),
                None => (long, false),
            };
            let &(_, takes, letter) = o.long.iter().find(|(n, _, _)| *n == name)?;
            match (takes, attached) {
                (Takes::No, true) => return None,
                (Value, false) => {
                    args.get(i)?;
                    i += 1;
                }
                _ => {}
            }
            seen.push(letter);
            continue;
        }
        // A cluster (`-pc`): letters up to one that takes a value, which takes the rest of the
        // word (or, for a `Value`, the next word when nothing is left).
        for (k, c) in a[1..].char_indices() {
            let &(_, takes) = o.short.iter().find(|(s, _)| *s == c)?;
            seen.push(c);
            if takes != Takes::No {
                if takes == Value && a[1 + k + c.len_utf8()..].is_empty() {
                    args.get(i)?;
                    i += 1;
                }
                break;
            }
        }
    }
    Some((seen, args[i.min(args.len())..].to_vec()))
}

/// What a wrapper does with what follows it.
pub(crate) enum Wrapped<'a> {
    /// Runs this command (none when empty: `nice` alone prints its niceness).
    Runs(Vec<&'a str>),
    /// Only shows a process's setting (`taskset -p PID`, `chrt -p PID`, `ionice -p PID`, `chrt -m`).
    Shows,
    /// Sets a running process's affinity, policy or I/O class (`taskset -p 0x1 PID`, `chrt -o -p 0
    /// PID`, `ionice -c 3 -p PID`).
    Sets,
}

/// What nice, ionice, nohup, stdbuf, command, exec, chrt, taskset, numactl, timeout and GNU time do
/// with `args`, each read with its own options (`Options`): the command after them (past taskset's
/// mask, chrt's priority and timeout's duration), or the process setting it shows or sets. None for another program,
/// or an option SparkBox can't read for it (`past_options`).
pub(crate) fn wrapped<'a>(name: &str, args: &[&'a str]) -> Option<Wrapped<'a>> {
    let options = match name {
        "nice" => &NICE,
        "nohup" => &NOHUP,
        "stdbuf" => &STDBUF,
        "command" => &COMMAND,
        "exec" => &EXEC,
        "chrt" => &CHRT,
        "taskset" => &TASKSET,
        "ionice" => &IONICE,
        "numactl" => &NUMACTL,
        "time" => &TIME,
        "timeout" => &TIMEOUT,
        _ => return None,
    };
    if name == "numactl" && !args.is_empty() && args.iter().all(|a| matches!(*a, "-H" | "--hardware" | "-s" | "--show")) {
        return Some(Wrapped::Shows);
    }
    let (seen, rest) = past_options(args, options)?;
    let has = |c: char| seen.contains(&c);
    Some(match name {
        // `-m` prints the priorities' range; with `-p` the last operand is a process: one operand
        // shows its policy, a priority before it sets it.
        "chrt" if has('m') => Wrapped::Shows,
        "chrt" | "taskset" if has('p') => {
            if rest.len() == 1 {
                Wrapped::Shows
            } else {
                Wrapped::Sets
            }
        }
        // The priority, the mask or CPU list, or the duration comes before the command.
        "chrt" | "taskset" | "timeout" => Wrapped::Runs(rest.get(1..).unwrap_or_default().to_vec()),
        // -p, -P and -u name processes (the operands are more of them, not a command): with -c or
        // -n ionice sets their I/O class, without it shows it.
        "ionice" if has('p') || has('P') || has('u') => {
            if has('c') || has('n') {
                Wrapped::Sets
            } else {
                Wrapped::Shows
            }
        }
        _ => Wrapped::Runs(rest),
    })
}

fn nvidia_smi_read_only(args: &[&str]) -> bool {
    const SETTERS: &[&str] = &[
        "-r", "--gpu-reset", "-pl", "--power-limit", "-ac", "--applications-clocks", "-rac", "--reset-applications-clocks",
        "-pm", "--persistence-mode", "-e", "--ecc-config", "-p", "--reset-ecc-errors", "-c", "--compute-mode", "-lgc",
        "--lock-gpu-clocks", "-rgc", "--reset-gpu-clocks", "-lmc", "--lock-memory-clocks", "-rmc", "--reset-memory-clocks",
        "-mig", "--multi-instance-gpu", "-am", "--accounting-mode", "-caa", "--clear-accounted-apps", "-dm", "--driver-model",
        "-fdm", "--force-driver-model", "-gom", "--gpu-operation-mode", "-cc", "--cuda-clocks", "-acp", "--auto-boost-permission",
        "-ac", "--auto-boost-default",
    ];
    // Spelled out, from this list: whether nvidia-smi takes a shortened option isn't known here.
    const LONG: &[&str] = &[
        "query", "query-gpu", "query-compute-apps", "query-accounted-apps", "query-retired-pages", "query-remapped-rows", "query-supported-clocks", "format", "id", "display", "list-gpus",
        "list-excluded-gpus", "loop", "loop-ms", "xml-format", "matrix", "status", "help", "version", "help-query-gpu", "help-query-compute-apps",
    ];
    if !long_options(args).all(|o| LONG.contains(&o)) {
        return false;
    }
    // The subcommands known only to show, and without one the short options known only to show.
    // There's no nvidia-smi on staging to walk, so what isn't known isn't a read.
    const SHORT: &[&str] = &["-q", "-L", "-B", "-i", "-d", "-x", "-u", "-l", "-lms", "-h"];
    match args.first().filter(|a| !a.starts_with('-')) {
        Some(sub) if !matches!(*sub, "dmon" | "pmon" | "topo") => return false,
        None if !args.iter().filter(|a| a.starts_with('-') && !a.starts_with("--")).all(|a| SHORT.contains(a)) => return false,
        _ => {}
    }
    let setter = args.iter().any(|a| SETTERS.iter().any(|s| a == s || a.starts_with(&format!("{s}="))));
    // Subcommands that change things: MIG instances, clocks, the vGPU scheduler, draining,
    // NVLink control; and -f writes the output into a file.
    let sub = args.iter().any(|a| {
        matches!(*a, "-cgi" | "-dgi" | "-cci" | "-dci" | "--create-gpu-instance" | "--destroy-gpu-instance" | "--create-compute-instance" | "--destroy-compute-instance" | "drain" | "-sc" | "--setcontrol" | "-f" | "--filename")
            || a.starts_with("--filename=")
            || (a.starts_with("-f") && a.len() > 2 && !a.starts_with("--"))
    });
    !setter && !sub
}

/// Docker's own options before the command that take a value (`docker -H unix://… --tlscacert
/// /x run …`), so the value isn't read as the command.
const DOCKER_GLOBAL_VALUE: &[&str] = &["-H", "--host", "--context", "-c", "--config", "-l", "--log-level", "--tlscacert", "--tlscert", "--tlskey"];

/// Docker's own options before a read. Not --config, whose folder's cli-plugins it runs on every
/// command, nor -H and --context, which reach another machine.
const DOCKER_READ_FLAGS: &[&str] = &["-D", "--debug", "--tls", "--tlsverify", "-v", "--version"];
const DOCKER_READ_VALUE: &[&str] = &["-l", "--log-level", "--tlscacert", "--tlscert", "--tlskey"];

fn docker_read_only(args: &[&str]) -> bool {
    let Some(rest) = past_exact_options(args, DOCKER_READ_FLAGS, DOCKER_READ_VALUE) else { return false };
    let Some((&verb, more)) = rest.split_first() else { return true };
    let sub = more.first().copied();
    match verb {
        "ps" | "inspect" | "logs" | "images" | "version" | "info" | "stats" | "top" | "port" | "history" | "diff" | "events" | "search" => true,
        "image" | "container" | "network" | "volume" | "context" | "plugin" | "node" | "service" | "secret" | "config" => {
            sub.is_some_and(|s| matches!(s, "ls" | "list" | "inspect" | "logs" | "top" | "port" | "stats" | "diff" | "history" | "show"))
        }
        "system" => sub.is_some_and(|s| matches!(s, "df" | "info" | "events")),
        // Only compose's own options before its verb: `--workdir ps rm -f` removes a container.
        "compose" => {
            let Some(s) = past_exact_options(more, &["--compatibility", "--all-resources"], COMPOSE_VALUE) else { return false };
            // `config -o f` writes f.
            let writes = s.first() == Some(&"config") && s[1..].iter().any(|a| names_option(a, "--output") || (a.starts_with('-') && !a.starts_with("--") && a.contains('o')));
            !writes && s.first().is_some_and(|s| matches!(*s, "ps" | "ls" | "logs" | "config" | "images" | "top" | "version" | "port" | "events"))
        }
        // `inspect --bootstrap` boots the builder; before the verb, `--builder ls rm x` removes x.
        "buildx" => {
            let Some(s) = past_exact_options(more, &["-D", "--debug"], &["--builder"]) else { return false };
            s.first().is_some_and(|v| matches!(*v, "ls" | "version" | "inspect" | "du")) && !s.iter().any(|a| names_option(a, "--bootstrap"))
        }
        _ => false,
    }
}

/// A `docker run`/`create`/`exec` once wrappers (sudo, env, `sh -c`…) are peeled: whether it
/// runs with the node's root, the host bind mounts it has, and the container's own command.
struct DockerCmd {
    root: bool,
    /// (host source, container destination) for each host bind mount.
    binds: Vec<(String, String)>,
    /// The command the container runs (quotes already removed by `split`).
    cmd: Vec<String>,
}

/// `docker run`/`create` options that take a value, so the image isn't mistaken for one.
const DOCKER_RUN_VALUE: &[&str] = &[
    "-v", "--volume", "--mount", "-e", "--env", "--env-file", "-p", "--publish", "--expose", "--name", "--network",
    "--net", "-u", "--user", "-w", "--workdir", "--entrypoint", "--gpus", "--shm-size", "--restart", "-l", "--label",
    "--label-file", "--cap-add", "--cap-drop", "--device", "-m", "--memory", "--memory-swap", "--memory-reservation",
    "--cpus", "--cpuset-cpus", "--cpuset-mems", "-h", "--hostname", "--ipc", "--pid", "--uts", "--userns", "--add-host",
    "--dns", "--dns-option", "--dns-search", "--log-driver", "--log-opt", "--ulimit", "--health-cmd", "--security-opt",
    "--runtime", "--platform", "--pull", "--stop-signal", "--stop-timeout", "--cidfile", "--detach-keys", "--group-add",
    "--link", "--volumes-from", "--tmpfs", "--sysctl", "--cgroup-parent", "--cgroupns", "--blkio-weight",
    "--device-cgroup-rule", "--mac-address", "--ip", "--ip6", "-a", "--attach", "--cpu-shares", "-c", "--volume-driver",
    "--storage-opt", "--pids-limit", "--oom-score-adj", "--isolation", "--health-interval", "--health-retries",
    "--health-timeout", "--health-start-period", "--gpu", "--annotation",
];

/// `docker exec` options that take a value.
const DOCKER_EXEC_VALUE: &[&str] = &["-e", "--env", "--env-file", "-w", "--workdir", "-u", "--user", "--detach-keys"];

/// The one-letter `docker run`/`create` options that take a value (-a -c -e -h -l -m -p -u -v
/// -w); -d, -i, -t, -P and -q don't.
const DOCKER_RUN_SHORT: &str = "acehlmpuvw";
const DOCKER_EXEC_SHORT: &str = "euw";

/// Docker's options as its CLI (pflag) reads them, up to the image or container: a cluster of
/// one-letter options is taken apart and an attached value set apart, so `-itv /:/host`,
/// `-v/:/host`, `-u0` and `-u=0` read as `-v /:/host` and `-u 0`. `short` names the letters
/// that take a value; the image or container and its command are kept as written.
fn docker_options(args: &[&str], short: &str, long: &[&str]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if a == "--" || !a.starts_with('-') || a == "-" {
            out.extend(args[i..].iter().map(|s| s.to_string()));
            break;
        }
        if a.starts_with("--") {
            out.push(a.to_string());
            if long.contains(&a) {
                out.extend(args.get(i + 1).map(|v| v.to_string()));
                i += 1;
            }
        } else {
            for (j, c) in a[1..].char_indices() {
                out.push(format!("-{c}"));
                if short.contains(c) {
                    // pflag: `-u=0` is `-u 0`; `-u0` is too; a bare `-u` takes the next word.
                    let rest = &a[1 + j + c.len_utf8()..];
                    let rest = if rest.len() > 1 { rest.strip_prefix('=').unwrap_or(rest) } else { rest };
                    if !rest.is_empty() {
                        out.push(rest.to_string());
                    } else {
                        out.extend(args.get(i + 1).map(|v| v.to_string()));
                        i += 1;
                    }
                    break;
                }
            }
        }
        i += 1;
    }
    out
}

/// A `--user` value that is uid 0: `0` or `root`, alone or with a group (`root:root`, `0:100`).
fn is_root_user(v: &str) -> bool {
    matches!(v.split(':').next(), Some("0" | "root"))
}

/// Parses `docker run`/`create`/`exec` (and `docker container run|create|exec`); None when the
/// segment (after wrappers) isn't one.
fn docker_invocation(words: &[&str]) -> Option<DockerCmd> {
    let (core, _root) = core(words);
    let w = match core {
        Core::Words(w) => w,
        Core::Script(_) | Core::Unread(_) => return None,
    };
    let name = w.first()?.rsplit('/').next().unwrap_or("");
    // The standalone compose binary (`docker-compose run …`).
    if name == "docker-compose" {
        return compose_invocation(&w[1..]);
    }
    if name != "docker" {
        return None;
    }
    let rest = skip_options(&w[1..], DOCKER_GLOBAL_VALUE);
    let (&verb, more) = rest.split_first()?;
    // `docker container run|create|exec` is the same command under the management verb.
    let (verb, more) = match (verb, more.split_first()) {
        ("container", Some((&v, m))) => (v, m),
        _ => (verb, more),
    };
    match verb {
        "run" | "create" => Some(parse_docker_run(more)),
        "exec" => Some(parse_docker_exec(more)),
        "compose" => compose_invocation(more),
        _ => None,
    }
}

/// Compose's own options before its command that take a value.
const COMPOSE_VALUE: &[&str] = &["-f", "--file", "-p", "--project-name", "--project-directory", "--env-file", "--profile", "--ansi", "--progress", "--parallel", "--workdir"];
/// `compose run`/`exec` options that take a value, long and one-letter.
const COMPOSE_RUN_VALUE: &[&str] = &[
    "-e", "--env", "-l", "--label", "--name", "-p", "--publish", "-u", "--user", "-v", "--volume", "-w", "--workdir", "--entrypoint", "--cap-add",
    "--cap-drop", "--pull", "--index",
];
const COMPOSE_RUN_SHORT: &str = "elpuvw";

/// `docker compose run|exec|up|create [options] [<service> <command>…]` (and `docker-compose …`):
/// what the service runs with is in a compose file SparkBox doesn't read (a host mount,
/// `privileged: true`), so it counts as root, and the command given is classified like a root
/// container's. Other compose verbs (ps, logs, down…) are left to the other rules.
fn compose_invocation(args: &[&str]) -> Option<DockerCmd> {
    let rest = skip_options(args, COMPOSE_VALUE);
    let (&verb, more) = rest.split_first()?;
    let cmd = match verb {
        "run" | "exec" => {
            let normal = docker_options(more, COMPOSE_RUN_SHORT, COMPOSE_RUN_VALUE);
            let words: Vec<&str> = normal.iter().map(String::as_str).collect();
            // After the service comes its command.
            skip_options(&words, COMPOSE_RUN_VALUE).get(1..).map(to_owned).unwrap_or_default()
        }
        "up" | "create" => Vec::new(),
        _ => return None,
    };
    Some(DockerCmd { root: true, binds: Vec::new(), cmd })
}

/// True for a host path (a bind mount) as opposed to a named volume.
fn is_host_bind(src: &str) -> bool {
    src.starts_with('/') || src.starts_with('~') || src.starts_with('.') || src.starts_with('$') || src.contains('/')
}

/// `src:dst[:opts]` from a `-v`/`--volume` value.
fn parse_bind_v(v: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = v.splitn(3, ':').collect();
    (parts.len() >= 2 && is_host_bind(parts[0])).then(|| (parts[0].to_string(), parts[1].to_string()))
}

/// `type=bind,source=…,destination=…` from a `--mount` value (only a host bind maps back).
fn parse_bind_mount(v: &str) -> Option<(String, String)> {
    let (mut src, mut dst, mut bind) = (None, None, false);
    for kv in v.split(',') {
        match kv.split_once('=') {
            Some(("type", t)) => bind = t == "bind",
            Some(("source" | "src", s)) => src = Some(s),
            Some(("destination" | "dst" | "target", d)) => dst = Some(d),
            _ => {}
        }
    }
    match (bind, src, dst) {
        (true, Some(s), Some(d)) if is_host_bind(s) => Some((s.to_string(), d.to_string())),
        _ => None,
    }
}

/// A `--mount` that's a tmpfs, or a bind mount (`parse_bind_mount` judges where it's from).
/// Anything else is a volume, Docker's default type when none is given.
fn mount_stays_inside(v: &str) -> bool {
    v.split(',').any(|kv| matches!(kv, "type=tmpfs" | "type=bind"))
}

/// The value of an option, attached (`--opt=v`) or the next token; `consumed` says whether the
/// next token was taken.
fn opt_value(attached: Option<&str>, args: &[&str], i: usize, consumed: &mut usize) -> Option<String> {
    match attached {
        Some(v) => Some(v.to_string()),
        None => args.get(i + 1).map(|v| {
            *consumed = 1;
            v.to_string()
        }),
    }
}

fn parse_docker_run(args: &[&str]) -> DockerCmd {
    let normal = docker_options(args, DOCKER_RUN_SHORT, DOCKER_RUN_VALUE);
    let args: &[&str] = &normal.iter().map(String::as_str).collect::<Vec<_>>();
    let (mut root, mut binds) = (false, Vec::new());
    let mut entrypoint = None;
    let mut image_at = None;
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if a == "--" {
            image_at = Some(i + 1);
            break;
        }
        if !a.starts_with('-') {
            image_at = Some(i);
            break;
        }
        let (name, attached) = match a.split_once('=') {
            Some((n, v)) => (n, Some(v)),
            None => (a, None),
        };
        let mut consumed = 0;
        match name {
            "--privileged" => root = attached != Some("false"),
            "--entrypoint" => entrypoint = opt_value(attached, args, i, &mut consumed).filter(|e| !e.is_empty()),
            // The host's namespaces, or another container's (round 4: --userns, --cgroupns and
            // `container:` too).
            "--pid" | "--network" | "--net" | "--ipc" | "--uts" | "--userns" | "--cgroupns" => {
                if opt_value(attached, args, i, &mut consumed).is_some_and(|v| v == "host" || v.starts_with("container:")) {
                    root = true;
                }
            }
            // Devices, capabilities, kernel settings, confinement, another container's volumes and
            // where its cgroup goes: any value reaches past the container (round 4 adds the rest
            // to --cap-add and --device).
            "--cap-add" | "--device" | "--security-opt" | "--sysctl" | "--volumes-from" | "--device-cgroup-rule" | "--cgroup-parent" => {
                let _ = opt_value(attached, args, i, &mut consumed);
                root = true;
            }
            // A runtime other than Docker's own or NVIDIA's runs the container some other way.
            "--runtime" => {
                if opt_value(attached, args, i, &mut consumed).is_none_or(|r| !matches!(r.as_str(), "runc" | "nvidia")) {
                    root = true;
                }
            }
            "-u" | "--user" => {
                if opt_value(attached, args, i, &mut consumed).is_some_and(|u| is_root_user(&u)) {
                    root = true;
                }
            }
            // A host bind mount, or a named volume (Docker's data on the host, maybe another
            // container's).
            "-v" | "--volume" => {
                let v = opt_value(attached, args, i, &mut consumed).unwrap_or_default();
                if let Some(b) = parse_bind_v(&v) {
                    root = true;
                    binds.push(b);
                } else if v.contains(':') {
                    root = true;
                }
            }
            "--mount" => {
                let v = opt_value(attached, args, i, &mut consumed).unwrap_or_default();
                if let Some(b) = parse_bind_mount(&v) {
                    root = true;
                    binds.push(b);
                } else if !mount_stays_inside(&v) {
                    root = true;
                }
            }
            _ if attached.is_none() && DOCKER_RUN_VALUE.contains(&name) => consumed = 1,
            _ => {}
        }
        i += 1 + consumed;
    }
    // `--entrypoint chroot … alpine /host reboot` runs `chroot /host reboot`.
    let after: Vec<String> = image_at.and_then(|at| args.get(at + 1..)).map(|c| c.iter().map(|s| s.to_string()).collect()).unwrap_or_default();
    let cmd = entrypoint.into_iter().chain(after).collect();
    DockerCmd { root, binds, cmd }
}

fn parse_docker_exec(args: &[&str]) -> DockerCmd {
    let normal = docker_options(args, DOCKER_EXEC_SHORT, DOCKER_EXEC_VALUE);
    let args: &[&str] = &normal.iter().map(String::as_str).collect::<Vec<_>>();
    let mut root = false;
    let mut container_at = None;
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if a == "--" {
            container_at = Some(i + 1);
            break;
        }
        if !a.starts_with('-') {
            container_at = Some(i);
            break;
        }
        let (name, attached) = match a.split_once('=') {
            Some((n, v)) => (n, Some(v)),
            None => (a, None),
        };
        let mut consumed = 0;
        match name {
            "--privileged" => root = attached != Some("false"),
            "-u" | "--user" => {
                if opt_value(attached, args, i, &mut consumed).is_some_and(|u| is_root_user(&u)) {
                    root = true;
                }
            }
            _ if attached.is_none() && DOCKER_EXEC_VALUE.contains(&name) => consumed = 1,
            _ => {}
        }
        i += 1 + consumed;
    }
    // After the container name comes its command.
    let cmd = container_at.and_then(|at| args.get(at + 1..)).map(|c| c.iter().map(|s| s.to_string()).collect()).unwrap_or_default();
    DockerCmd { root, binds: Vec::new(), cmd }
}

/// Maps a container command's paths back to the host through the bind mounts, longest
/// destination first (`-v /:/host` makes `/host/var/lib/docker` mean `/var/lib/docker`).
fn map_host_paths(words: &[String], binds: &[(String, String)]) -> Vec<String> {
    let mut sorted: Vec<&(String, String)> = binds.iter().collect();
    sorted.sort_by_key(|(_, dst)| std::cmp::Reverse(dst.len()));
    let one = |w: &str| -> String {
        for (src, dst) in &sorted {
            let dst = dst.trim_end_matches('/');
            if w == dst {
                return src.clone();
            }
            if let Some(rest) = w.strip_prefix(&format!("{dst}/")) {
                return format!("{}/{rest}", src.trim_end_matches('/'));
            }
        }
        w.to_string()
    };
    words
        .iter()
        .map(|w| {
            if !w.contains(char::is_whitespace) {
                return one(w);
            }
            // A script (`sh -c 'rm -rf /host/var/lib/docker'`): each of its words, quotes aside.
            w.split_inclusive(char::is_whitespace)
                .map(|t| {
                    let body = t.trim_end();
                    let lead = body.len() - body.trim_start_matches(['\'', '"', '(']).len();
                    let core = body[lead..].trim_end_matches(['\'', '"', ')', ';']);
                    format!("{}{}{}", &body[..lead], one(core), &t[lead + core.len()..])
                })
                .collect()
        })
        .collect()
}

/// Single-quotes a word so the inner command re-parses as the same words (a word may hold
/// spaces that `split` kept from quoting).
fn squote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// ip's options that only say what's shown and how, and those with a value. ip takes the start of
/// any option's name, and `--` for `-`, so these are the spelled-out ones: `-fa inet l set x down`
/// is -family, and -batch runs a file.
const IP_FLAGS: &[&str] = &[
    "-s", "-stats", "-statistics", "-d", "-details", "-o", "-oneline", "-t", "-timestamp", "-ts", "-tshort", "-j", "-json", "-p", "-pretty", "-c", "-color", "-h", "-human", "-human-readable", "-iec", "-br", "-brief",
    "-r", "-resolve", "-a", "-all", "-N", "-Numeric", "-V", "-Version", "-4", "-6", "-0", "-B", "-M",
];
const IP_VALUED: &[&str] = &["-f", "-family", "-l", "-loops", "-rc", "-rcvbuf", "-n", "-netns"];

/// `ip -br a`, `ip link`, `ip addr show dev x`, `ip route get 1.1.1.1`, `ip -s link`.
fn ip_read_only(args: &[&str]) -> bool {
    let Some(rest) = past_exact_options(args, IP_FLAGS, IP_VALUED) else { return false };
    let Some((object, more)) = rest.split_first() else { return true };
    let is = |full: &str, from: usize| object.len() >= from && full.starts_with(*object);
    // These have an object under the object, and the next word is that: `ip xfrm s flush` is state
    // flush, `ip mptcp l set` limits set. A read spells its verb out.
    if is("xfrm", 1) || is("mptcp", 2) || is("sr", 1) || is("ioam", 2) {
        return matches!(more, [_, "show" | "list", ..]);
    }
    let Some(&action) = more.first() else { return true };
    // ip takes the start of any verb too (`ip rule f` flushes every routing rule), and on a link
    // `s` is set. Only the three objects read that way may shorten it.
    let shortens = is("address", 1) || is("route", 1) || (is("link", 1) && action != "s");
    if shortens {
        !action.is_empty() && ["show", "list", "lst", "ls", "get"].iter().any(|v| v.starts_with(action))
    } else {
        matches!(action, "show" | "list" | "lst" | "ls" | "get")
    }
}

/// ip's command, past its options as ip reads them: any start of a name, `--` for `-`, and for the
/// ones that take a value (-loops, -family, -batch, -rcvbuf and -netns) the next word.
fn ip_command<'a, 'b>(args: &'b [&'a str]) -> &'b [&'a str] {
    let mut i = 0;
    while let Some(&a) = args.get(i) {
        if a == "--" {
            i += 1;
            break;
        }
        if !a.starts_with('-') {
            break;
        }
        let o = if a.starts_with("--") { &a[1..] } else { a };
        let starts = |full: &str, from: usize| o.len() >= from && full.starts_with(o);
        i += if starts("-loops", 1) || starts("-family", 2) || starts("-batch", 2) || starts("-rcvbuf", 3) || starts("-netns", 2) { 2 } else { 1 };
    }
    &args[i.min(args.len())..]
}

/// Long curl options that only say how to fetch and show a URL. The rest upload, write a file,
/// expand variables or send a raw command (--quote, --expand-*), and curl has taken a shortened
/// name in some versions.
const CURL_LONG: &[&str] = &[
    "silent", "show-error", "no-progress-meter", "progress-bar", "fail", "fail-with-body", "fail-early", "location", "head", "include", "get", "verbose", "url", "max-time", "connect-timeout",
    "retry", "retry-delay", "retry-max-time", "retry-connrefused", "retry-all-errors", "write-out", "insecure", "http1.0", "http1.1", "http2", "compressed", "no-buffer", "header", "user-agent",
    "user", "referer", "cookie", "range", "ipv4", "ipv6", "interface", "resolve", "connect-to", "unix-socket", "cacert", "capath", "cert", "key", "proxy", "noproxy", "limit-rate", "max-filesize",
    "max-redirs", "tcp-nodelay", "output", "dump-header", "request", "help", "version",
];

/// Short curl options that only say how to fetch and show a URL (see CURL_LONG; -o, -D and -X
/// are judged by their values below).
const CURL_SHORT: &str = "sSfLIikv46mHAeubrxEwoDXNgG#0123";

fn curl_read_only(args: &[&str]) -> bool {
    // -w '%output{file}…' writes into that file, however the option is spelled.
    if args.iter().any(|a| a.contains("%output")) {
        return false;
    }
    if !long_options(args).all(|o| CURL_LONG.contains(&o)) || args.iter().any(|a| a.starts_with("--output=") || a.starts_with("--dump-header=")) {
        return false;
    }
    // A URL in any other scheme can send a command (gopher, dict, telnet, smb, sftp, ftp).
    if args.iter().filter_map(|a| a.split_once("://")).any(|(scheme, _)| scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c)) && !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https" | "file")) {
        return false;
    }
    // Short options run together ("-sSo out", "-sSo/etc/hosts", "-sD-") are taken apart first:
    // each letter that takes a value gets the rest of the cluster (whatever it is, a path too)
    // or the next word. The round-4 hole was `-o/etc/hosts`: the attached value has a slash, so
    // the old cluster test rejected it and it slipped through as Read (P1.42).
    const TAKES_VALUE: &str = "oDcXdFTKHuAemxUEbrwyYzCtPQ";
    let mut split: Vec<String> = Vec::new();
    let mut k = 0;
    while k < args.len() {
        let a = args[k];
        if a.starts_with('-') && !a.starts_with("--") && a.len() > 2 {
            let body = &a[1..];
            for (j, c) in body.char_indices() {
                if TAKES_VALUE.contains(c) {
                    split.push(format!("-{c}"));
                    let rest = &body[j + c.len_utf8()..];
                    if rest.is_empty() {
                        if let Some(v) = args.get(k + 1) {
                            split.push(v.to_string());
                            k += 1;
                        }
                    } else {
                        split.push(rest.to_string());
                    }
                    break;
                } else if c.is_ascii_alphanumeric() || c == '#' {
                    split.push(format!("-{c}"));
                } else {
                    // A cluster SparkBox can't read (a symbol with no value-taker before it):
                    // treat the whole command as a change, not a read (fail closed).
                    return false;
                }
            }
        } else {
            split.push(a.to_string());
        }
        k += 1;
    }
    let args: Vec<&str> = split.iter().map(String::as_str).collect();
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        let next = args.get(i + 1).copied().unwrap_or("");
        match a {
            "-X" | "--request" if !matches!(next.to_ascii_uppercase().as_str(), "GET" | "HEAD" | "OPTIONS") => return false,
            "-o" | "--output" if next != "/dev/null" => return false,
            "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-urlencode" | "--json" | "-F" | "--form" | "-T" | "--upload-file" | "-O" | "--remote-name" | "-K" | "--config" => return false,
            // Files curl writes besides the output: cookies, headers, traces, caches.
            "-c" | "--cookie-jar" | "--trace" | "--trace-ascii" | "--libcurl" | "--etag-save" | "--hsts" | "--alt-svc" | "--stderr" | "--output-dir" | "--create-dirs" | "-J" | "--remote-header-name" | "--remote-name-all" => return false,
            "-D" | "--dump-header" if !matches!(next, "-" | "/dev/null") => return false,
            // -w '%output{/etc/hosts}…' writes what follows into that file, and -w @f or @- takes
            // the format from a file or stdin, so no word shows it.
            "-w" | "--write-out" if next.contains("%output") || next.starts_with('@') => return false,
            _ if a.strip_prefix("--write-out=").is_some_and(|v| v.contains("%output") || v.starts_with('@')) => return false,
            _ if a.starts_with("-X") && a.len() > 2 && !matches!(&a[2..], "GET" | "HEAD") => return false,
            _ if a.starts_with("--request=") && !matches!(&a[10..], "GET" | "HEAD") => return false,
            _ if a.starts_with("--data") || a.starts_with("--json") || a.starts_with("--form") => return false,
            _ if a.len() == 2 && a.starts_with('-') && a != "--" && !CURL_SHORT.contains(&a[1..]) => return false,
            _ => {}
        }
        i += 1;
    }
    true
}

// ---- Destructive commands

/// What a segment runs once keywords, variable assignments and prefixes are peeled off.
enum Core<'a> {
    Words(Vec<&'a str>),
    /// `sh -c '…'`, `su -c '…'` or `watch '…'`: a script of its own.
    Script(String),
    /// A wrapper (named) with an option SparkBox can't read (`past_options`): what it runs can't
    /// be told, so it's destructive (`verdict`).
    Unread(&'a str),
}

/// Peels `sudo`, `doas`, `pkexec`, `env`, `nice`, `timeout`, `xargs`, `watch`, `sh -c` and the
/// like off a segment; true when one of them runs it as root.
fn core<'a>(words: &[&'a str]) -> (Core<'a>, bool) {
    let mut w: Vec<&'a str> = words.to_vec();
    let mut root = false;
    loop {
        let Some(&first) = w.first() else { return (Core::Words(w), root) };
        if CLOSERS.contains(&first) || KEYWORDS.contains(&first) || is_assignment(first) {
            w.remove(0);
            continue;
        }
        let name = first.rsplit('/').next().unwrap_or(first);
        let args = w[1..].to_vec();
        w = match name {
            "sudo" | "doas" | "pkexec" | "run0" => {
                root = true;
                // A shortened long option may take the next word as its value, so what runs can't be told.
                if name == "sudo" && args.iter().take_while(|a| a.starts_with('-')).filter_map(|a| a.strip_prefix("--")).any(|l| !l.is_empty() && !SUDO_LONG.contains(&l.split('=').next().unwrap_or(l))) {
                    return (Core::Unread(first), root);
                }
                skip_options(&args, &["-u", "-g", "-p", "-C", "-h", "-U", "-r", "-t", "-D", "--user", "--group", "--prompt", "--close-from", "--host", "--other-user", "--role", "--type", "--command-timeout", "--chdir", "--chroot"])
            }
            "su" => {
                root = true;
                return match su_script(&args) {
                    Some(script) => (Core::Script(script), root),
                    None => (Core::Words(Vec::new()), root),
                };
            }
            "env" => match env_runs(&args) {
                EnvRuns::Line(line) => return (Core::Script(line), root),
                EnvRuns::Words(rest) => rest,
                EnvRuns::Unread => return (Core::Unread(first), root),
            },
            // eval runs its words, joined, as a script.
            "eval" => return (Core::Script(args.join(" ")), root),
            // A trap's action runs later in this shell, an alias's text wherever its name is used
            // next: scripts of their own, so a power verb or a destructive command there counts.
            "trap" => {
                let rest: &[&str] = if args.first() == Some(&"--") { &args[1..] } else { &args };
                return match rest.first() {
                    Some(action) if !action.starts_with('-') && rest.len() > 1 => (Core::Script(action.to_string()), root),
                    _ => (Core::Words(w), root),
                };
            }
            "alias" => {
                let text: Vec<&str> = args.iter().filter_map(|a| a.split_once('=').map(|(_, v)| v)).collect();
                return if text.is_empty() { (Core::Words(w), root) } else { (Core::Script(text.join("\n")), root) };
            }
            // Each read with its own options (`exec -a NAME`, `taskset -c 0-3 CMD`, `chrt -f 10
            // CMD`); one that shows or sets a process's setting runs nothing more.
            "exec" | "nice" | "ionice" | "nohup" | "stdbuf" | "command" | "time" | "numactl" | "chrt" | "taskset" | "timeout" => match wrapped(name, &args) {
                Some(Wrapped::Runs(rest)) => rest,
                Some(Wrapped::Shows | Wrapped::Sets) => return (Core::Words(w), root),
                None => return (Core::Unread(first), root),
            },
            // Wrappers that run a real command on the host (P1.42): peel them to find its verb.
            // chroot and flock each drop one operand (the directory, the lock file) first.
            "chroot" => {
                let rest = skip_options(&args, &["--userspec", "--groups"]);
                match rest.split_first() {
                    Some((_dir, cmd)) if !cmd.is_empty() => cmd.to_vec(),
                    _ => return (Core::Words(Vec::new()), root),
                }
            }
            // After the lock file, `-c`/`--command` runs a shell command (`flock /tmp/l -c reboot`).
            "flock" => {
                let rest = skip_options(&args, &["-w", "--wait", "-E", "--conflict-exit-code", "--timeout"]);
                match rest.split_first() {
                    Some((_lock, [c, script])) if matches!(*c, "-c" | "--command") => return (Core::Script(script.to_string()), root),
                    Some((_lock, cmd)) if !cmd.is_empty() => cmd.to_vec(),
                    _ => return (Core::Words(Vec::new()), root),
                }
            }
            "nsenter" => skip_options(&args, &["-t", "--target", "-S", "--setuid", "-G", "--setgid", "-W", "--wd"]),
            "unshare" => skip_options(
                &args,
                &[
                    "--map-user", "--map-group", "--map-users", "--map-groups", "-R", "--root", "-w", "--wd", "--setgroups", "--propagation",
                    "-S", "--setuid", "-G", "--setgid", "-l", "--load-interp", "--monotonic", "--boottime",
                ],
            ),
            "setsid" => skip_options(&args, &[]),
            // runuser runs a command as a user (`-c` a shell command); without -u the words after
            // the user are its shell's, which SparkBox doesn't follow.
            "runuser" => {
                if let Some(script) = su_script(&args) {
                    return (Core::Script(script), root);
                }
                if !args.iter().any(|a| *a == "-u" || *a == "--user" || a.starts_with("--user=")) {
                    return (Core::Words(Vec::new()), root);
                }
                skip_options(&args, &["-u", "--user", "-g", "--group", "-G", "--supp-group", "-s", "--shell", "-w", "--whitelist-environment"])
            }
            "setpriv" => skip_options(
                &args,
                &[
                    "--ruid", "--euid", "--reuid", "--rgid", "--egid", "--regid", "--groups", "--inh-caps", "--ambient-caps", "--bounding-set",
                    "--securebits", "--pdeathsig", "--selinux-label", "--apparmor-profile", "--landlock-access", "--landlock-rule",
                ],
            ),
            // `busybox reboot` is its reboot applet.
            "busybox" if args.first().is_some_and(|a| !a.starts_with('-')) => args,
            "systemd-run" => {
                let rest = skip_options(&args, SYSTEMD_RUN_VALUE);
                // -H runs the command on another machine: left for `destructive` to name.
                let opts = &args[..args.len() - rest.len()];
                if opts.iter().any(|a| *a == "--host" || a.starts_with("--host=") || (a.starts_with("-H") && !a.starts_with("--"))) {
                    return (Core::Words(w), root);
                }
                rest
            }
            "xargs" => match past_options(&args, &XARGS) {
                Some((_, rest)) => rest,
                None => return (Core::Unread(first), root),
            },
            // Without -x watch joins its words with spaces and runs the line with `sh -c`.
            "watch" => match past_options(&args, &WATCH) {
                Some((seen, rest)) if seen.contains(&'x') => rest,
                Some((_, rest)) => return (Core::Script(rest.join(" ")), root),
                None => return (Core::Unread(first), root),
            },
            // `sh -c '…'`, wherever `-c` sits among the options (`bash -lc '…'`, `sh -c -e '…'`,
            // `bash -c -- '…'`): the first operand after them (`ShellArgs`). With -i, --rcfile or
            // --init-file it also sources an rcfile (from /dev/stdin too), so it's not a
            // transparent `-c` wrapper: a change, whatever the script looks like.
            "sh" | "bash" | "dash" | "zsh" => {
                let sh = ShellArgs::of(&args);
                return match sh.script() {
                    Some(script) if !sh.sources_rcfile => (Core::Script(script), root),
                    _ => (Core::Words(w), root),
                };
            }
            // `ssh [options] <host> <command>…` runs the command on another machine: what its
            // shell there reads. Without a command it's a login shell there (`stdin_script`).
            "ssh" if first == "ssh" || first.ends_with("/bin/ssh") => {
                return match ssh_remote(&args) {
                    Some(cmd) => (Core::Script(cmd), root),
                    None => (Core::Words(w), root),
                };
            }
            _ => return (Core::Words(w), root),
        };
    }
}

/// The command `su`/`runuser` runs: `-c SCRIPT` or `--command=SCRIPT`/`--command SCRIPT`, with
/// `-c` reachable inside an option cluster (`su -lc SCRIPT`, `runuser -u root --command=…`). The
/// value-taking short options (`-s`, `-g`, `-G`, `-w`, `-u`) eat the rest of their cluster, so a
/// `c` after one (`su -sc` is `-s c`) isn't the command. A `--` ends the options.
fn su_script(args: &[&str]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if a == "--" {
            return None;
        }
        if let Some(long) = a.strip_prefix("--") {
            match long.split_once('=') {
                Some(("command", v)) => return Some(v.to_string()),
                None if long == "command" => return args.get(i + 1).map(|s| s.to_string()),
                None if matches!(long, "user" | "group" | "supp-group" | "shell" | "whitelist-environment") => i += 1,
                _ => {}
            }
            i += 1;
            continue;
        }
        if a.starts_with('-') && a.len() > 1 {
            let body = &a[1..];
            for (j, c) in body.char_indices() {
                match c {
                    'c' => {
                        let rest = &body[j + 1..];
                        return if rest.is_empty() { args.get(i + 1).map(|s| s.to_string()) } else { Some(rest.to_string()) };
                    }
                    // A value-taking option takes the rest of the cluster (or the next word): stop.
                    's' | 'g' | 'G' | 'w' | 'u' => {
                        if body[j + 1..].is_empty() {
                            i += 1;
                        }
                        break;
                    }
                    _ => {}
                }
            }
        }
        // A `-`, a username or another operand: keep scanning for `-c`.
        i += 1;
    }
    None
}

/// What `env` runs.
pub(crate) enum EnvRuns<'a> {
    /// The words past its options and assignments.
    Words(Vec<&'a str>),
    /// A command line to read again: `env -S`, with its string in place (`env_split`).
    Line(String),
    /// An option SparkBox can't read (`past_options`), or a string with env's own `\_`, `\c` and
    /// `${VAR}`, which are no shell's: what runs can't be told.
    Unread,
}

pub(crate) fn env_runs<'a>(args: &[&'a str]) -> EnvRuns<'a> {
    match (env_split(args), past_options(args, &ENV)) {
        (Some(line), _) if line.contains(['\\', '$']) => EnvRuns::Unread,
        (Some(line), _) => EnvRuns::Line(line),
        (None, Some((_, rest))) => EnvRuns::Words(rest.into_iter().filter(|a| !is_assignment(a)).collect()),
        (None, None) => EnvRuns::Unread,
    }
}

/// `env -S STRING`, also at the end of a cluster (`-iS STRING`, `-iSSTRING`), as `--split-string=
/// STRING` and `--split-string STRING`, or None when there's no `-S`. env splits STRING into
/// words in place and goes on reading its own options, so the answer is an `env` line with STRING
/// where the option stood: `env -iS '-u X rm f' g` is `env -u X rm f 'g'`.
fn env_split(args: &[&str]) -> Option<String> {
    let line = |string: &str, rest: &[&str]| format!("env {string} {}", rest.iter().map(|w| squote(w)).collect::<Vec<_>>().join(" "));
    let mut i = 0;
    while let Some(&a) = args.get(i) {
        i += 1;
        if a == "--" || !a.starts_with('-') || a == "-" {
            return None;
        }
        if let Some(long) = a.strip_prefix("--") {
            let (name, value) = long.split_once('=').map_or((long, None), |(n, v)| (n, Some(v)));
            if name == "split-string" {
                return match value {
                    Some(v) => Some(line(v, &args[i..])),
                    None => args.get(i).map(|v| line(v, &args[i + 1..])),
                };
            }
            if value.is_none() && ENV.long.iter().any(|(n, takes, _)| *n == name && *takes == Value) {
                i += 1;
            }
            continue;
        }
        for (k, c) in a[1..].char_indices() {
            let &(_, takes) = ENV.short.iter().find(|(s, _)| *s == c)?;
            if takes == Takes::No {
                continue;
            }
            let attached = &a[1 + k + c.len_utf8()..];
            if c == 'S' {
                return if attached.is_empty() { args.get(i).map(|v| line(v, &args[i + 1..])) } else { Some(line(attached, &args[i..])) };
            }
            if attached.is_empty() {
                i += 1;
            }
            break;
        }
    }
    None
}

/// The command a `find … -exec|-execdir|-ok|-okdir CMD … ;|+` runs, as words up to the `;` or
/// `+` that ends it; None when `find` has no such action.
fn find_exec<'a>(args: &[&'a str]) -> Option<Vec<&'a str>> {
    let at = args.iter().position(|a| matches!(*a, "-exec" | "-execdir" | "-ok" | "-okdir"))?;
    let after = &args[at + 1..];
    let end = after.iter().position(|a| matches!(*a, ";" | "+")).unwrap_or(after.len());
    let cmd = &after[..end];
    (!cmd.is_empty()).then(|| cmd.to_vec())
}

/// The `-c` script of a shell that also sources an rcfile (`bash -ic …`, `bash --rcfile F -c …`),
/// which `core` leaves as words; None for any other segment.
fn rcfile_script(w: &[&str]) -> Option<String> {
    let (&sh, args) = w.split_first()?;
    if !matches!(sh.rsplit('/').next().unwrap_or(sh), "sh" | "bash" | "dash" | "zsh") {
        return None;
    }
    let sh = ShellArgs::of(args);
    if sh.sources_rcfile { sh.script() } else { None }
}

/// `ssh`'s one-letter options that take a value (OpenSSH has no long ones).
const SSH_VALUE: &str = "BbcDEeFIiJLlmOoPpQRSWw";

/// Where `ssh [options] <destination> [command…]`'s destination is in `args`, past its options
/// (a cluster of letters ends at one that takes a value, attached or the next word).
fn ssh_destination(args: &[&str]) -> Option<usize> {
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if a == "--" {
            return (i + 1 < args.len()).then_some(i + 1);
        }
        if !a.starts_with('-') || a == "-" {
            return Some(i);
        }
        if a[1..].find(|c| SSH_VALUE.contains(c)).is_some_and(|j| j + 2 == a.len()) {
            i += 1;
        }
        i += 1;
    }
    None
}

/// The command `ssh` runs on the other machine: the words after the destination, joined with
/// spaces as ssh sends them for the shell there to read. None without one. OpenSSH still reads
/// its own options after the destination (it permutes), so they're skipped to the command,
/// unless a `--` ended the options — then everything after the first `--` is the command.
fn ssh_remote(args: &[&str]) -> Option<String> {
    let after = args.get(ssh_destination(args)? + 1..)?;
    let cmd = ssh_command_after(after);
    (!cmd.is_empty()).then(|| cmd.join(" "))
}

/// The remote command in the words after `ssh`'s destination: client options there are skipped,
/// and a `--` starts the command (its one leading `--` dropped).
fn ssh_command_after<'a>(after: &[&'a str]) -> Vec<&'a str> {
    let mut i = 0;
    while i < after.len() {
        let a = after[i];
        if a == "--" {
            return after[i + 1..].to_vec();
        }
        if !a.starts_with('-') || a == "-" {
            return after[i..].to_vec();
        }
        if a[1..].find(|c| SSH_VALUE.contains(c)).is_some_and(|j| j + 2 == a.len()) {
            i += 1;
        }
        i += 1;
    }
    Vec::new()
}

/// /dev/stdin and the names it goes by.
fn is_stdin(path: &str) -> bool {
    matches!(path, "/dev/stdin" | "/dev/fd/0" | "/proc/self/fd/0")
}

/// A shell whose BASH_ENV or ENV is stdin (`echo reboot | BASH_ENV=/dev/stdin bash -c true`)
/// sources what it's fed before its `-c` script: that text, when SparkBox can read it.
fn env_fed(seg: &Segment, prev: Option<&Segment>) -> Option<String> {
    let sets = seg.words.iter().any(|w| w.split_once('=').is_some_and(|(k, v)| matches!(k, "BASH_ENV" | "ENV") && is_stdin(v)));
    let shell = seg.words.iter().any(|w| matches!(w.rsplit('/').next().unwrap_or(w), "sh" | "bash" | "dash" | "zsh"));
    (sets && shell).then(|| fed_text(seg, prev)).flatten()
}

/// A shell that reads its script from stdin: `sh`, `bash -s`, `bash -x` (no script file, no
/// `-c`, which `core` reads), one whose rcfile or BASH_ENV/ENV is stdin (`bash --rcfile
/// /dev/stdin -i -c true`, `BASH_ENV=/dev/stdin bash -c true`: it sources what it's fed whatever
/// its `-c` script), or `ssh <host>` with no command (the login shell there). What it
/// runs: Some(Some(text)) when SparkBox can read what it's fed (`fed_text`), Some(None) when it
/// can't. None when the segment isn't one.
fn stdin_script(seg: &Segment, prev: Option<&Segment>) -> Option<Option<String>> {
    let words: Vec<&str> = seg.words.iter().map(String::as_str).collect();
    // A root shell with no command of its own reads stdin the same way (`echo reboot | sudo -s`,
    // `su <<< reboot`), so the fed text runs as root.
    if root_shell_reads_stdin(&words) {
        return Some(fed_text(seg, prev));
    }
    let Core::Words(w) = core(&words).0 else { return None };
    let (&first, args) = w.split_first()?;
    let reads_stdin = match first.rsplit('/').next().unwrap_or(first) {
        // No operand, or -s (the operands are then the script's arguments, not a script file),
        // or an rcfile that is stdin.
        "sh" | "bash" | "dash" | "zsh" => {
            let sh = ShellArgs::of(args);
            sh.operands.is_empty() || sh.s || sh.rcfile_stdin
        }
        "ssh" if first == "ssh" || first.ends_with("/bin/ssh") => ssh_destination(args).is_some_and(|d| d + 1 == args.len()),
        _ => false,
    };
    reads_stdin.then(|| fed_text(seg, prev))
}

/// The text a segment gets on stdin when SparkBox can read it: a here-string, or the words
/// `echo`/`printf` print in the segment that pipes into it.
fn fed_text(seg: &Segment, prev: Option<&Segment>) -> Option<String> {
    if let Some(text) = &seg.here_string {
        return Some(text.clone());
    }
    let producer = prev.filter(|p| seg.piped && !p.writes).map(|p| p.words.iter().map(String::as_str).collect::<Vec<_>>());
    let Some(Core::Words(p)) = producer.as_deref().map(|p| core(p).0) else { return None };
    let (&cmd, printed) = p.split_first().filter(|(c, _)| matches!(c.rsplit('/').next(), Some("echo" | "printf")))?;
    let printed: Vec<&str> = if cmd.ends_with("echo") { printed.iter().copied().skip_while(|a| matches!(*a, "-n" | "-e" | "-E" | "-ne" | "-en")).collect() } else { printed.to_vec() };
    Some(printed.join(" ").replace("\\n", "\n"))
}

/// A shell's arguments (`sh`, `bash`, `dash`, `zsh`) read the way the shell reads them: options
/// first, in clusters (`-ec`, `-ce`, `+e`), `-o NAME`/`+o NAME` and bash's `-O NAME`/`+O NAME`
/// each taking the next word (also from inside a cluster: `-eo pipefail`), long options
/// (`--norc`, `--posix`; `--rcfile FILE`, `--init-file FILE`), and `-` or `--` ending them. With
/// `-c` anywhere among the options, the first operand after them is the script and the rest are
/// its `$0`, `$1`… (POSIX `sh -c [options] command_string [command_name [argument…]]`).
pub struct ShellArgs<'a> {
    /// `-c` (or `+c`, which dash and bash read the same) came among the options.
    pub c: bool,
    /// `-s`: the script comes from stdin, the operands are its arguments.
    pub s: bool,
    /// -i, --rcfile or --init-file: it sources an rcfile (from /dev/stdin too), so a `-c` script
    /// isn't all it runs.
    pub sources_rcfile: bool,
    /// The rcfile is its stdin (`--rcfile /dev/stdin`): it runs what it's fed.
    pub rcfile_stdin: bool,
    /// The words after the options.
    pub operands: Vec<&'a str>,
}

impl<'a> ShellArgs<'a> {
    pub fn of(args: &[&'a str]) -> ShellArgs<'a> {
        let mut out = ShellArgs { c: false, s: false, sources_rcfile: false, rcfile_stdin: false, operands: Vec::new() };
        let mut i = 0;
        while i < args.len() {
            let a = args[i];
            i += 1;
            if a == "-" || a == "--" {
                break;
            }
            if let Some(long) = a.strip_prefix("--") {
                let file = match long.split_once('=') {
                    Some(("rcfile" | "init-file", f)) => Some(f),
                    None if matches!(long, "rcfile" | "init-file") => {
                        i += 1;
                        Some(args.get(i - 1).copied().unwrap_or(""))
                    }
                    _ => None,
                };
                if let Some(f) = file {
                    out.sources_rcfile = true;
                    out.rcfile_stdin |= is_stdin(f);
                }
                continue;
            }
            // A cluster of option letters; a lone `+` is an empty one.
            let minus = a.starts_with('-');
            if !(minus || a.starts_with('+')) {
                i -= 1;
                break;
            }
            for ch in a[1..].chars() {
                match ch {
                    'c' => out.c = true,
                    's' if minus => out.s = true,
                    'i' if minus => out.sources_rcfile = true,
                    'o' | 'O' => i += 1,
                    _ => {}
                }
            }
        }
        out.operands = args[i.min(args.len())..].to_vec();
        out
    }

    /// With `-c`: its script, and what may run with it. A script that expands anything (`$…`)
    /// may run its own operands (`sh -c '"$@"' sh reboot`, `sh -c '$0' reboot`), so each run
    /// of them, from `$0` on, is checked as a command too. None without `-c`; empty without a
    /// script (`sh -c` alone).
    pub fn script(&self) -> Option<String> {
        if !self.c {
            return None;
        }
        let Some((script, rest)) = self.operands.split_first() else { return Some(String::new()) };
        if rest.is_empty() || !script.contains('$') {
            return Some(script.to_string());
        }
        let mut parts = vec![script.to_string()];
        for k in 0..rest.len() {
            parts.push(rest[k..].iter().map(|w| squote(w)).collect::<Vec<_>>().join(" "));
        }
        Some(parts.join("\n"))
    }
}

/// Programs `xargs` can feed any operands and still only read.
const XARGS_READERS: &[&str] = &["cat", "ls", "head", "tail", "wc", "stat", "grep", "du", "sha256sum"];

/// `xargs [opts] CMD ARGS…` behind the leading wrappers that don't change CMD's arguments: CMD
/// and its ARGS (empty when xargs runs echo). None when the segment isn't an xargs invocation.
fn xargs_command<'a>(words: &[&'a str]) -> Option<Vec<&'a str>> {
    let mut w: Vec<&'a str> = words.to_vec();
    loop {
        let &first = w.first()?;
        if is_assignment(first) {
            w.remove(0);
            continue;
        }
        let name = first.rsplit('/').next().unwrap_or(first);
        w = match name {
            "xargs" => return past_options(&w[1..], &XARGS).map(|(_, rest)| rest),
            "sudo" | "doas" | "pkexec" | "run0" => skip_options(&w[1..], &["-u", "-g", "-p", "-C", "-h", "-U", "-r", "-t", "-D", "--user"]),
            "env" => past_options(&w[1..], &ENV)?.1.into_iter().filter(|a| !is_assignment(a)).collect(),
            "setsid" => skip_options(&w[1..], &[]),
            "timeout" => skip_options(&w[1..], &["-s", "-k", "--signal", "--kill-after"]).into_iter().skip(1).collect(),
            _ => match wrapped(name, &w[1..])? {
                Wrapped::Runs(rest) => rest,
                Wrapped::Shows | Wrapped::Sets => return None,
            },
        };
    }
}

/// `xargs CMD ARGS…` run against the words it's fed: `CMD ARGS <fed words>`, when SparkBox can
/// read the input (echo/printf, or a here-string). None when the segment isn't an xargs, it runs
/// echo (no command), or the input can't be read — then `CMD ARGS` alone is judged elsewhere.
fn xargs_job(seg: &Segment, prev: Option<&Segment>) -> Option<String> {
    let words: Vec<&str> = seg.words.iter().map(String::as_str).collect();
    let cmd = xargs_command(&words)?;
    if cmd.is_empty() {
        return None;
    }
    let fed = fed_text(seg, prev)?;
    let mut parts: Vec<String> = cmd.iter().map(|w| squote(w)).collect();
    parts.extend(fed.split_whitespace().map(squote));
    Some(parts.join(" "))
}

/// A root shell with no command of its own, which reads stdin: `sudo -s|-i` (no command after the
/// options), or `su` with no `-c` (bare, `su -`, `su -l`, `su <user>`).
fn root_shell_reads_stdin(words: &[&str]) -> bool {
    let w: Vec<&str> = words.iter().copied().skip_while(|a| is_assignment(a)).collect();
    let Some((&first, rest)) = w.split_first() else { return false };
    match first.rsplit('/').next().unwrap_or(first) {
        "sudo" | "doas" | "pkexec" | "run0" => {
            let shell = rest
                .iter()
                .take_while(|a| a.starts_with('-'))
                .any(|a| matches!(*a, "--login" | "--shell") || (!a.starts_with("--") && a[1..].chars().take_while(|c| !"ugpChUrtD".contains(*c)).any(|c| matches!(c, 'i' | 's'))));
            let command = !skip_options(rest, &["-u", "-g", "-p", "-C", "-h", "-U", "-r", "-t", "-D", "--user"]).is_empty();
            shell && !command
        }
        "su" => !rest.iter().any(|a| *a == "-c" || *a == "--command") && rest.iter().filter(|a| !a.starts_with('-') && **a != "-").count() <= 1,
        _ => false,
    }
}

/// `systemd-run` options that take a value, so it isn't mistaken for the command.
const SYSTEMD_RUN_VALUE: &[&str] = &[
    "--unit", "-u", "-p", "--property", "-E", "--setenv", "--uid", "--gid", "--nice", "--working-directory", "-M", "--machine",
    "-H", "--host", "-C", "--capsule", "--service-type", "--slice", "--description", "--on-active", "--on-calendar", "--on-boot",
    "--on-startup", "--on-unit-active", "--on-unit-inactive", "--timer-property", "--path-property", "--socket-property",
    "--background", "--json", "--expand-environment",
];

/// What `at`/`batch` will run later.
enum AtJob {
    /// The job's text: a here-string, or what `echo`/`printf` pipe in.
    Known(String),
    /// From a file (`-f`), a here-document or any other producer: SparkBox can't read it.
    Unknown(&'static str),
}

/// The job `at`/`batch` schedules (under sudo or the other wrappers too), or None when the
/// segment isn't one scheduling a job (`at -l` lists, `-c` prints, `-r`/`-d` remove). `prev` is
/// the segment before, which feeds it when `seg.piped`.
fn at_job(seg: &Segment, prev: Option<&Segment>) -> Option<AtJob> {
    let words: Vec<&str> = seg.words.iter().map(String::as_str).collect();
    let Core::Words(w) = core(&words).0 else { return None };
    let (&first, raw) = w.split_first()?;
    let name = match first.rsplit('/').next().unwrap_or(first) {
        "at" => "at",
        "batch" => "batch",
        _ => return None,
    };
    let args = getopt(name, raw);
    if args.iter().any(|a| matches!(a.as_str(), "-l" | "-c" | "-r" | "-d")) || args == ["-V"] {
        return None;
    }
    if args.iter().any(|a| a == "-f") {
        return Some(AtJob::Unknown(name));
    }
    // A here-string, or `echo 'shutdown -h now' | at now`, `printf 'reboot\n' | batch`.
    Some(fed_text(seg, prev).map_or(AtJob::Unknown(name), AtJob::Known))
}

/// The power a unit takes the node to when started: the power targets, and the services
/// behind them. Bus calls name it in full (`reboot.target`); systemctl adds `.service` to a
/// bare name, so `systemctl start systemd-reboot` counts too.
fn power_unit(unit: &str) -> Option<Power> {
    match unit.trim_matches(['\'', '"']) {
        "reboot.target" | "kexec.target" | "soft-reboot.target" | "ctrl-alt-del.target" | "systemd-reboot.service" | "systemd-kexec.service" | "systemd-soft-reboot.service" => Some(Power::Restart),
        "poweroff.target" | "halt.target" | "exit.target" | "systemd-poweroff.service" | "systemd-halt.service" | "systemd-exit.service" => Some(Power::Off),
        _ => None,
    }
}

/// systemctl verbs that start the units they name.
const STARTS_UNITS: &[&str] = &["start", "restart", "try-restart", "reload-or-restart", "try-reload-or-restart", "condrestart", "force-reload", "isolate"];

/// The power target or service `systemctl start|restart|isolate` (or `enable --now`) starts,
/// with the power it takes. The user manager (`--user`) has none.
fn systemctl_power_unit<'a>(args: &[&'a str]) -> Option<(&'a str, Power)> {
    let ops = operands(args);
    let (verb, units) = ops.split_first()?;
    let starts = STARTS_UNITS.contains(verb) || (matches!(*verb, "enable" | "reenable") && args.contains(&"--now"));
    if !starts || args.contains(&"--user") {
        return None;
    }
    units.iter().find_map(|u| power_unit(if u.contains('.') { u.to_string() } else { format!("{u}.service") }.as_str()).map(|p| (*u, p)))
}

/// A power method called over `busctl call`, `gdbus call` or `dbus-send`: logind's or systemd's
/// Reboot, PowerOff, Halt, KExec or SoftReboot (bare, `…Manager.PowerOff`, or a newer form
/// like RebootWithFlags), ScheduleShutdown of a power type, and starting a power target or its
/// service (StartUnit reboot.target, or Start on the unit's own object). The power it takes.
fn bus_power(name: &str, args: &[&str]) -> Option<Power> {
    if !matches!(name, "busctl" | "gdbus" | "dbus-send") || (name != "dbus-send" && !args.contains(&"call")) {
        return None;
    }
    // dbus-send types its arguments (string:reboot.target); gdbus quotes them ('reboot.target').
    let values: Vec<&str> = args.iter().map(|a| a.strip_prefix("string:").or_else(|| a.strip_prefix("objpath:")).unwrap_or(a).trim_matches(['\'', '"'])).collect();
    for a in args {
        let m = a.rsplit('.').next().unwrap_or(a);
        if m.is_empty() || !m.chars().all(|c| c.is_ascii_alphanumeric()) {
            continue;
        }
        if ["Reboot", "KExec", "SoftReboot"].iter().any(|p| m.starts_with(p)) {
            return Some(Power::Restart);
        }
        if ["PowerOff", "Halt"].iter().any(|p| m.starts_with(p)) {
            return Some(Power::Off);
        }
        let power = match m {
            "ScheduleShutdown" => values.iter().find_map(|v| match *v {
                "poweroff" | "halt" => Some(Power::Off),
                "reboot" | "kexec" | "soft-reboot" => Some(Power::Restart),
                _ => None,
            }),
            "StartUnit" | "StartUnitReplace" | "StartUnitWithFlags" | "RestartUnit" | "TryRestartUnit" | "ReloadOrRestartUnit" | "ReloadOrTryRestartUnit" | "EnqueueUnitJob" => {
                values.iter().find_map(|v| power_unit(v))
            }
            // The unit's own object: /org/freedesktop/systemd1/unit/reboot_2etarget.
            "Start" | "Restart" | "TryRestart" | "ReloadOrRestart" | "ReloadOrTryRestart" => {
                values.iter().find_map(|v| v.strip_prefix("/org/freedesktop/systemd1/unit/").and_then(|u| power_unit(&unescape_bus_path(u))))
            }
            _ => None,
        };
        if power.is_some() {
            return power;
        }
    }
    None
}

/// systemd's object-path escaping undone: `_2e` is `.`, `_2d` is `-`.
fn unescape_bus_path(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    let mut rest = p;
    while let Some(at) = rest.find('_') {
        out.push_str(&rest[..at]);
        match rest.get(at + 1..at + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) {
            Some(b) => {
                out.push(b as char);
                rest = &rest[at + 3..];
            }
            None => {
                out.push('_');
                rest = &rest[at + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// /dev/sda, /dev/nvme0n1p2, /dev/mapper/x, /dev/disk/by-id/… — not /dev/null or /dev/zero.
fn is_block_device(path: &str) -> bool {
    const DISKS: &[&str] = &["sd", "nvme", "vd", "xvd", "hd", "mmcblk", "md", "dm-", "mapper/", "disk/", "loop", "nbd", "rbd", "zd"];
    path.strip_prefix("/dev/").is_some_and(|rest| DISKS.iter().any(|d| rest.starts_with(d) && rest.len() > d.len()))
}

/// /, a home (~, ~user, $HOME, /home/<user>) and the system folders, with or without a
/// trailing / or /*.
fn is_critical_path(path: &str) -> bool {
    const CRITICAL: &[&str] = &["/", "~", "$HOME", "${HOME}", "/home", "/etc", "/usr", "/var", "/boot", "/root", "/bin", "/sbin", "/lib", "/lib64", "/opt", "/srv", "/dev", "/proc", "/sys", "/run"];
    let trimmed = path.trim().trim_end_matches(['*', '/']);
    let normal = if trimmed.starts_with('/') { normal_path(trimmed) } else { trimmed.to_string() };
    if normal.strip_prefix("/home/").is_some_and(|u| !u.is_empty() && !u.contains('/')) {
        return true;
    }
    if normal.strip_prefix('~').is_some_and(|u| !u.is_empty() && !u.contains('/') && u.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))) {
        return true;
    }
    let mut p = path.trim();
    while p.len() > 1 && (p.ends_with("/*") || p.ends_with('/') || p.ends_with("/.")) {
        p = p.trim_end_matches('*').trim_end_matches('.').trim_end_matches('/');
        if p.is_empty() {
            p = "/";
        }
    }
    if p == "/*" || p == "/." {
        p = "/";
    }
    CRITICAL.contains(&p)
}

/// Where the node keeps what takes hours to get back: Docker's and containerd's data, the
/// model folders and the Hugging Face cache (the folder itself or anything in it).
fn is_data_dir(path: &str) -> bool {
    let p = normal_path(path.trim().trim_end_matches('*'));
    let p = p.trim_end_matches('/');
    // The path inside a home folder: ~/x, $HOME/x, /root/x or /home/<user>/x.
    let home_rel = ["~/", "$HOME/", "${HOME}/", "/root/"]
        .iter()
        .find_map(|h| p.strip_prefix(h))
        .or_else(|| p.strip_prefix("/home/").and_then(|r| r.split_once('/')).map(|(_, r)| r));
    let under = |base: &str, x: &str| x == base || x.starts_with(&format!("{base}/"));
    // Docker's own data is all or nothing; a model folder or the cache is one thing, one model
    // in it is an ordinary change.
    ["/var/lib/docker", "/var/lib/containerd"].iter().any(|b| under(b, p))
        || p == "/opt/models"
        || home_rel.is_some_and(|r| matches!(r, "models" | ".cache/huggingface" | ".cache/huggingface/hub" | ".cache"))
        || matches!(p, "$HF_HOME" | "${HF_HOME}" | "$HF_HOME/hub" | "${HF_HOME}/hub")
}

fn data_dir_what(path: &str) -> &'static str {
    if path.contains("docker") || path.contains("containerd") {
        "every container, image and volume on the node"
    } else {
        "model weights that take hours to download again"
    }
}

/// `-r`, `-R`, `--recursive`, or short flags that include r or R (`-rf`, `-fR`).
fn has_recursive(args: &[&str]) -> bool {
    args.iter().any(|a| *a == "--recursive" || (a.starts_with('-') && !a.starts_with("--") && a.contains(['r', 'R'])))
}

fn operands<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut opts = true;
    for a in args {
        if opts && *a == "--" {
            opts = false;
        } else if !opts || !a.starts_with('-') || *a == "-" {
            out.push(*a);
        }
    }
    out
}

/// What makes a segment destructive (and whether it cuts access), or None.
fn severe(seg: &Segment, depth: usize) -> Option<Verdict> {
    if let Some(t) = seg.targets.iter().find(|t| is_block_device(t)) {
        return Some(Verdict::destructive(format!("Writes straight to the disk device {t}.")));
    }
    if seg.targets.iter().any(|t| normal_path(t) == "/proc/sysrq-trigger") {
        return Some(Verdict::cuts(SYSRQ.into()));
    }
    if let Some(t) = seg.targets.iter().find(|t| is_auth_file(t)) {
        return Some(Verdict::cuts(auth_file_reason(t)));
    }
    let words: Vec<&str> = seg.words.iter().map(String::as_str).collect();
    let (core, _) = core(&words);
    let w = match core {
        Core::Script(script) => {
            let v = verdict_at(&script, depth + 1);
            return (v.kind == Kind::Destructive).then_some(v);
        }
        Core::Words(w) => w,
        Core::Unread(_) => return None,
    };
    let (&first, raw) = w.split_first()?;
    let name = first.rsplit('/').next().unwrap_or(first);
    // `sort -o/etc/passwd` names /etc/passwd as plainly as `sort -o /etc/passwd`.
    let normal = getopt(name, raw);
    let args: &[&str] = &normal.iter().map(String::as_str).collect::<Vec<_>>();
    if let Some(why) = cuts_access(name, args, seg) {
        return Some(Verdict::cuts(why));
    }
    // A builtin that re-evaluates its operands runs a `$(…)` or backtick in one, quoted or not
    // (`test -v 'a[$(reboot)]'`): what that runs counts.
    if reevaluates(name, raw) {
        for text in raw.iter().flat_map(|a| quoted_substitutions(a)) {
            let v = verdict_at(&text, depth + 1);
            if v.kind == Kind::Destructive {
                return Some(v);
            }
        }
    }
    // python3 -c "os.system('reboot')", perl -e, awk 'BEGIN { system("…") }': the shell
    // commands inside are judged by the same rules.
    if let Some(v) = embedded_code(name, args).and_then(|code| embedded(&code, depth)) {
        return Some(v);
    }
    // `find … -exec CMD … ;` runs CMD on what it finds, so a power verb or destructive command
    // after -exec counts; find's own mass-delete check (below) still catches -delete and -exec rm.
    if name == "find" {
        if let Some(cmd) = find_exec(args) {
            let joined = cmd.iter().map(|w| squote(w)).collect::<Vec<_>>().join(" ");
            let v = verdict_at(&joined, depth + 1);
            if v.kind == Kind::Destructive {
                return Some(v);
            }
        }
    }
    destructive(name, args).map(|why| Verdict { power: powers_down(name, args), ..Verdict::destructive(why) })
}

/// The program text an interpreter is given on its command line: `-c`, `-e`, `-E`, `-r`
/// (PHP), `--eval`, or an awk program that calls system().
fn embedded_code(name: &str, args: &[&str]) -> Option<String> {
    let flag_value = |flags: &[&str]| args.iter().position(|a| flags.contains(a)).and_then(|i| args.get(i + 1)).map(|s| s.to_string());
    let name = name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    match name {
        "python" | "pypy" => flag_value(&["-c"]),
        "perl" | "ruby" | "node" | "nodejs" | "deno" | "lua" | "luajit" => flag_value(&["-e", "-E", "--eval", "-p"]),
        "php" => flag_value(&["-r"]),
        "awk" | "gawk" | "mawk" | "nawk" => {
            let program = skip_options(args, &["-v", "--assign", "-F", "--field-separator"]).first().map(|s| s.to_string())?;
            program.contains("system").then_some(program)
        }
        _ => None,
    }
}

/// The most severe verdict for the shell commands quoted inside some program text: each string
/// literal, and all of them joined (`['systemctl', 'stop', 'ssh']`). Only a destructive one
/// counts; a fragment that doesn't parse as a command is ignored, not fatal.
fn embedded(code: &str, depth: usize) -> Option<Verdict> {
    let lits = literals(code);
    let mut candidates = lits.clone();
    if lits.len() > 1 {
        candidates.push(lits.join(" "));
    }
    candidates
        .iter()
        .map(|c| verdict_at(c, depth + 1))
        .filter(|v| v.kind == Kind::Destructive && !unfollowed(v))
        .max_by_key(|v| v.cuts_access)
}

/// The contents of every '…', "…" and `…` string in some program text.
fn literals(code: &str) -> Vec<String> {
    let c: Vec<char> = code.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let q = c[i];
        if matches!(q, '\'' | '"' | '`') {
            let mut lit = String::new();
            i += 1;
            while i < c.len() && c[i] != q {
                if c[i] == '\\' && i + 1 < c.len() {
                    i += 1;
                }
                lit.push(c[i]);
                i += 1;
            }
            if !lit.trim().is_empty() {
                out.push(lit);
            }
        }
        i += 1;
    }
    out
}

/// Why a command can destroy data or take the node down, or None.
fn destructive(name: &str, args: &[&str]) -> Option<String> {
    let has = |flags: &[&str]| args.iter().any(|a| flags.contains(a));
    let device = || operands(args).into_iter().chain(args.iter().copied()).find(|a| is_block_device(a.split_once('=').map_or(a, |(_, v)| v)));
    match name {
        "dd" => args.iter().find_map(|a| a.strip_prefix("of=").filter(|t| is_block_device(t))).map(|t| format!("Writes straight to the disk device {t} (dd).")),
        // A new filesystem erases whatever it's made on, device or image file.
        n if n.starts_with("mkfs") || n == "mke2fs" => Some(match device() {
            Some(d) => format!("Erases the disk device {d} ({n})."),
            None => format!("Makes a new filesystem ({n}), erasing what was there."),
        }),
        n if matches!(n, "mkswap" | "blkdiscard" | "shred" | "truncate") => device().map(|d| format!("Erases the disk device {d} ({n}).")),
        "wipefs" if !has(&["-n", "--no-act"]) => device().map(|d| format!("Erases the signatures on {d} (wipefs).")),
        "fdisk" | "sfdisk" | "cfdisk" | "gdisk" | "sgdisk" | "parted" => {
            if partition_listing(name, args) {
                return None;
            }
            device().map(|d| format!("Rewrites the partition table of {d} ({name})."))
        }
        "find" => {
            let roots: Vec<&str> = args.iter().copied().take_while(|a| !a.starts_with('-') && *a != "(" && *a != "!").collect();
            let deletes = has(&["-delete"]) || args.windows(2).any(|p| matches!(p[0], "-exec" | "-execdir") && p[1].rsplit('/').next() == Some("rm"));
            roots.iter().find(|r| is_critical_path(r)).filter(|_| deletes).map(|r| format!("Deletes files under {r} (find -delete)."))
        }
        "shutdown" if !has(&["-c", "--help", "--show"]) => Some("Shuts the node down (shutdown).".into()),
        "reboot" | "poweroff" | "halt" if !has(&["--help"]) => Some(format!("Takes the node down ({name}).")),
        "init" | "telinit" if args.iter().any(|a| matches!(*a, "0" | "6")) => Some(format!("Takes the node down ({name} {}).", args.join(" "))),
        // logind's and systemd's power methods over the bus take the node down as surely as the verbs.
        "busctl" | "gdbus" | "dbus-send" => bus_power(name, args).map(|p| format!("Takes the node down ({name} {}).", if p == Power::Restart { "Reboot" } else { "PowerOff" })),
        // Reached only with -H (`core` peels it otherwise): the command runs on another machine.
        "systemd-run" => Some("Runs a command on another machine (systemd-run -H): SparkBox can't check what it does there.".into()),
        // Resetting a GPU is destructive: it tears down whatever is running on it.
        "nvidia-smi" if args.iter().any(|a| matches!(*a, "-r" | "--gpu-reset")) => Some("Resets the GPU (nvidia-smi -r): it stops whatever is using it.".into()),
        // Ending a user's login sessions kills their processes — SparkBox's own session included.
        "loginctl" if args.iter().any(|a| matches!(*a, "terminate-user" | "kill-user")) => {
            Some("Ends a user's login sessions (loginctl): their processes stop, SparkBox's connection with them.".into())
        }
        "systemctl" => {
            let verb = args.iter().find(|a| !a.starts_with('-'))?;
            // Starting a power target or its service (`systemctl start reboot.target`) is the verb
            // itself; `systemctl exit` on the system manager is poweroff.
            if let Some((unit, _)) = systemctl_power_unit(args) {
                return Some(format!("Takes the node down (systemctl {verb} {unit})."));
            }
            (matches!(*verb, "poweroff" | "reboot" | "halt" | "kexec" | "soft-reboot" | "suspend" | "hibernate" | "hybrid-sleep" | "suspend-then-hibernate")
                || (*verb == "exit" && !args.contains(&"--user")))
                .then(|| format!("Takes the node down (systemctl {verb})."))
        }
        "kill" => {
            let mut i = 0;
            let mut after_signal = false;
            while i < args.len() {
                let a = args[i];
                match a {
                    "-s" | "-n" | "--signal" => {
                        i += 2;
                        after_signal = true;
                        continue;
                    }
                    // `1`, or `01`: kill reads a PID as a number.
                    a if a.bytes().all(|b| b.is_ascii_digit()) && a.trim_start_matches('0') == "1" => return Some("Kills init (PID 1), which takes the node down.".into()),
                    "-1" if after_signal || i > 0 => return Some("Kills every process on the node (kill -1).".into()),
                    _ => after_signal = a.starts_with('-'),
                }
                i += 1;
            }
            None
        }
        "killall5" => Some("Kills every process on the node (killall5).".into()),
        "pkill" | "killall" | "slay" if getopt(name, args).iter().any(|a| a == "-u" || a.starts_with("--euid") || a.starts_with("--uid") || a == "--user") => {
            Some(format!("Kills every process of a user ({name} -u): their shells, jobs and SparkBox's own connection."))
        }
        "crontab" if getopt(name, args).iter().any(|a| a == "-r" || a == "--remove") => Some("Deletes the whole crontab (crontab -r), with no copy kept.".into()),
        "rm" if has_recursive(args) => operands(args).into_iter().find(|t| is_data_dir(t)).map(|t| format!("Deletes {t} and everything in it (rm -r): {}.", data_dir_what(t))),
        "docker" => {
            let rest = skip_options(args, DOCKER_GLOBAL_VALUE);
            let (verb, more) = rest.split_first()?;
            let sub = more.iter().find(|a| !a.starts_with('-')).copied();
            // `docker rm -f $(docker ps -aq)`, or `… | xargs docker rm -f`: every container, running ones too.
            let force_all = |words: &[&str]| words.iter().any(|a| matches!(*a, "-f" | "--force") || (a.starts_with('-') && !a.starts_with("--") && a.contains('f'))) && {
                let ops: Vec<&&str> = words.iter().filter(|a| !a.starts_with('-')).collect();
                ops.is_empty() || ops.iter().any(|a| a.contains(SUB))
            };
            match (*verb, sub) {
                ("system", Some("prune")) => Some("Deletes every Docker container, image and network not in use (docker system prune).".into()),
                ("volume", Some("rm" | "remove" | "prune")) => Some("Deletes Docker volumes and the data in them.".into()),
                ("container", Some("prune")) => Some("Deletes every stopped container and what's in it (docker container prune).".into()),
                ("image", Some("prune")) if more.iter().any(|a| matches!(*a, "-a" | "--all")) => Some("Deletes every image no container uses (docker image prune -a).".into()),
                ("rm", _) if force_all(more) => Some("Removes every container on the node, running ones too (docker rm -f $(docker ps -aq)).".into()),
                ("container", Some("rm" | "remove")) if force_all(&more[1..]) => Some("Removes every container on the node, running ones too (docker container rm -f).".into()),
                ("rmi", _) if force_all(more) => Some("Deletes every image on the node (docker rmi -f).".into()),
                _ => None,
            }
        }
        // ACCEPT policies and `ufw default allow` open the node up; DROP ones cut access (cuts_access).
        "iptables" | "ip6tables" | "iptables-nft" | "iptables-legacy" | "ip6tables-nft" | "ip6tables-legacy" if getopt("iptables", args).iter().any(|a| a == "-P" || a == "--policy") => {
            Some(format!("Changes a firewall chain's default policy ({name} -P): it decides what reaches the node."))
        }
        "ufw" if args.contains(&"default") || args.contains(&"disable") => Some(format!("Changes the firewall's policy (ufw {}).", args.join(" "))),
        "tee" => args.iter().find(|a| is_block_device(a)).map(|d| format!("Writes straight to the disk device {d} (tee).")),
        _ => None,
    }
}

/// Restarts or switches the node off (Fleet treats it as cutting access on several nodes at once).
fn powers_down(name: &str, args: &[&str]) -> Option<Power> {
    let restart = |yes: bool| Some(if yes { Power::Restart } else { Power::Off });
    match name {
        "shutdown" if !args.iter().any(|a| matches!(*a, "-c" | "--help" | "--show")) => restart(args.iter().any(|a| *a == "-r" || *a == "--reboot" || (a.starts_with('-') && !a.starts_with("--") && a.contains('r')))),
        "reboot" if !args.contains(&"--help") => restart(!args.iter().any(|a| matches!(*a, "-p" | "--poweroff"))),
        "poweroff" | "halt" if !args.contains(&"--help") => restart(args.contains(&"--reboot")),
        "init" | "telinit" if args.iter().any(|a| matches!(*a, "0" | "6")) => restart(args.contains(&"6")),
        "systemctl" if systemctl_power_unit(args).is_some() => systemctl_power_unit(args).map(|(_, p)| p),
        "systemctl" => match args.iter().find(|a| !a.starts_with('-')).copied() {
            Some("reboot" | "kexec" | "soft-reboot") => restart(true),
            Some("poweroff" | "halt") => restart(false),
            Some("exit") if !args.contains(&"--user") => restart(false),
            _ => None,
        },
        "busctl" | "gdbus" | "dbus-send" => bus_power(name, args),
        _ => None,
    }
}

// ---- Commands that may cut SparkBox's access

const SYSRQ: &str = "Writes to /proc/sysrq-trigger, which can crash, reboot or freeze the node at once.";
/// SSH and the services that keep the node on the network.
const ACCESS_UNITS: &[&str] = &["ssh", "sshd", "ssh.socket", "sshd.socket", "networking", "NetworkManager", "systemd-networkd", "systemd-resolved", "wpa_supplicant", "tailscaled"];

/// `/etc//ssh/./sshd_config/` → `/etc/ssh/sshd_config`.
fn normal_path(p: &str) -> String {
    let p = p.trim_matches(|c| c == '"' || c == '\'');
    let mut parts: Vec<&str> = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            x => parts.push(x),
        }
    }
    if p.starts_with('/') { format!("/{}", parts.join("/")) } else { parts.join("/") }
}

/// The files logins and sudo depend on: /etc/passwd, shadow, group, gshadow, sudoers (and
/// sudoers.d), fstab, everything in /etc/ssh, and the SSH keys SparkBox logs in with: any
/// .ssh folder (~/.ssh, /root/.ssh, /home/<user>/.ssh) and any authorized_keys file.
fn is_auth_file(p: &str) -> bool {
    let p = normal_path(p.split_once('=').map_or(p, |(_, v)| v));
    matches!(p.as_str(), "/etc/passwd" | "/etc/shadow" | "/etc/group" | "/etc/gshadow" | "/etc/sudoers" | "/etc/fstab" | "/etc/sudoers.d" | "/etc/ssh")
        || p.starts_with("/etc/sudoers.d/")
        || p.starts_with("/etc/ssh/")
        || is_ssh_keys(&p)
}

/// ~/.ssh and the like, and authorized_keys wherever it is.
fn is_ssh_keys(p: &str) -> bool {
    let p = p.trim_end_matches('/');
    p.rsplit('/').next().is_some_and(|f| f.starts_with("authorized_keys"))
        || p.split('/').any(|part| part == ".ssh")
}

fn auth_file_reason(p: &str) -> String {
    let p = normal_path(p.split_once('=').map_or(p, |(_, v)| v));
    if is_ssh_keys(&p) {
        format!("Changes {p}: SparkBox and you log in with the keys there, so nobody may get back in.")
    } else if p.starts_with("/etc/ssh") {
        format!("Changes {p}: a mistake there stops SSH logins, SparkBox's included.")
    } else if p == "/etc/fstab" {
        "Changes /etc/fstab: a mistake there can stop the node booting.".into()
    } else {
        format!("Changes {p}: a mistake there can stop logins or sudo for everyone.")
    }
}

fn is_access_unit(u: &str) -> bool {
    let u = u.trim_end_matches(".service");
    ACCESS_UNITS.contains(&u) || u.starts_with("ssh@") || u.starts_with("sshd@")
}

/// Why a command may leave SparkBox unable to log in to the node again, or None.
fn cuts_access(name: &str, args: &[&str], seg: &Segment) -> Option<String> {
    let has = |flags: &[&str]| args.iter().any(|a| flags.iter().any(|f| a == f || (f.starts_with("--") && a.starts_with(&format!("{f}=")))));
    let ops = operands(args);
    // Writing to the files logins depend on, by any command that writes (cp and the like: their
    // destination only; mv: either end).
    if !segment_read_only(seg) && !matches!(name, "cat" | "grep" | "head" | "tail" | "less" | "diff") {
        let mentioned: Vec<&str> = match name {
            "cp" | "install" | "rsync" | "scp" | "ln" => ops.last().copied().into_iter().collect(),
            _ => args.to_vec(),
        };
        if let Some(f) = mentioned.iter().find(|a| is_auth_file(a)) {
            return Some(auth_file_reason(f));
        }
        if args.iter().any(|a| normal_path(a.split_once('=').map_or(a, |(_, v)| v)) == "/proc/sysrq-trigger") {
            return Some(SYSRQ.into());
        }
    }
    let verb = ops.first().copied().unwrap_or("");
    match name {
        "systemctl" => {
            let verb = args.iter().find(|a| !a.starts_with('-')).copied().unwrap_or("");
            let units: Vec<&str> = ops.iter().skip(1).copied().collect();
            // Isolating a power target is a power verb (`destructive` names it), not a lockout.
            if matches!(verb, "rescue" | "emergency" | "isolate") && systemctl_power_unit(args).is_none() {
                return Some(format!("Switches the node to another target (systemctl {verb}), which stops SSH and the network."));
            }
            (matches!(verb, "stop" | "disable" | "mask" | "kill") && units.iter().any(|u| is_access_unit(u)))
                .then(|| format!("Stops {} (systemctl {verb}): SparkBox and you may not get back in until it's started at the node's console.", units.iter().find(|u| is_access_unit(u)).unwrap()))
        }
        "service" | "invoke-rc.d" => (is_access_unit(verb) && ops.get(1).is_some_and(|a| matches!(*a, "stop" | "disable"))).then(|| format!("Stops {verb} ({name} {verb} stop): SparkBox may not get back in.")),
        "update-rc.d" => (is_access_unit(verb) && ops.get(1).is_some_and(|a| matches!(*a, "disable" | "remove"))).then(|| format!("Turns {verb} off at boot (update-rc.d).")),
        n if (n == "ssh" || n == "sshd") && seg.words.iter().any(|w| w.contains("/init.d/")) => args.contains(&"stop").then(|| format!("Stops {n} (init.d): SparkBox may not get back in.")),
        "killall" | "pkill" => ops.iter().any(|a| a.trim_end_matches(':').ends_with("sshd") || *a == "ssh").then(|| format!("Kills the SSH server ({name}): SparkBox may not get back in.")),
        "apt" | "apt-get" | "aptitude" => {
            let v = args.iter().find(|a| !a.starts_with('-')).copied().unwrap_or("");
            (matches!(v, "remove" | "purge" | "autoremove") && ops.iter().skip(1).any(|p| matches!(p.split(':').next().unwrap_or(p), "openssh-server" | "ssh" | "openssh-server*")))
                .then(|| format!("Removes the SSH server ({name} {v} openssh-server): nobody can log in over the network afterwards."))
        }
        "dpkg" => (has(&["-r", "--remove", "-P", "--purge"]) && ops.iter().any(|p| matches!(*p, "openssh-server" | "ssh")))
            .then(|| "Removes the SSH server (dpkg): nobody can log in over the network afterwards.".into()),
        "ip" => {
            let rest = ip_command(args);
            let object = rest.first().copied().unwrap_or("");
            let action = rest.get(1).copied().unwrap_or("");
            let link = "link".starts_with(object) && !object.is_empty();
            let addr = !object.is_empty() && ("address".starts_with(object) || object == "addr");
            let route = !object.is_empty() && "route".starts_with(object);
            if link && (rest.contains(&"down") || matches!(action, "del" | "delete")) {
                Some("Takes a network interface down (ip link … down): the node may drop off the network, and SparkBox with it.".into())
            } else if (addr || route) && matches!(action, "flush" | "del" | "delete" | "replace" | "change") {
                Some(format!("Removes network addresses or routes (ip {object} {action}): the node may drop off the network."))
            } else {
                None
            }
        }
        // netplan apply reconfigures every interface; the node may drop off the network.
        "netplan" => (verb == "apply" || verb == "try").then(|| format!("Reconfigures the network (netplan {verb}): the node may drop off the network.")),
        "networkctl" => (verb == "down" || verb == "delete").then(|| format!("Takes a link down (networkctl {verb}): the node may drop off the network.")),
        // Dropping packets on the LAN, or admin-downing/unloading the fabric, can cut SparkBox off.
        "tc" if args.contains(&"netem") && args.contains(&"loss") && args.iter().any(|a| matches!(*a, "add" | "change" | "replace")) => {
            Some("Drops traffic with netem (tc … netem loss): packets to the node, SSH included, may stop getting through.".into())
        }
        // -a is --port_state: `-a DN`, `-aDN`, `--port_state DN`, `--port_state=DN`, any case.
        "mlxlink"
            if args.windows(2).any(|p| matches!(p[0], "-a" | "--amber" | "--port_state") && p[1].eq_ignore_ascii_case("dn"))
                || args.iter().any(|a| a.eq_ignore_ascii_case("-adn") || a.eq_ignore_ascii_case("--port_state=dn")) =>
        {
            Some("Takes a fabric port down (mlxlink -a DN): the node may drop off the fabric.".into())
        }
        // modprobe's -r in a cluster too (`-rv`), before any letter that takes a value.
        "rmmod" | "modprobe"
            if ops.iter().any(|m| m.trim_end_matches(".ko").starts_with("mlx5"))
                && (name == "rmmod"
                    || args.iter().any(|a| *a == "--remove" || (a.starts_with('-') && !a.starts_with("--") && a[1..].chars().take_while(|c| !"CdSo".contains(*c)).any(|c| c == 'r')))) =>
        {
            Some(format!("Unloads the fabric driver ({name} mlx5_core): the node drops off the fabric."))
        }
        "ifdown" => Some("Takes a network interface down (ifdown): the node may drop off the network.".into()),
        "ifconfig" => args.contains(&"down").then(|| "Takes a network interface down (ifconfig … down): the node may drop off the network.".into()),
        "nmcli" => {
            let words: Vec<&str> = nmcli_command(args).iter().filter(|a| !a.starts_with('-')).copied().collect();
            let obj = words.first().copied().unwrap_or("");
            let act = words.get(1).copied().unwrap_or("");
            let off = (!obj.is_empty() && "networking".starts_with(obj) && matches!(act, "off" | "on"))
                || (!obj.is_empty() && "connection".starts_with(obj) && matches!(act, "up" | "down" | "delete" | "del"))
                || (!obj.is_empty() && "device".starts_with(obj) && matches!(act, "disconnect" | "down" | "delete" | "connect"))
                || (!obj.is_empty() && "radio".starts_with(obj) && words.contains(&"off"));
            off.then(|| format!("Takes the network down (nmcli {obj} {act}): the node may drop off the network."))
        }
        "iptables" | "ip6tables" | "iptables-nft" | "iptables-legacy" | "ip6tables-nft" | "ip6tables-legacy" => {
            let drop = |a: &&str| matches!(*a, "DROP" | "REJECT");
            if has(&["-F", "--flush", "-X", "--delete-chain"]) {
                Some(format!("Flushes the firewall ({name} -F): with a DROP policy that locks everyone out, SparkBox included."))
            } else if has(&["-P", "--policy"]) && args.iter().any(drop) {
                Some(format!("Sets a firewall chain to drop everything ({name} -P … DROP): SSH may stop getting through."))
            } else if has(&["-A", "--append", "-I", "--insert"]) && args.iter().any(|a| matches!(*a, "INPUT" | "OUTPUT")) && args.iter().any(drop) {
                Some(format!("Adds a firewall rule that drops traffic ({name}): SSH may stop getting through."))
            } else {
                None
            }
        }
        "iptables-restore" | "ip6tables-restore" | "iptables-nft-restore" => Some(format!("Replaces the whole firewall ({name}): SSH may stop getting through.")),
        "nft" => (has(&["-f", "--file"]) || ops.first().is_some_and(|v| matches!(*v, "flush" | "delete" | "destroy")))
            .then(|| "Replaces or flushes the firewall (nft): SSH may stop getting through.".into()),
        "ufw" => {
            let words: Vec<&str> = args.iter().filter(|a| !a.starts_with('-')).copied().collect();
            let bad = match words.as_slice() {
                ["enable", ..] | ["reset", ..] => true,
                ["default", "deny" | "reject", ..] => true,
                ["deny" | "reject", rest @ ..] => rest.iter().any(|r| matches!(*r, "22" | "22/tcp" | "ssh" | "OpenSSH")),
                _ => false,
            };
            bad.then(|| format!("Changes the firewall (ufw {}): SSH may stop getting through.", words.join(" ")))
        }
        "firewall-cmd" => args.iter().any(|a| a.starts_with("--panic-on") || a.contains("--remove-service=ssh") || a.starts_with("--set-default-zone=drop") || a.starts_with("--set-default-zone=block"))
            .then(|| "Changes the firewall (firewall-cmd): SSH may stop getting through.".into()),
        "passwd" => Some("Changes a password or locks an account (passwd): SparkBox's saved password may stop working.".into()),
        "chpasswd" => Some("Sets passwords (chpasswd): SparkBox's saved password may stop working.".into()),
        "chage" if !has(&["-l", "--list"]) => Some("Changes when a password or account expires (chage): logins may stop working.".into()),
        "usermod" if has(&["-L", "--lock", "-e", "--expiredate", "-p", "--password"]) => Some("Locks an account or changes its password (usermod): SparkBox's login may stop working.".into()),
        "userdel" | "deluser" => Some(format!("Deletes a user account ({name}): if it's the one SparkBox logs in as, it can't get back in.")),
        "vipw" | "vigr" => Some(format!("Edits the user database ({name}): a mistake stops logins.")),
        "visudo" if !has(&["-c", "--check"]) => Some("Edits sudoers (visudo): a mistake stops sudo for everyone.".into()),
        "rm" => {
            if has(&["--no-preserve-root"]) {
                return Some("Deletes everything from / down (rm --no-preserve-root).".into());
            }
            ops.iter().find(|t| is_critical_path(t)).filter(|_| has_recursive(args)).map(|t| format!("Deletes {t} and everything in it (rm -r): the node stops working."))
        }
        // The sources (everything but the destination) of a move.
        "mv" => ops.iter().take(ops.len().saturating_sub(1)).find(|t| is_critical_path(t)).map(|t| format!("Moves {t} (mv): the node stops working, and SparkBox can't log in.")),
        "chmod" | "chown" | "chgrp" if has_recursive(args) => {
            ops.iter().skip(1).find(|t| is_critical_path(t)).map(|t| format!("Changes the owner or permissions of every file under {t} ({name} -R): logins and sudo may stop working."))
        }
        // Even without -R: sshd refuses logins when / or a home is writable by others, and
        // sudo when its files change owner.
        "chmod" | "chown" | "chgrp" => ops.iter().skip(1).find(|t| is_critical_path(t)).map(|t| format!("Changes the owner or permissions of {t} ({name}): SSH logins and sudo may stop working.")),
        _ => None,
    }
}

/// `fdisk -l`, `sfdisk --dump`, `parted -l`, `parted /dev/x print`, `sgdisk -p` only look. Every
/// option must be a listing one for sfdisk and sgdisk: sfdisk's last action wins, so `sfdisk -l
/// --delete x` deletes. fdisk -l opens the disk read-only, whatever else it's given. parted runs each
/// word after the device and takes the start of any of them (`print mkl gpt`), so print stands alone.
fn partition_listing(name: &str, args: &[&str]) -> bool {
    let only = |flags: &[&str]| {
        let opts: Vec<&&str> = args.iter().filter(|a| a.starts_with('-')).collect();
        !opts.is_empty() && opts.iter().all(|a| flags.contains(a))
    };
    match name {
        "fdisk" => args.iter().any(|a| matches!(*a, "-l" | "--list")),
        "sfdisk" => only(&["-l", "--list", "-d", "--dump", "-J", "--json", "-s", "--show-size", "-V", "--verify"]),
        "parted" => {
            let shown = past_exact_options(args, &["-s", "--script", "-m", "--machine", "-sm", "-ms"], &[]);
            matches!(args, ["-l" | "--list"]) || matches!(shown, Some([_, "print"] | [_, "print", "free"]))
        }
        "sgdisk" => only(&["-p", "--print", "-i", "--info", "-v", "--verify", "-O", "--print-mbr"]),
        _ => false,
    }
}

/// `:(){ :|:& };:` and named variants: a function that pipes into itself in the background.
fn fork_bomb(cmd: &str) -> Option<String> {
    let compact: String = cmd.chars().filter(|c| !c.is_whitespace()).collect();
    let compact = compact.replace("function", "");
    let mut from = 0;
    while let Some(i) = compact[from..].find("(){") {
        let at = from + i;
        from = at + 3;
        let name: String = compact[..at].chars().rev().take_while(|c| c.is_alphanumeric() || matches!(c, '_' | ':' | '.' | '-')).collect::<Vec<_>>().into_iter().rev().collect();
        if name.is_empty() {
            continue;
        }
        let body = &compact[at + 3..];
        if body.contains(&format!("{name}|{name}&")) || body.contains(&format!("{name}&{name}")) {
            return Some(name);
        }
    }
    None
}

/// The rounds' command literals, byte for byte as the round plans' Contracts give them (round 3's
/// C1–C9; round 4's C-S, C-SH, C-R, C1′, C2′ and C6′; P2 round 1's C-L2, C-U2, C-P2 and C-J), for
/// the classifier's and the exit's tests.
#[cfg(test)]
pub(crate) mod contracts {
    #[test]
    fn current_quick_checks_and_literal_imports_are_reads_but_image_execution_is_not() {
        for cmd in ["python3 ~/.sparkbox/sparkjob.py download-tools", "python3 ~/.sparkbox/sparkjob.py check '~/models' 'vllm:tag' quick", "python3 ~/.sparkbox/sparkjob.py check '~/models' 'vllm:tag' quick --instance dev", "python3 ~/.sparkbox/sparkjob.py recipe-import '~/glm53'"] {
            assert!(super::read_only(cmd), "{cmd}");
            assert!(!super::escalates(cmd));
        }
        for cmd in ["python3 ~/.sparkbox/sparkjob.py check '~/models' 'vllm:tag'", "python3 ~/.sparkbox/sparkjob.py recipe-import '~/glm53'; touch /tmp/x", "python3 /tmp/sparkjob.py recipe-import '~/glm53'"] {
            assert!(!super::read_only(cmd), "{cmd}");
        }
    }
    /// C1: upgrade one package in a detached unit (sudo).
    pub const C1_ONE: &str = r#"mkdir -p '/var/log/sparkbox' && chmod 0755 '/var/log/sparkbox' && dpkg-query -W > '/var/log/sparkbox/dpkg-before-t1.txt' && systemd-run --unit sparkbox-upgrade-t1 --collect -p StandardOutput=append:/var/log/sparkbox/upgrade-t1.log -p StandardError=append:/var/log/sparkbox/upgrade-t1.log /bin/sh -c 'trap : TERM; env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l NEEDRESTART_SUSPEND=1 apt-get -o DPkg::Lock::Timeout=300 -o APT::Keep-Downloaded-Packages=true -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold -y install --only-upgrade '\''libfoo'\''; code=$?; echo SPARKBOX-AFTER; apt-get check; needrestart -b -r l; echo SPARKBOX-AFTER-END; echo "SPARKBOX-EXIT $code"; exit $code'"#;

    /// C1, "everything": dist-upgrade.
    pub fn c1_everything() -> String {
        C1_ONE.replace(r#"-y install --only-upgrade '\''libfoo'\''"#, "-y dist-upgrade")
    }

    /// C1 with `dl_limit_kbps: Some(500)`.
    pub fn c1_dl_limit() -> String {
        C1_ONE.replace("-o Dpkg::Options::=--force-confold", "-o Dpkg::Options::=--force-confold -o Acquire::http::Dl-Limit=500 -o Acquire::https::Dl-Limit=500")
    }

    /// C1 for a log folder other than /var/log/sparkbox (`Live::upgrade_log_dir` in tests).
    pub fn c1_in(dir: &str, everything: bool) -> String {
        let c = if everything { c1_everything() } else { C1_ONE.to_string() };
        c.replace("/var/log/sparkbox", dir)
    }

    /// C2: the dry runs (no sudo).
    pub const C2_ONE: &str = "apt-get -s -o DPkg::Lock::Timeout=300 -o APT::Keep-Downloaded-Packages=true -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold -y install --only-upgrade 'libfoo'";
    pub const C2_EVERYTHING: &str = "apt-get -s -o DPkg::Lock::Timeout=300 -o APT::Keep-Downloaded-Packages=true -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold -y dist-upgrade";

    /// C2 with the download limit.
    pub fn c2_dl_limit() -> String {
        C2_ONE.replace("-o Dpkg::Options::=--force-confold", "-o Dpkg::Options::=--force-confold -o Acquire::http::Dl-Limit=500 -o Acquire::https::Dl-Limit=500")
    }

    /// C3: the package-list refresh (sudo).
    pub const C3: &str = "apt-get -o DPkg::Lock::Timeout=300 update";

    /// C4: holds (sudo), and the GPU-stack preset's read.
    pub const C4_HOLD: &str = "apt-mark hold 'a' 'b'";
    pub const C4_UNHOLD: &str = "apt-mark unhold 'a'";
    pub const C4_PRESET_READ: &str = r"dpkg-query -W -f='${Package}\n'";

    /// C5: the update check, its stamps, and the sudo firmware read.
    pub fn check_script() -> String {
        format!(
            "{}{}",
            crate::ops::updates::STATUS_SCRIPT,
            r#"; echo '--- dgx'; grep -E '^DGX_(SWBUILD_VERSION|OTA_VERSION|OTA_DATE)=' /etc/dgx-release 2>/dev/null; echo '--- fwupd'; fwupdmgr get-updates --json --no-unreported-check --no-metadata-check </dev/null 2>&1; echo "fwupd-exit=$?"; echo '--- snap'; snap refresh --list 2>&1; echo '--- ollama'; ollama --version 2>/dev/null; echo '--- docker'; docker info --format '{{.LiveRestoreEnabled}}' 2>/dev/null; echo '--- stamps'; stat -c '%n %Y' /var/lib/apt/periodic/update-success-stamp /var/lib/dpkg/status /var/run/reboot-required 2>/dev/null; true"#
        )
    }
    pub const STAMPS: &str = "stat -c '%n %Y' /var/lib/apt/periodic/update-success-stamp /var/lib/dpkg/status /var/run/reboot-required 2>/dev/null; true";
    pub const C5_SUDO_FWUPD: &str = "fwupdmgr get-updates --json --no-unreported-check --no-metadata-check";

    /// C6: which kernel boots next, then its checks (K = 7.0.0-1019-nvidia): C6′, the compute
    /// package's status with its version and a closing `; true`.
    pub const C6_KERNEL: &str = "readlink -f /boot/vmlinuz";
    pub const C6_STEP_B: &str = r#"echo '--- initrd'; test -s /boot/initrd.img-7.0.0-1019-nvidia && echo present; echo '--- modinfo'; modinfo -k 7.0.0-1019-nvidia -F version nvidia 2>&1; echo '--- compute'; dpkg-query -W -f='${db:Status-Abbrev}${Version}\n' libnvidia-compute-580 2>&1; echo '--- dkms'; dpkg -l 'nvidia-dkms-*' 2>/dev/null | grep -E '^[uihrp][iUFHWt]'; echo '--- modules'; dpkg -l 'linux-modules-nvidia-580-open-*' 2>/dev/null | grep -E '^[uihrp][iUFHWt]'; echo '--- kho'; grep -o kho=off /proc/cmdline; dpkg -l nvidia-spark-grub-kho 2>/dev/null | grep -E '^[uihrp][iUFHWt]'; echo '--- grub'; grep GRUB_DEFAULT /etc/default/grub; true"#;

    /// C7: the test container, for this build's instance.
    pub fn c7(instance: &str) -> String {
        format!("docker run -d --name sparkbox-test-busybox --label sparkbox.test=1 --label sparkbox.instance={instance} busybox:1.36 sleep 3600")
    }

    /// The round plan's `sh -c` rows (b), each with its power verb (the last one powers off).
    pub const SH_C_POWER: [&str; 12] = [
        "sh -c -e reboot",
        "bash -c -- reboot",
        "sh -e -c reboot",
        "sh -ec reboot",
        "sh -ce reboot",
        "bash -c -o errexit reboot",
        "bash +e -c reboot",
        "sh -c reboot x",
        "/bin/sh -c -- 'systemctl reboot'",
        "sudo sh -c -e reboot",
        "env LC_ALL=C sh -c -e reboot",
        "timeout 5s sh -c -e 'shutdown -h now'",
    ];

    /// C-L2: a rank's `docker run`, here a one-node load with its key (deployment d1, instance
    /// release, cell2, gemma-3-27b-it, its recipe, image id sha256:a…a, port 8000, 32768 tokens).
    /// One command, no preamble: its options are in the deploy folder (C-F).
    pub const C_L: &str = r#"docker run -d --name sparkbox-d1 --label sparkbox.instance=release --label sparkbox.deployment=d1 --label sparkbox.group=d1 --label sparkbox.role=single --label sparkbox.rank=0 --label sparkbox.model=gemma-3-27b-it --label sparkbox.recipe=gemma-3-27b-it --label sparkbox.image=vllm/vllm-openai:glm53-flash-arm64-cu130 --label sparkbox.path=/home/sparkuser/models/google/gemma-3-27b-it --label sparkbox.parallel=1x1 --label sparkbox.gpu-share=0.75 --label sparkbox.port=8000 --label sparkbox.listen=192.168.50.154 --label sparkbox.maxlen=32768 --network host --ipc host --gpus all --ulimit memlock=-1 --ulimit stack=67108864 --ulimit nofile=1048576:1048576 --restart no -v /home/sparkuser/models/google/gemma-3-27b-it:/home/sparkuser/models/google/gemma-3-27b-it:ro -v "$HOME/.cache/vllm:/root/.cache/vllm" -v "$HOME/.cache/flashinfer:/root/.cache/flashinfer" -v "$HOME/.triton:/root/.triton" -v "$HOME/.sparkbox/deploy/d1:/run/sparkbox/deploy:ro" -v "$HOME/.sparkbox/launch-12b1d2f4c5b5565459585085c89dab0171be5d22c0618d3f7a219b871733ddac.py:/run/sparkbox/launch.py:ro" -v "$HOME/.sparkbox/keys/d1:/run/sparkbox/key:ro" --entrypoint python3 sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa /run/sparkbox/launch.py --key"#;

    /// C-L2 on one node without a key (`require_key: false`).
    pub fn c_l_no_key() -> String {
        C_L.replace(r#" -v "$HOME/.sparkbox/keys/d1:/run/sparkbox/key:ro""#, "").replace(" /run/sparkbox/launch.py --key", " /run/sparkbox/launch.py")
    }

    /// C-L2 for the head of the same load on two nodes, with `docker_access: "privileged"`.
    pub const C_L_HEAD: &str = r#"docker run -d --name sparkbox-d1-r0 --label sparkbox.instance=release --label sparkbox.deployment=d1 --label sparkbox.group=d1 --label sparkbox.role=head --label sparkbox.rank=0 --label sparkbox.model=gemma-3-27b-it --label sparkbox.recipe=gemma-3-27b-it --label sparkbox.image=vllm/vllm-openai:glm53-flash-arm64-cu130 --label sparkbox.path=/home/sparkuser/models/google/gemma-3-27b-it --label sparkbox.parallel=2x1 --label sparkbox.gpu-share=0.75 --label sparkbox.port=8000 --label sparkbox.listen=192.168.50.154 --label sparkbox.maxlen=32768 --network host --ipc host --gpus all --ulimit memlock=-1 --ulimit stack=67108864 --ulimit nofile=1048576:1048576 --privileged --restart no -v /home/sparkuser/models/google/gemma-3-27b-it:/home/sparkuser/models/google/gemma-3-27b-it:ro -v "$HOME/.cache/vllm:/root/.cache/vllm" -v "$HOME/.cache/flashinfer:/root/.cache/flashinfer" -v "$HOME/.triton:/root/.triton" -v "$HOME/.sparkbox/deploy/d1:/run/sparkbox/deploy:ro" -v "$HOME/.sparkbox/launch-12b1d2f4c5b5565459585085c89dab0171be5d22c0618d3f7a219b871733ddac.py:/run/sparkbox/launch.py:ro" -v "$HOME/.sparkbox/keys/d1:/run/sparkbox/key:ro" --entrypoint python3 sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa /run/sparkbox/launch.py --key"#;

    /// C-L2 for its worker, rank 1: no port, no key.
    pub const C_L_WORKER: &str = r#"docker run -d --name sparkbox-d1-r1 --label sparkbox.instance=release --label sparkbox.deployment=d1 --label sparkbox.group=d1 --label sparkbox.role=worker --label sparkbox.rank=1 --label sparkbox.model=gemma-3-27b-it --label sparkbox.recipe=gemma-3-27b-it --label sparkbox.image=vllm/vllm-openai:glm53-flash-arm64-cu130 --label sparkbox.path=/home/sparkuser/models/google/gemma-3-27b-it --label sparkbox.parallel=2x1 --label sparkbox.gpu-share=0.75 --label sparkbox.maxlen=32768 --network host --ipc host --gpus all --ulimit memlock=-1 --ulimit stack=67108864 --ulimit nofile=1048576:1048576 --privileged --restart no -v /home/sparkuser/models/google/gemma-3-27b-it:/home/sparkuser/models/google/gemma-3-27b-it:ro -v "$HOME/.cache/vllm:/root/.cache/vllm" -v "$HOME/.cache/flashinfer:/root/.cache/flashinfer" -v "$HOME/.triton:/root/.triton" -v "$HOME/.sparkbox/deploy/d1:/run/sparkbox/deploy:ro" -v "$HOME/.sparkbox/launch-12b1d2f4c5b5565459585085c89dab0171be5d22c0618d3f7a219b871733ddac.py:/run/sparkbox/launch.py:ro" --entrypoint python3 sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa /run/sparkbox/launch.py"#;

    /// C-U2: the unload; C-S: stopping one role.
    pub const C_U: &str = r#"ids=$(docker ps -aq --filter label=sparkbox.instance=release --filter label=sparkbox.deployment=d1); [ -z "$ids" ] || docker stop -t 30 $ids >/dev/null; [ -z "$ids" ] || docker rm $ids >/dev/null; rm -rf ~/.sparkbox/keys/d1 ~/.sparkbox/deploy/d1; echo ok"#;
    pub const C_S: &str = r#"ids=$(docker ps -aq --filter label=sparkbox.instance=release --filter label=sparkbox.deployment=d1 --filter label=sparkbox.role=head); [ -z "$ids" ] || docker stop -t 30 $ids >/dev/null; [ -z "$ids" ] || docker rm $ids >/dev/null; echo ok"#;

    /// C-P2: the pre-flight read of a load on two nodes.
    pub const C_P: &str = r#"echo '--- mem'; grep -E '^(MemTotal|MemFree|MemAvailable|Cached):' /proc/meminfo; echo '--- earlyoom'; systemctl is-active earlyoom; cat /etc/default/earlyoom 2>&1; echo '--- image'; docker image inspect -f '{{.Id}}' 'vllm/vllm-openai:glm53-flash-arm64-cu130' 2>&1; echo '--- store'; docker info -f '{{.Driver}} {{json .DriverStatus}}' 2>&1; echo '--- model'; test -f '/home/sparkuser/models/google/gemma-3-27b-it/config.json' && test ! -e '/home/sparkuser/models/google/gemma-3-27b-it/.sparkbox.partial' && echo present; echo '--- port'; ss -Hltn '( sport = :8000 or sport = :29501 )'; echo '--- shm'; ls -la /dev/shm; echo '--- caches'; ls -ld ~/.cache/vllm ~/.cache/flashinfer ~/.triton 2>&1; echo '--- containers'; docker ps -a --filter label=sparkbox.deployment --format '{{.ID}}|{{.Names}}|{{.Label "sparkbox.deployment"}}|{{.Label "sparkbox.instance"}}|{{.Status}}' 2>&1; echo '--- ray'; docker ps -a --no-trunc --format '{{.ID}}|{{.Names}}|{{.Status}}|{{.Command}}' 2>&1 | grep -e 'ray start --address'; echo '--- ollama'; curl -s -m 3 http://127.0.0.1:11434/api/ps 2>&1; true"#;

    /// C-J: the jumbo path from a worker, one line per half (head 192.168.100.1 and .101.1).
    pub const C_J: &str = "ping -c 2 -W 2 -M do -s 8972 -I enp1s0f1np1 192.168.100.1 >/dev/null 2>&1 && echo half1 ok || echo half1 failed; ping -c 2 -W 2 -M do -s 8972 -I enP2p1s0f1np1 192.168.101.1 >/dev/null 2>&1 && echo half2 ok || echo half2 failed";

    /// C-SH: the shell facts (evidence.rs's `shell` step).
    pub const C_SH: &str = r#"ls -l /bin/sh /usr/bin/sh; readlink -f /bin/sh /usr/bin/sh; type sh; dpkg-query -W -f='${Package} ${Version}\n' dash bash; getent passwd "$USER""#;

    /// C-R: the models list and the protocol probe (reads), and the 17+25 check (inference).
    pub const R_MODELS: &str = r#"curl -s -m 10 -H @- -w '\n%{http_code}' http://192.168.50.154:8000/v1/models"#;
    pub const R_OPENAPI: &str = r#"curl -s -m 10 -w '\n%{http_code}' http://192.168.50.154:8000/openapi.json"#;
    pub const R_SANITY: &str = r#"curl -s -m 120 -H @- -H 'Content-Type: application/json' -d '{"model":"gemma-3-27b-it","messages":[{"role":"user","content":"What is 17+25? Reply with the number only."}],"max_tokens":8,"temperature":0,"chat_template_kwargs":{"enable_thinking":false}}' -w '\n%{http_code}' http://192.168.50.154:8000/v1/chat/completions"#;
    /// R-sanity again, only after a reply that ran out of tokens while the model was still thinking.
    pub const R_SANITY_THINKING: &str = r#"curl -s -m 300 -H @- -H 'Content-Type: application/json' -d '{"model":"gemma-3-27b-it","messages":[{"role":"user","content":"What is 17+25? Reply with the number only."}],"max_tokens":512,"temperature":0,"chat_template_kwargs":{"enable_thinking":false}}' -w '\n%{http_code}' http://192.168.50.154:8000/v1/chat/completions"#;

    /// C1′: the pinned upgrade (unit sparkbox-upgrade-t1, libfoo=1.2-3), and its dry run C2′.
    pub const C1_PINNED: &str = r#"mkdir -p '/var/log/sparkbox' && chmod 0755 '/var/log/sparkbox' && dpkg-query -W > '/var/log/sparkbox/dpkg-before-t1.txt' && systemd-run --unit sparkbox-upgrade-t1 --collect -p StandardOutput=append:/var/log/sparkbox/upgrade-t1.log -p StandardError=append:/var/log/sparkbox/upgrade-t1.log /bin/sh -c 'trap : TERM; env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l NEEDRESTART_SUSPEND=1 apt-get -o DPkg::Lock::Timeout=300 -o APT::Keep-Downloaded-Packages=true -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold -y --no-remove install --only-upgrade '\''libfoo=1.2-3'\''; code=$?; echo SPARKBOX-AFTER; apt-get check; needrestart -b -r l; echo SPARKBOX-AFTER-END; echo "SPARKBOX-EXIT $code"; exit $code'"#;
    pub const C2_PINNED: &str = "apt-get -s -o DPkg::Lock::Timeout=300 -o APT::Keep-Downloaded-Packages=true -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold -y --no-remove install --only-upgrade 'libfoo=1.2-3'";

    /// C1′ for a log folder other than /var/log/sparkbox, with `pins` instead of libfoo=1.2-3.
    pub fn c1_pinned_in(dir: &str, pins: &[&str]) -> String {
        let quoted: Vec<String> = pins.iter().map(|p| format!(r"'\''{p}'\''")).collect();
        C1_PINNED.replace(r"'\''libfoo=1.2-3'\''", &quoted.join(" ")).replace("/var/log/sparkbox", dir)
    }

    /// C8: Files › New file, as sparkjob.py's arguments (empty stdin).
    pub fn c8(path: &str) -> String {
        format!("commit {} --sha256 e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 --expect absent", crate::live::sh_quote(path))
    }
}
