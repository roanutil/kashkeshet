//! Which `Host` header values the server answers to. Checking `Host` blocks
//! DNS rebinding: a web page whose own name has been pointed at this machine
//! still sends its own name, which is not on the list.
//!
//! Loopback names are always allowed; the operator can add more, such as
//! `host.docker.internal` or a Tailscale name. An entry without a port
//! allows any port; an entry with a port allows only that port.

use crate::config::{ALWAYS_ALLOWED_HOSTS, InvalidSetting};
use std::fmt;
use std::net::Ipv6Addr;

/// The host names the server answers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllowedHosts {
    /// The loopback names followed by the operator's.
    hosts: Vec<Host>,
    /// The operator's names as given, for the record of the server run.
    extra: Vec<String>,
}

impl AllowedHosts {
    /// The loopback names plus `extra`. Each extra entry is a host name or
    /// IP address with an optional port, such as `host.docker.internal` or
    /// `[fd7a::1]:7420`. Any other entry is refused, naming the setting
    /// `allow_host`.
    pub fn new(extra: &[String]) -> Result<AllowedHosts, InvalidSetting> {
        let mut hosts = Vec::with_capacity(ALWAYS_ALLOWED_HOSTS.len() + extra.len());
        for name in ALWAYS_ALLOWED_HOSTS.iter().copied() {
            match parse(name) {
                Some(host) => hosts.push(host),
                None => {
                    return Err(InvalidSetting::new(
                        "ALWAYS_ALLOWED_HOSTS",
                        format!("must hold host names, not {name:?}"),
                    ));
                }
            }
        }
        for name in extra {
            match parse(name) {
                Some(host) => hosts.push(host),
                None => {
                    return Err(InvalidSetting::new(
                        "allow_host",
                        format!(
                            "must be a host name or IP address with an optional port \
                             (such as host.docker.internal), not {name:?}"
                        ),
                    ));
                }
            }
        }
        Ok(AllowedHosts {
            hosts,
            extra: extra.to_vec(),
        })
    }

    /// True if a request whose `Host` header is `header` may be served.
    pub fn allows(&self, header: &str) -> bool {
        let Some(asked) = parse(header) else {
            return false;
        };
        self.hosts.iter().any(|allowed| {
            allowed.name == asked.name
                && match allowed.port {
                    Some(port) => asked.port == Some(port),
                    None => true,
                }
        })
    }

    /// Every allowed entry as text, loopback names first.
    pub fn entries(&self) -> Vec<String> {
        self.hosts.iter().map(Host::to_string).collect()
    }

    /// The names the operator added, as given.
    pub fn extra(&self) -> &[String] {
        &self.extra
    }
}

/// A host name or IP address, lowercased and without IPv6 brackets, with an
/// optional port.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Host {
    name: String,
    port: Option<u16>,
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let is_ipv6 = self.name.contains(':');
        match (is_ipv6, self.port) {
            (true, Some(port)) => write!(f, "[{}]:{port}", self.name),
            (false, Some(port)) => write!(f, "{}:{port}", self.name),
            (_, None) => write!(f, "{}", self.name),
        }
    }
}

/// Reads `name`, `name:port`, an IPv6 address, or `[ipv6]:port`. Returns
/// `None` for anything else, including schemes, paths and wildcards.
fn parse(text: &str) -> Option<Host> {
    let text = text.to_ascii_lowercase();
    if let Some(rest) = text.strip_prefix('[') {
        let (address, after) = rest.split_once(']')?;
        address.parse::<Ipv6Addr>().ok()?;
        let port = match after {
            "" => None,
            _ => Some(parse_port(after.strip_prefix(':')?)?),
        };
        return Some(Host {
            name: address.to_string(),
            port,
        });
    }
    if text.parse::<Ipv6Addr>().is_ok() {
        return Some(Host {
            name: text,
            port: None,
        });
    }
    let (name, port) = match text.split_once(':') {
        Some((name, port)) => (name, Some(parse_port(port)?)),
        None => (text.as_str(), None),
    };
    let is_name = name
        .split('.')
        .all(|label| !label.is_empty() && label.bytes().all(is_name_byte));
    is_name.then(|| Host {
        name: name.to_string(),
        port,
    })
}

fn parse_port(text: &str) -> Option<u16> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts(extra: &[&str]) -> AllowedHosts {
        let extra: Vec<String> = extra.iter().map(|s| s.to_string()).collect();
        AllowedHosts::new(&extra).expect("valid hosts")
    }

    #[test]
    fn loopback_names_are_always_allowed_on_any_port() {
        let h = hosts(&[]);
        for header in [
            "localhost",
            "localhost:7420",
            "127.0.0.1:1",
            "[::1]:7420",
            "[::1]",
            "LOCALHOST:7420",
        ] {
            assert!(h.allows(header), "{header}");
        }
        assert!(!h.allows("host.docker.internal:7420"));
        assert!(!h.allows("evil.example"));
    }

    #[test]
    fn added_names_are_allowed() {
        let h = hosts(&["host.docker.internal", "Box.Tail1234.TS.net"]);
        assert!(h.allows("host.docker.internal:7420"));
        assert!(h.allows("host.docker.internal"));
        assert!(h.allows("box.tail1234.ts.net"));
        assert!(!h.allows("docker.internal"));
        assert!(!h.allows("evil.host.docker.internal"));
    }

    #[test]
    fn a_port_in_an_entry_must_match() {
        let h = hosts(&["kkt.example:443", "[fd7a::1]:7420"]);
        assert!(h.allows("kkt.example:443"));
        assert!(!h.allows("kkt.example:8443"));
        assert!(!h.allows("kkt.example"));
        assert!(h.allows("[FD7A::1]:7420"));
        assert!(!h.allows("[fd7a::1]:7421"));
    }

    #[test]
    fn malformed_headers_are_refused() {
        let h = hosts(&[]);
        for header in [
            "",
            "localhost:",
            "localhost:abc",
            "localhost:99999",
            "localhost/x",
            "[::1",
            "localhost:7420:1",
            " localhost",
        ] {
            assert!(!h.allows(header), "{header:?}");
        }
    }

    #[test]
    fn bad_entries_are_refused_naming_the_setting() {
        for bad in [
            "",
            "*",
            "*.example",
            "http://host.docker.internal",
            "host.docker.internal/",
            "a..b",
            "host:",
            "host:port",
            "[not-ipv6]",
            "two words",
        ] {
            let err = AllowedHosts::new(&[bad.to_string()]).expect_err(bad);
            assert_eq!(err.name, "allow_host", "{bad:?}");
        }
    }

    #[test]
    fn entries_list_loopback_then_added_names() {
        let h = hosts(&["Host.Docker.Internal", "[fd7a::1]:7420", "x.example:8"]);
        assert_eq!(
            h.entries(),
            [
                "localhost",
                "127.0.0.1",
                "::1",
                "host.docker.internal",
                "[fd7a::1]:7420",
                "x.example:8"
            ]
        );
        assert_eq!(
            h.extra(),
            ["Host.Docker.Internal", "[fd7a::1]:7420", "x.example:8"]
        );
    }
}
