//! How SparkBox logs in to a node: the parts of ~/.ssh/config it honours, and the login
//! settings from Settings › Cluster login.
//!
//! From ~/.ssh/config (and the files it Includes) a node's block can set HostName, User,
//! Port, IdentityFile, IdentitiesOnly and ProxyJump (one hop). As in ssh, the first value
//! found for an option wins and a block applies when one of its patterns matches the
//! node and none of its negated (!) patterns do. Match blocks aren't evaluated; they're
//! skipped with a note, as are options SparkBox has no use for.
//!
//! The order SparkBox tries: keys from an ssh-agent (SSH_AUTH_SOCK, or 1Password's or
//! Secretive's agent when chosen), then key files (IdentityFile, or the one chosen in
//! Settings, with its passphrase from the Keychain), then the cluster password last.
//! The password is still what sudo gets.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One `Host` (or `Match`) block: its patterns and options in file order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Block {
    pub patterns: Vec<String>,
    pub options: Vec<(String, String)>,
    /// A `Match` block: never applied.
    pub is_match: bool,
}

/// What ~/.ssh/config says about one host.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostConfig {
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_files: Vec<String>,
    pub identities_only: bool,
    /// `[user@]host[:port]` of the one jump host.
    pub proxy_jump: Option<String>,
    /// A `Host` block other than `Host *` matched (HostName, User and Port then win over
    /// SparkBox's own node settings, as when you type `ssh <host>`).
    pub explicit: bool,
    /// What was skipped, in plain words.
    pub notes: Vec<String>,
}

/// Where SparkBox looks for an ssh-agent.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentChoice {
    /// SSH_AUTH_SOCK, as Terminal's ssh uses it.
    #[default]
    System,
    OnePassword,
    Secretive,
    /// No agent: key files and the password only.
    Off,
}

/// Settings › Cluster login.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoginSettings {
    pub agent: AgentChoice,
    /// A key file to try for every node (besides IdentityFile from ~/.ssh/config).
    pub identity_file: Option<String>,
    /// Read ~/.ssh/config.
    pub use_ssh_config: bool,
    /// Fall back to the cluster password when no key is accepted.
    pub password_fallback: bool,
}

impl Default for LoginSettings {
    fn default() -> Self {
        LoginSettings { agent: AgentChoice::System, identity_file: None, use_ssh_config: true, password_fallback: true }
    }
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// 1Password's and Secretive's agent sockets on a Mac.
pub fn agent_socket(choice: &AgentChoice) -> Option<PathBuf> {
    match choice {
        AgentChoice::System => std::env::var_os("SSH_AUTH_SOCK").map(PathBuf::from).filter(|p| !p.as_os_str().is_empty()),
        AgentChoice::OnePassword => Some(home().join("Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock")),
        AgentChoice::Secretive => Some(home().join("Library/Containers/com.maxgoedjen.Secretive.SecretAgent/Data/socket.ssh")),
        AgentChoice::Off => None,
    }
}

/// "~/.ssh/id_ed25519" for a path under the home folder.
pub fn tilde(path: &str) -> String {
    let h = home().display().to_string();
    match path.strip_prefix(&h) {
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

/// `~/x` and `%d/x` to a full path (also `%u` local user, `%h` host, `%r` remote user, `%%`).
pub fn expand(path: &str, host: &str, remote_user: &str) -> String {
    let h = home().display().to_string();
    let local_user = std::env::var("USER").unwrap_or_default();
    let mut out = String::new();
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('d') => out.push_str(&h),
            Some('u') => out.push_str(&local_user),
            Some('h') => out.push_str(host),
            Some('r') => out.push_str(remote_user),
            Some('%') => out.push('%'),
            Some(o) => {
                out.push('%');
                out.push(o);
            }
            None => out.push('%'),
        }
    }
    match out.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{h}{rest}"),
        _ => out,
    }
}

/// Splits a config line into keyword and value (`Key value`, `Key=value`, quotes kept out).
fn split_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let cut = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let key = line[..cut].to_ascii_lowercase();
    let value = line[cut..].trim_start_matches(|c: char| c.is_whitespace() || c == '=').trim();
    let value = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(value);
    Some((key, value.to_string()))
}

