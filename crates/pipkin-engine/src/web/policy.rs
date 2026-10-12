//! The network boundary for fetched URLs: only `http`/`https`, no credentials in the URL, no
//! loopback/private/link-local/multicast targets, checked for every redirect hop, with the
//! resolved addresses pinned to the connection so a name cannot resolve one way for the check
//! and another for the connect.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use url::{Host, Url};

/// Why a URL may not be fetched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blocked {
    Scheme(String),
    Credentials,
    NoHost,
    PrivateHost(String),
    Unresolvable(String),
}

impl std::fmt::Display for Blocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Blocked::Scheme(s) => write!(f, "only http and https URLs can be fetched (got {s})"),
            Blocked::Credentials => f.write_str("URLs with embedded credentials are not fetched"),
            Blocked::NoHost => f.write_str("the URL has no host"),
            Blocked::PrivateHost(h) => write!(
                f,
                "{h} is a local or private network target, which fetched pages may not reach"
            ),
            Blocked::Unresolvable(h) => write!(f, "{h} could not be resolved"),
        }
    }
}

fn ipv4_is_public(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    !(ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_unspecified()
        || ip.is_documentation()
        // Carrier-grade NAT (100.64.0.0/10).
        || (o[0] == 100 && (64..=127).contains(&o[1]))
        // Benchmarking (198.18.0.0/15) and reserved (240.0.0.0/4).
        || (o[0] == 198 && (18..=19).contains(&o[1]))
        || o[0] >= 240)
}

fn ipv6_is_public(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return ipv4_is_public(v4);
    }
    let seg = ip.segments();
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        // Unique local (fc00::/7) and link local (fe80::/10).
        || (seg[0] & 0xfe00) == 0xfc00
        || (seg[0] & 0xffc0) == 0xfe80
        // Documentation (2001:db8::/32) and IPv4-compatible / 6to4-of-private are rare;
        // documentation is blocked, the rest falls through to the mapped check above.
        || (seg[0] == 0x2001 && seg[1] == 0x0db8))
}

/// Whether an address is a public (global) target.
pub fn ip_is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => ipv4_is_public(v4),
        IpAddr::V6(v6) => ipv6_is_public(v6),
    }
}

fn name_is_local(name: &str) -> bool {
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    name == "localhost"
        || name.ends_with(".localhost")
        || name.ends_with(".local")
        || name.ends_with(".internal")
        || name.ends_with(".home.arpa")
        || !name.contains('.')
}

/// Static checks that need no network: scheme, credentials, host shape and literal IPs.
pub fn check_url(url: &Url, allow_private: bool) -> Result<(), Blocked> {
    match url.scheme() {
        "http" | "https" => {}
        other => return Err(Blocked::Scheme(other.to_owned())),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Blocked::Credentials);
    }
    match url.host() {
        None => Err(Blocked::NoHost),
        Some(_) if allow_private => Ok(()),
        Some(Host::Ipv4(ip)) if !ipv4_is_public(ip) => Err(Blocked::PrivateHost(ip.to_string())),
        Some(Host::Ipv6(ip)) if !ipv6_is_public(ip) => Err(Blocked::PrivateHost(ip.to_string())),
        Some(Host::Domain(name)) if name_is_local(name) => {
            Err(Blocked::PrivateHost(name.to_owned()))
        }
        Some(_) => Ok(()),
    }
}

/// Resolve the host and require every address to be public (unless private targets are
/// allowed, as in tests against a local server). The addresses are returned so the
/// connection can be pinned to exactly what was checked.
pub async fn resolve_checked(url: &Url, allow_private: bool) -> Result<Vec<SocketAddr>, Blocked> {
    check_url(url, allow_private)?;
    let host = url.host_str().ok_or(Blocked::NoHost)?.to_owned();
    let port = url.port_or_known_default().unwrap_or(80);
    let addrs: Vec<SocketAddr> = match url.host() {
        Some(Host::Ipv4(ip)) => vec![SocketAddr::new(IpAddr::V4(ip), port)],
        Some(Host::Ipv6(ip)) => vec![SocketAddr::new(IpAddr::V6(ip), port)],
        _ => tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|_| Blocked::Unresolvable(host.clone()))?
            .collect(),
    };
    if addrs.is_empty() {
        return Err(Blocked::Unresolvable(host));
    }
    if !allow_private && !addrs.iter().all(|a| ip_is_public(a.ip())) {
        return Err(Blocked::PrivateHost(host));
    }
    Ok(addrs)
}

