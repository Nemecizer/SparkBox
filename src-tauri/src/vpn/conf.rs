//! Reads WireGuard and AmneziaWG `.conf` files (as exported by the Firewalla app).

use std::collections::BTreeMap;

/// AmneziaWG settings the tunnel engine understands (AmneziaWG 1.0 and 1.5).
const AWG_KEYS: &[&str] = &[
    "jc", "jmin", "jmax", "s1", "s2", "h1", "h2", "h3", "h4", "i1", "i2", "i3", "i4", "i5", "j1", "j2",
    "j3", "itime",
];
/// AmneziaWG 2.0 additions the engine can't handle yet.
const AWG2_KEYS: &[&str] = &["s3", "s4"];

#[derive(Debug, Default, PartialEq)]
pub struct ParsedConf {
    pub private_key: String,
    pub addresses: Vec<String>,
    pub dns: Vec<String>,
    pub mtu: Option<u32>,
    /// Lower-case AmneziaWG keys; empty for plain WireGuard.
    pub obfuscation: BTreeMap<String, String>,
    pub peer_public_key: String,
    pub preshared_key: Option<String>,
    pub endpoint: Option<String>,
    pub allowed_ips: Vec<String>,
    pub keepalive: Option<u32>,
}

impl ParsedConf {
    pub fn protocol(&self) -> &'static str {
        if self.obfuscation.is_empty() { "wireguard" } else { "amneziawg" }
    }
}

fn list(v: &str) -> Vec<String> {
    v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

pub fn parse(text: &str) -> Result<ParsedConf, String> {
    #[derive(PartialEq)]
    enum Section {
        None,
        Interface,
        Peer,
    }
    let mut section = Section::None;
    let mut peers = 0;
    let mut c = ParsedConf::default();

    for (n, raw) in text.lines().enumerate() {
        let line = raw.split(['#', ';']).next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            section = match line.to_ascii_lowercase().as_str() {
                "[interface]" => Section::Interface,
                "[peer]" => {
                    peers += 1;
                    Section::Peer
                }
                other => return Err(format!("line {}: unknown section {other}", n + 1)),
            };
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            return Err(format!("line {}: expected `Key = Value`", n + 1));
        };
        // Base64 keys end in '=', so only the first '=' separates key from value.
        let key = k.trim().to_ascii_lowercase();
        let value = v.trim().to_string();

        match section {
            Section::Interface => match key.as_str() {
                "privatekey" => c.private_key = value,
                "address" => c.addresses.extend(list(&value)),
                // DNS may also list search domains; the tunnel only needs server IPs.
                "dns" => c.dns.extend(list(&value).into_iter().filter(|d| d.parse::<std::net::IpAddr>().is_ok())),
                "mtu" => c.mtu = Some(value.parse().map_err(|_| format!("line {}: bad MTU", n + 1))?),
                k if AWG2_KEYS.contains(&k) => {
                    return Err("This profile uses AmneziaWG 2.0 settings (S3/S4), which SparkBox's tunnel engine doesn't support yet. Firewalla's own profiles use AmneziaWG 1.5.".into())
                }
                k if AWG_KEYS.contains(&k) => {
                    if k.starts_with('h') && value.contains('-') {
                        return Err("This profile uses AmneziaWG 2.0 header ranges (H1–H4 as a range), which SparkBox's tunnel engine doesn't support yet.".into());
                    }
                    c.obfuscation.insert(k.to_string(), value);
                }
                // ListenPort, Table, PostUp/PreDown etc. only matter to system VPN tools.
                _ => {}
            },
            Section::Peer if peers == 1 => match key.as_str() {
                "publickey" => c.peer_public_key = value,
                "presharedkey" => c.preshared_key = Some(value),
                "endpoint" => c.endpoint = Some(value),
                "allowedips" => c.allowed_ips.extend(list(&value)),
                "persistentkeepalive" => {
                    c.keepalive = Some(value.parse().map_err(|_| format!("line {}: bad PersistentKeepalive", n + 1))?)
                }
                _ => {}
            },
            Section::Peer => {}
            Section::None => return Err(format!("line {}: setting outside [Interface] or [Peer]", n + 1)),
        }
    }

    if peers > 1 {
        return Err("The profile has more than one [Peer]; SparkBox expects a single Firewalla peer.".into());
    }
    if c.private_key.is_empty() {
        return Err("The profile has no PrivateKey in [Interface].".into());
    }
    if c.addresses.is_empty() {
        return Err("The profile has no Address in [Interface].".into());
    }
    if c.peer_public_key.is_empty() {
        return Err("The profile has no [Peer] PublicKey.".into());
    }
    if c.allowed_ips.is_empty() {
        c.allowed_ips.push("0.0.0.0/0".into());
    }
    Ok(c)
}