/// Words of a value, honouring double quotes.
fn words(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in value.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// ssh's wildcard match: `*` any run, `?` one character.
pub fn glob(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.to_lowercase().chars().collect());
    let p: Vec<char> = p.iter().flat_map(|c| c.to_lowercase()).collect();
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn block_matches(b: &Block, alias: &str) -> bool {
    let mut hit = false;
    for p in &b.patterns {
        match p.strip_prefix('!') {
            Some(neg) if glob(neg, alias) => return false,
            Some(_) => {}
            None => hit |= glob(p, alias),
        }
    }
    hit
}

/// Parses config text; `Include` lines are followed with `include` (path, depth).
fn parse_into(text: &str, blocks: &mut Vec<Block>, depth: u32, include: &dyn Fn(&str) -> Vec<String>) {
    // Lines before the first Host apply to every host.
    if blocks.is_empty() {
        blocks.push(Block { patterns: vec!["*".into()], ..Default::default() });
    }
    for line in text.lines() {
        let Some((key, value)) = split_line(line) else { continue };
        match key.as_str() {
            "host" => blocks.push(Block { patterns: words(&value), ..Default::default() }),
            "match" => blocks.push(Block { patterns: vec![value], is_match: true, ..Default::default() }),
            "include" if depth < 8 => {
                for w in words(&value) {
                    for body in include(&w) {
                        // An Include inside a block stays in that block's scope until its own Host lines.
                        parse_into(&body, blocks, depth + 1, include);
                    }
                }
            }
            _ => blocks.last_mut().unwrap().options.push((key, value)),
        }
    }
}

/// Parses ~/.ssh/config text (no Includes). Tests only; the app uses `load`.
#[cfg(test)]
pub fn parse(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    parse_into(text, &mut blocks, 0, &|_| Vec::new());
    blocks
}

/// Reads a config file and everything it Includes (relative paths are under ~/.ssh; `*` globs
/// in the last part of a path are expanded, in name order).
pub fn load(path: &Path) -> Vec<Block> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let ssh_dir = path.parent().map(Path::to_path_buf).unwrap_or_else(|| home().join(".ssh"));
    let include = move |pattern: &str| -> Vec<String> {
        let full = expand(pattern, "", "");
        let full = if full.starts_with('/') { PathBuf::from(full) } else { ssh_dir.join(full) };
        let (dir, name) = (full.parent().map(Path::to_path_buf).unwrap_or_default(), full.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
        let mut files: Vec<PathBuf> = if name.contains('*') || name.contains('?') {
            std::fs::read_dir(&dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| glob(&name, &n.to_string_lossy()))).collect()).unwrap_or_default()
        } else {
            vec![full]
        };
        files.sort();
        files.iter().filter_map(|f| std::fs::read_to_string(f).ok()).collect()
    };
    let mut blocks = Vec::new();
    parse_into(&text, &mut blocks, 0, &include);
    blocks
}

/// The options that apply to `alias`, first value winning.
pub fn resolve(blocks: &[Block], alias: &str) -> HostConfig {
    let mut c = HostConfig::default();
    let mut only: Option<bool> = None;
    for b in blocks {
        if b.is_match {
            if !c.notes.iter().any(|n| n.starts_with("Match")) {
                c.notes.push("Match blocks in ~/.ssh/config are ignored.".into());
            }
            continue;
        }
        if !block_matches(b, alias) {
            continue;
        }
        if b.patterns.iter().any(|p| p != "*" && !p.starts_with('!')) {
            c.explicit = true;
        }
        for (k, v) in &b.options {
            match k.as_str() {
                "hostname" if c.hostname.is_none() => c.hostname = Some(v.replace("%h", alias)),
                "user" if c.user.is_none() => c.user = Some(v.clone()),
                "port" if c.port.is_none() => c.port = v.parse().ok(),
                "identityfile" => {
                    if !c.identity_files.contains(v) && !v.eq_ignore_ascii_case("none") {
                        c.identity_files.push(v.clone());
                    }
                }
                "identitiesonly" if only.is_none() => only = Some(v.eq_ignore_ascii_case("yes")),
                "proxyjump" if c.proxy_jump.is_none() => {
                    if v.eq_ignore_ascii_case("none") {
                        c.proxy_jump = Some(String::new());
                    } else {
                        let hops: Vec<&str> = v.split(',').collect();
                        if hops.len() > 1 {
                            c.notes.push(format!("ProxyJump {v}: SparkBox uses the first hop only."));
                        }
                        c.proxy_jump = Some(hops[0].trim().to_string());
                    }
                }
                _ => {}
            }
        }
    }
    c.proxy_jump = c.proxy_jump.filter(|j| !j.is_empty());
    c.identities_only = only.unwrap_or(false);
    c
}