/// A redirect target is checked like the first URL. A cross-origin hop to a URL that
/// carries credentials is refused outright (never followed with the credentials stripped).
pub fn redirect_target(from: &Url, location: &str) -> Result<Url, Blocked> {
    let target = from
        .join(location)
        .map_err(|_| Blocked::Unresolvable(location.to_owned()))?;
    if (!target.username().is_empty() || target.password().is_some())
        && target.origin() != from.origin()
    {
        return Err(Blocked::Credentials);
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn private_and_local_targets_are_blocked_and_public_ones_pass() {
        for blocked in [
            "http://127.0.0.1/",
            "http://10.1.2.3/x",
            "https://192.168.1.1/",
            "http://172.16.5.5/",
            "http://169.254.169.254/latest/meta-data",
            "http://100.64.0.1/",
            "http://0.0.0.0/",
            "http://[::1]/",
            "http://[fe80::1]/",
            "http://[fd00::1]/",
            "http://[::ffff:10.0.0.1]/",
            "http://localhost/",
            "http://db.localhost/",
            "http://printer.local/",
            "http://intranet/",
        ] {
            assert!(
                matches!(check_url(&u(blocked), false), Err(Blocked::PrivateHost(_))),
                "{blocked}"
            );
            assert!(
                check_url(&u(blocked), true).is_ok(),
                "{blocked} with override"
            );
        }
        for ok in [
            "https://example.com/",
            "http://93.184.216.34/",
            "http://[2606:2800:220:1:248:1893:25c8:1946]/",
        ] {
            assert_eq!(check_url(&u(ok), false), Ok(()), "{ok}");
        }
    }

    #[test]
    fn schemes_and_credentials_are_refused() {
        assert_eq!(
            check_url(&u("ftp://example.com/"), false),
            Err(Blocked::Scheme("ftp".into()))
        );
        assert_eq!(
            check_url(&u("file:///etc/passwd"), false),
            Err(Blocked::Scheme("file".into()))
        );
        assert_eq!(
            check_url(&u("https://user:pw@example.com/"), false),
            Err(Blocked::Credentials)
        );
        assert_eq!(
            check_url(&u("https://user@example.com/"), false),
            Err(Blocked::Credentials)
        );
    }

    #[test]
    fn redirects_resolve_relative_targets_and_refuse_cross_origin_credentials() {
        let from = u("https://example.com/a/b");
        assert_eq!(
            redirect_target(&from, "../c").unwrap().as_str(),
            "https://example.com/c"
        );
        assert_eq!(
            redirect_target(&from, "https://other.example/x")
                .unwrap()
                .as_str(),
            "https://other.example/x"
        );
        assert_eq!(
            redirect_target(&from, "https://u:p@other.example/x"),
            Err(Blocked::Credentials)
        );
        // Same-origin credentials are then refused by the static check anyway.
        let same = redirect_target(&from, "https://u:p@example.com/x").unwrap();
        assert_eq!(check_url(&same, false), Err(Blocked::Credentials));
    }

    #[tokio::test]
    async fn resolution_pins_literal_addresses_and_rejects_private_results() {
        let addrs = resolve_checked(&u("http://127.0.0.1:8080/"), true)
            .await
            .unwrap();
        assert_eq!(addrs, vec!["127.0.0.1:8080".parse().unwrap()]);
        assert!(matches!(
            resolve_checked(&u("http://localhost:8080/"), false).await,
            Err(Blocked::PrivateHost(_))
        ));
    }
}