/// Splits "host:port" / "[v6]:port"; the port defaults to WireGuard's 51820.
pub fn split_endpoint(endpoint: &str) -> (String, u16) {
    let e = endpoint.trim();
    if let Some(rest) = e.strip_prefix('[') {
        if let Some((host, port)) = rest.split_once("]:") {
            return (host.to_string(), port.parse().unwrap_or(51820));
        }
    }
    match e.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => (host.to_string(), port.parse().unwrap_or(51820)),
        _ => (e.trim_matches(['[', ']']).to_string(), 51820),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WG: &str = "\
[Interface]
PrivateKey = yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=
Address = 10.6.0.5/32, fd00::5/128
DNS = 192.168.50.1, home.lan
MTU = 1420

[Peer]
PublicKey = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=
PresharedKey = 3pBmo1Qv/PHEeK7j1mxUoB6Hx3Ex3RsuLqbP6EVu0Ck=
AllowedIPs = 0.0.0.0/0, ::/0
Endpoint = vpn.example.net:51820
PersistentKeepalive = 25
";

    #[test]
    fn reads_a_wireguard_profile() {
        let c = parse(WG).unwrap();
        assert_eq!(c.protocol(), "wireguard");
        assert_eq!(c.private_key, "yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=");
        assert_eq!(c.addresses, ["10.6.0.5/32", "fd00::5/128"]);
        assert_eq!(c.dns, ["192.168.50.1"], "search domains are dropped");
        assert_eq!(c.mtu, Some(1420));
        assert_eq!(c.preshared_key.as_deref(), Some("3pBmo1Qv/PHEeK7j1mxUoB6Hx3Ex3RsuLqbP6EVu0Ck="));
        assert_eq!(c.allowed_ips, ["0.0.0.0/0", "::/0"]);
        assert_eq!(c.keepalive, Some(25));
        assert_eq!(split_endpoint(c.endpoint.as_deref().unwrap()), ("vpn.example.net".into(), 51820));
    }

    #[test]
    fn reads_amneziawg_settings() {
        let text = WG.replace(
            "MTU = 1420",
            "MTU = 1280\nJc = 4\nJmin = 40\nJmax = 70\nS1 = 15\nS2 = 21\nH1 = 1294727403\nH2 = 2\nH3 = 3\nH4 = 4\nI1 = <b 0xc0ffee><r 24>",
        );
        let c = parse(&text).unwrap();
        assert_eq!(c.protocol(), "amneziawg");
        assert_eq!(c.obfuscation.get("jc").map(String::as_str), Some("4"));
        assert_eq!(c.obfuscation.get("i1").map(String::as_str), Some("<b 0xc0ffee><r 24>"));
        assert_eq!(c.obfuscation.len(), 10);
    }

    #[test]
    fn rejects_amneziawg_2_settings() {
        assert!(parse(&WG.replace("MTU = 1420", "S3 = 10")).unwrap_err().contains("2.0"));
        assert!(parse(&WG.replace("MTU = 1420", "H1 = 100-200")).unwrap_err().contains("2.0"));
    }

    #[test]
    fn explains_what_is_missing() {
        assert!(parse("[Interface]\nAddress = 10.0.0.2/32\n").unwrap_err().contains("PrivateKey"));
        assert!(parse(&WG.replace("[Peer]", "[Peer]\n[Peer]")).unwrap_err().contains("more than one"));
        assert!(parse("PrivateKey = x").unwrap_err().contains("outside"));
    }

    #[test]
    fn endpoints_split_with_default_port() {
        assert_eq!(split_endpoint("203.0.113.24"), ("203.0.113.24".into(), 51820));
        assert_eq!(split_endpoint("203.0.113.24:51821"), ("203.0.113.24".into(), 51821));
        assert_eq!(split_endpoint("[2001:db8::1]:51820"), ("2001:db8::1".into(), 51820));
        assert_eq!(split_endpoint("2001:db8::1"), ("2001:db8::1".into(), 51820));
    }
}