/// `[user@]host[:port]` of a ProxyJump hop.
pub fn parse_jump(spec: &str) -> (Option<String>, String, Option<u16>) {
    let spec = spec.trim().trim_start_matches("ssh://");
    let (user, rest) = match spec.rsplit_once('@') {
        Some((u, r)) => (Some(u.to_string()), r),
        None => (None, spec),
    };
    match rest.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => (user, h.to_string(), p.parse().ok()),
        _ => (user, rest.to_string(), None),
    }
}

/// The login settings, from `path` (defaults when it's missing or unreadable).
pub fn load_settings(path: &Path) -> LoginSettings {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save_settings(path: &Path, s: &LoginSettings) -> Result<(), String> {
    crate::util::save_json(path, s)
}

/// The Keychain name of a key file's passphrase.
pub fn passphrase_account(path: &str) -> String {
    format!("ssh-key:{}", expand(path, "", ""))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "
# defaults first
User fallback
Host cell? !cell9
  HostName 192.168.50.5%h
  User sparkuser
  Port 2222
  IdentityFile ~/.ssh/id_cluster
  IdentitiesOnly yes
Host cell1
  HostName ignored-the-first-one-won
  IdentityFile %d/.ssh/second
Match host cell1 exec \"true\"
  User matched
Host gx10-*
  ProxyJump jump@cell1:2200,other
Host *
  IdentityFile ~/.ssh/id_ed25519
  Port 22
";

    #[test]
    fn options_come_from_matching_blocks_first_value_winning() {
        let blocks = parse(CONFIG);
        let c = resolve(&blocks, "cell1");
        assert_eq!(c.hostname.as_deref(), Some("192.168.50.5cell1"), "%h is the alias");
        assert_eq!(c.user.as_deref(), Some("fallback"), "a line before any Host applies to every host, and comes first");
        assert_eq!(c.port, Some(2222));
        assert_eq!(c.identity_files, ["~/.ssh/id_cluster", "%d/.ssh/second", "~/.ssh/id_ed25519"], "IdentityFile adds up");
        assert!(c.identities_only && c.explicit);
        assert_eq!(c.notes, ["Match blocks in ~/.ssh/config are ignored."]);
        assert!(!resolve(&blocks, "cell9").identities_only, "!cell9 excludes it");
        assert!(!resolve(&blocks, "cell9").explicit);

        let gx = resolve(&blocks, "gx10-0000");
        assert_eq!(gx.proxy_jump.as_deref(), Some("jump@cell1:2200"));
        assert!(gx.notes.iter().any(|n| n.contains("first hop only")), "{:?}", gx.notes);
        assert_eq!(gx.port, Some(22));
        assert_eq!(parse_jump("jump@cell1:2200"), (Some("jump".into()), "cell1".into(), Some(2200)));
        assert_eq!(parse_jump("cell1"), (None, "cell1".into(), None));
    }

    #[test]
    fn keywords_are_case_blind_and_take_equals_signs() {
        let c = resolve(&parse("HOST=spark\n  hostname=\"10.0.0.7\"\n  IDENTITYFILE ~/k\n"), "SPARK");
        assert_eq!(c.hostname.as_deref(), Some("10.0.0.7"));
        assert_eq!(c.identity_files, ["~/k"]);
        assert!(glob("cell*", "CELL3") && glob("c?ll1", "cell1") && !glob("cell", "cell1"));
    }

    #[test]
    fn includes_are_followed_relative_to_the_ssh_folder() {
        let dir = std::env::temp_dir().join(format!("sparkbox-sshcfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("conf.d")).unwrap();
        std::fs::write(dir.join("config"), "Include conf.d/*.conf\nHost *\n  User last\n").unwrap();
        std::fs::write(dir.join("conf.d/10-spark.conf"), "Host spark1\n  HostName 10.1.1.1\n  User fromincl\n").unwrap();
        std::fs::write(dir.join("conf.d/notes.txt"), "Host spark1\n  HostName wrong\n").unwrap();
        let c = resolve(&load(&dir.join("config")), "spark1");
        assert_eq!((c.hostname.as_deref(), c.user.as_deref()), (Some("10.1.1.1"), Some("fromincl")));
    }

    #[test]
    fn paths_expand_like_ssh() {
        let h = home().display().to_string();
        assert_eq!(expand("~/.ssh/id_%h", "cell1", "u"), format!("{h}/.ssh/id_cell1"));
        assert_eq!(expand("%d/k%%", "", ""), format!("{h}/k%"));
        assert_eq!(tilde(&format!("{h}/.ssh/id_ed25519")), "~/.ssh/id_ed25519");
    }
}

// ---- Settings › Cluster login (Tauri commands)

/// How SparkBox logs in to one node: what the last connection used, and what it would try.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeLogin {
    pub node: String,
    pub name: String,
    /// How the node's connection logged in (None: not connected).
    pub method: Option<crate::live::ssh::AuthMethod>,
    /// "~/.ssh/id_ed25519 via agent", "~/.ssh/id_ed25519", "password".
    pub detail: Option<String>,
    /// "cell1 · key ~/.ssh/id_ed25519 via agent".
    pub summary: String,
    /// The agent socket it would ask, and whether one is there.
    pub agent: Option<String>,
    pub agent_running: bool,
    /// Key files it would offer that exist.
    pub keys: Vec<String>,
    /// The cluster password is the last resort.
    pub password: bool,
    /// A ProxyJump hop from ~/.ssh/config.
    pub jump: Option<String>,
    pub notes: Vec<String>,
}

fn summary_of(name: &str, method: Option<crate::live::ssh::AuthMethod>, detail: Option<&str>) -> String {
    use crate::live::ssh::AuthMethod;
    match (method, detail) {
        (Some(AuthMethod::Agent), Some(d)) => format!("{name} · key {d}"),
        (Some(AuthMethod::Key), Some(d)) => format!("{name} · key {d}"),
        (Some(AuthMethod::Password), _) => format!("{name} · shared cluster password"),
        _ => format!("{name} · not connected"),
    }
}

/// The login settings (read-only).
#[tauri::command]
pub fn login_settings_get(state: tauri::State<'_, crate::AppState>) -> LoginSettings {
    state.live.login_settings()
}

/// Changes how SparkBox logs in (agent, key file, ~/.ssh/config, password fallback): Mac scope,
/// since it decides which of this Mac's keys are offered.
#[tauri::command]
pub fn login_settings_set(app: tauri::AppHandle, state: tauri::State<'_, crate::AppState>, settings: LoginSettings) -> Result<LoginSettings, String> {
    crate::admin::require_mac(&app)?;
    if let Some(f) = settings.identity_file.as_deref().filter(|f| !f.trim().is_empty()) {
        let full = expand(f, "", "");
        if !Path::new(&full).is_file() {
            return Err(format!("There's no key file at {f}."));
        }
    }
    state.live.set_login_settings(&settings)?;
    state.live.audit.record_app(
        "Cluster login changed",
        &format!(
            "agent {:?}, key file {}, ~/.ssh/config {}, password fallback {}",
            settings.agent,
            settings.identity_file.as_deref().unwrap_or("(none)"),
            if settings.use_ssh_config { "on" } else { "off" },
            if settings.password_fallback { "on" } else { "off" }
        ),
    );
    // New connections use them; the existing ones carry on until they reconnect.
    state.live.reconnect_all();
    Ok(state.live.login_settings())
}

/// Per node: how it logged in and what SparkBox would try (read-only, nothing is sent).
#[tauri::command]
pub fn login_methods(state: tauri::State<'_, crate::AppState>) -> Vec<NodeLogin> {
    use crate::live::ssh::AuthMethod;
    let inv = state.inventory.lock().unwrap().clone();
    if !crate::is_live(&state) {
        // Demo: like the owner's Sparks, every node logs in with the shared cluster password,
        // until key login is set up for it.
        return inv
            .nodes
            .iter()
            .map(|n| {
                let password_only = !DEMO_KEYED.lock().unwrap().contains(&n.id);
                let (method, detail) = if password_only { (AuthMethod::Password, "password") } else { (AuthMethod::Agent, "~/.ssh/id_ed25519 via agent") };
                NodeLogin {
                    node: n.id.clone(),
                    name: n.name.clone(),
                    method: Some(method),
                    detail: Some(detail.into()),
                    summary: summary_of(&n.name, Some(method), Some(detail)),
                    agent: Some("SSH_AUTH_SOCK".into()),
                    agent_running: true,
                    keys: vec!["~/.ssh/id_ed25519".into()],
                    password: true,
                    jump: None,
                    notes: Vec::new(),
                }
            })
            .collect();
    }
    let logins = state.live.logins();
    inv.nodes
        .iter()
        .map(|n| {
            let (agent, keys, password, notes, jump) = state.live.login_plan(n);
            let l = logins.get(&n.id);
            NodeLogin {
                node: n.id.clone(),
                name: n.name.clone(),
                method: l.map(|l| l.method),
                detail: l.map(|l| l.detail.clone()),
                summary: summary_of(&n.name, l.map(|l| l.method), l.map(|l| l.detail.as_str())),
                agent_running: agent.as_ref().is_some_and(|a| a.exists()),
                agent: agent.map(|a| tilde(&a.display().to_string())),
                keys,
                password,
                jump,
                notes,
            }
        })
        .collect()
}

/// Saves (or, with None, removes) a key file's passphrase in the Keychain. Mac scope.
#[tauri::command]
pub fn login_passphrase_set(app: tauri::AppHandle, state: tauri::State<'_, crate::AppState>, path: String, passphrase: Option<String>) -> Result<(), String> {
    crate::admin::require_mac_confirmed(&app, "login_passphrase_set")?;
    let account = passphrase_account(&path);
    match passphrase.filter(|p| !p.is_empty()) {
        Some(p) => crate::secrets::set(&account, &p)?,
        None => crate::secrets::clear(&account)?,
    }
    state.live.audit.record_app("Key passphrase saved in the Keychain", &tilde(&expand(&path, "", "")));
    state.live.reconnect_all();
    Ok(())
}

// ---- Set up key login

/// How setting up key login went on one node.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeySetup {
    pub node: String,
    /// The key is in ~/.ssh/authorized_keys and a login with it alone worked.
    pub verified: bool,
    /// Password login is now off on the node.
    pub password_off: bool,
    pub message: String,
}

/// The drop-in that turns password login off (a temporary folder in tests).
pub fn sshd_dropin() -> String {
    if cfg!(test) {
        format!("{}/sshd_config.d/00-sparkbox-keys-only.conf", std::env::var("SB_TEST_SSHD_DIR").unwrap_or_else(|_| "/nonexistent".into()))
    } else {
        "/etc/ssh/sshd_config.d/00-sparkbox-keys-only.conf".into()
    }
}

/// Adds `public` to the user's ~/.ssh/authorized_keys once (never twice, nothing else touched).
pub fn authorize_command(public: &str) -> String {
    let key = crate::live::sh_quote(public.trim());
    format!("umask 077 && mkdir -p ~/.ssh && touch ~/.ssh/authorized_keys && (grep -qxF {key} ~/.ssh/authorized_keys || printf '%s\\n' {key} >> ~/.ssh/authorized_keys)")
}

/// Turns password login off with a drop-in named to sort first (sshd keeps the first value it
/// reads, and Ubuntu's own 50-cloud-init.conf often says `PasswordAuthentication yes`), checked
/// by `sshd -t` before sshd reloads, and then by what sshd says it will do (`sshd -T`). Either
/// check failing removes the drop-in again and says why.
pub fn keys_only_command(dropin: &str) -> String {
    let f = crate::live::sh_quote(dropin);
    format!(
        "mkdir -p \"$(dirname {f})\" && printf 'PasswordAuthentication no\\nKbdInteractiveAuthentication no\\n' > {f} && \
         if ! sshd -t; then rm -f {f}; echo 'sshd rejected the change; nothing was changed' >&2; exit 1; fi && \
         if ! sshd -T 2>/dev/null | grep -qix 'passwordauthentication no'; then rm -f {f}; echo 'sshd still allows passwords (another setting wins); nothing was changed' >&2; exit 2; fi && \
         systemctl reload ssh"
    )
}

impl crate::live::Live {
    /// Adds the key to each node over the connection it has now, then logs in with that key
    /// alone to prove it works; only then (when asked) turns password login off there, and
    /// reports it off only when `sshd -T` says so.
    pub async fn setup_key_login(&self, nodes: &[crate::inventory::NodeConfig], key_path: &str, disable_password: bool) -> Vec<KeySetup> {
        let full = expand(key_path, "", "");
        let public = match std::fs::read_to_string(format!("{full}.pub")) {
            Ok(p) => p.trim().to_string(),
            Err(e) => {
                return nodes.iter().map(|n| KeySetup { node: n.id.clone(), verified: false, password_off: false, message: format!("Couldn't read {}.pub: {e}", tilde(&full)) }).collect();
            }
        };
        let mut out = Vec::new();
        for cfg in nodes {
            let mut r = KeySetup { node: cfg.id.clone(), verified: false, password_off: false, message: String::new() };
            match self.run(&cfg.id, &authorize_command(&public), None, std::time::Duration::from_secs(20)).await {
                Ok(o) if o.code == Some(0) => {}
                Ok(o) => {
                    r.message = o.failure("Adding the key", &cfg.name).unwrap_or_default();
                    out.push(r);
                    continue;
                }
                Err(e) => {
                    r.message = e;
                    out.push(r);
                    continue;
                }
            }
            match self.connect_with_key(cfg, &full).await {
                Ok(()) => r.verified = true,
                Err(e) => {
                    r.message = format!("The key was added, but logging in with it failed: {e}");
                    out.push(r);
                    continue;
                }
            }
            if disable_password {
                match self.sudo(&cfg.id, &keys_only_command(&sshd_dropin()), std::time::Duration::from_secs(30)).await {
                    Ok(o) if o.code == Some(0) => r.password_off = true,
                    Ok(o) => r.message = o.failure("Turning password login off", &cfg.name).unwrap_or_default(),
                    Err(e) => r.message = e,
                }
            }
            if r.message.is_empty() {
                r.message = if r.password_off { "Key login works; password login is off".into() } else { "Key login works".into() };
            }
            out.push(r);
        }
        out
    }
}

/// Settings › Cluster login › Set up key login: adds this Mac's key to each node, proves it
/// works, and (only when asked, and only where it worked) turns password login off.
#[tauri::command]
pub async fn login_setup_key(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    nodes: Vec<String>,
    key_path: String,
    generate: bool,
    disable_password: bool,
) -> Result<Vec<KeySetup>, String> {
    crate::admin::require(&app)?;
    let full = expand(&key_path, "", "");
    if generate && !Path::new(&full).exists() && crate::is_live(&state) {
        // A new key on this Mac: Mac scope too.
        crate::admin::require_mac(&app)?;
        let ok = std::process::Command::new("/usr/bin/ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", "sparkbox", "-f", &full])
            .status()
            .map_err(|e| format!("Couldn't run ssh-keygen: {e}"))?;
        if !ok.success() {
            return Err(format!("ssh-keygen couldn't create {}.", tilde(&full)));
        }
        state.live.audit.record_app("SSH key created", &format!("{} (ed25519, no passphrase)", tilde(&full)));
    }
    let inv = state.inventory.lock().unwrap().clone();
    let chosen: Vec<crate::inventory::NodeConfig> = inv.nodes.into_iter().filter(|n| nodes.contains(&n.id)).collect();
    if chosen.is_empty() {
        return Err("Pick at least one node.".into());
    }
    if !crate::is_live(&state) {
        return Ok(chosen
            .iter()
            .map(|n| {
                state.live.audit.record_simulated(&n.id, &format!("Set up key login on {}", n.name), &authorize_command(&format!("ssh-ed25519 AAAA… {}", tilde(&key_path))));
                if disable_password {
                    state.live.audit.record_simulated(&n.id, &format!("Turn password login off on {}", n.name), &format!("sudo {}", keys_only_command(&sshd_dropin())));
                }
                DEMO_KEYED.lock().unwrap().insert(n.id.clone());
                KeySetup { node: n.id.clone(), verified: true, password_off: disable_password, message: "Key login works".into() }
            })
            .collect());
    }
    let results = state.live.setup_key_login(&chosen, &key_path, disable_password).await;
    let mut settings = state.live.login_settings();
    if settings.identity_file.is_none() && results.iter().any(|r| r.verified) {
        settings.identity_file = Some(tilde(&expand(&key_path, "", "")));
        state.live.set_login_settings(&settings)?;
    }
    Ok(results)
}

/// Demo nodes whose key login was set up this session.
pub(crate) static DEMO_KEYED: std::sync::Mutex<std::collections::BTreeSet<String>> = std::sync::Mutex::new(std::collections::BTreeSet::new());
