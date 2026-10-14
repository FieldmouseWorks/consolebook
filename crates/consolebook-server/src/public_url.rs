//! The address people use to reach this installation (ADR 0025).
//!
//! Operators behind a TLS reverse proxy set it (`--public-url` /
//! `CONSOLEBOOK_PUBLIC_URL`). An `https://` address marks every session
//! cookie `Secure`, so a browser never sends it over plain HTTP. It is an
//! explicit setting, never inferred from forwarded headers that a client
//! reaching the service directly could forge.

use std::fmt;
use std::str::FromStr;

/// An origin: `http` or `https`, a host, and an optional port. Nothing
/// else, because Consolebook is served at the root of its origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicUrl {
    origin: String,
    secure: bool,
}

impl PublicUrl {
    /// The origin as given, without a trailing slash.
    #[must_use]
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Whether session cookies must carry `Secure`.
    #[must_use]
    pub fn is_secure(&self) -> bool {
        self.secure
    }
}

impl fmt::Display for PublicUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.origin)
    }
}

impl FromStr for PublicUrl {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let trimmed = raw.trim().trim_end_matches('/');
        let (secure, rest) = if let Some(rest) = trimmed.strip_prefix("https://") {
            (true, rest)
        } else if let Some(rest) = trimmed.strip_prefix("http://") {
            (false, rest)
        } else {
            return Err(format!(
                "public URL {raw:?} must start with https:// (or http:// for a trial)"
            ));
        };
        if rest.contains(['/', '?', '#', '@']) || rest.chars().any(char::is_whitespace) {
            return Err(format!(
                "public URL {raw:?} must be only scheme, host, and optional port; Consolebook is served at the root"
            ));
        }
        let authority =
            authority(rest, secure).map_err(|problem| format!("public URL {raw:?} {problem}"))?;
        Ok(Self {
            origin: format!("{}://{authority}", if secure { "https" } else { "http" }),
            secure,
        })
    }
}

/// Checks `host[:port]`, where the host is a DNS name, an IPv4 address,
/// or a bracketed IPv6 address, and the port is 1 to 65535. Returns it as
/// a browser writes the origin (lowercase host, compressed IPv6, no default
/// port), so the sign-in page can compare it with `location.origin`.
fn authority(rest: &str, secure: bool) -> Result<String, &'static str> {
    let (host, port) = if let Some(inner) = rest.strip_prefix('[') {
        let (address, after) = inner
            .split_once(']')
            .ok_or("has an unclosed [ in its host")?;
        let Ok(address) = address.parse::<std::net::Ipv6Addr>() else {
            return Err("has an invalid IPv6 address");
        };
        let address = format!("[{address}]");
        match after {
            "" => (address, None),
            _ => (
                address,
                Some(
                    after
                        .strip_prefix(':')
                        .ok_or("has text after its IPv6 address")?,
                ),
            ),
        }
    } else {
        match rest.split_once(':') {
            Some((host, port)) => (host.to_ascii_lowercase(), Some(port)),
            None => (rest.to_ascii_lowercase(), None),
        }
    };
    if host.is_empty() {
        return Err("has no host");
    }
    if !rest.starts_with('[') {
        if !host.split('.').all(|label| {
            !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        }) {
            return Err("has an invalid host name");
        }
        // Browsers read a host whose last label is numeric (decimal or 0x)
        // as an IPv4 address and rewrite or refuse anything but the plain
        // dotted form, so only that form is accepted.
        let last = host.rsplit('.').next().unwrap_or(&host);
        let numeric = last.chars().all(|c| c.is_ascii_digit())
            || last
                .get(..2)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("0x"));
        if numeric && host.parse::<std::net::Ipv4Addr>().is_err() {
            return Err("has an invalid IPv4 address");
        }
    }
    let port = match port {
        None => None,
        Some(port) => match port.parse::<u16>() {
            Ok(0) | Err(_) => return Err("has an invalid port (1 to 65535)"),
            Ok(port) if port == if secure { 443 } else { 80 } => None,
            Ok(port) => Some(port),
        },
    };
    Ok(match port {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

#[cfg(test)]
mod tests {
    use super::PublicUrl;

    #[test]
    fn https_origins_are_secure_and_normalized() {
        let url: PublicUrl = "https://consolebook.example.org/".parse().expect("valid");
        assert!(url.is_secure());
        assert_eq!(url.origin(), "https://consolebook.example.org");
        let url: PublicUrl = "http://192.0.2.10:7770".parse().expect("valid");
        assert!(!url.is_secure());
        assert_eq!(url.origin(), "http://192.0.2.10:7770");
        let url: PublicUrl = "https://[2001:db8::1]:8443".parse().expect("valid");
        assert_eq!(url.origin(), "https://[2001:db8::1]:8443");
        assert!("https://[2001:db8::1]".parse::<PublicUrl>().is_ok());
        assert!("https://localhost:8443".parse::<PublicUrl>().is_ok());
        assert!("https://cb1.example.org".parse::<PublicUrl>().is_ok());
        // As a browser writes the origin: lowercase, no default port,
        // compressed IPv6.
        for (raw, origin) in [
            (
                "https://Consolebook.Example.ORG:443/",
                "https://consolebook.example.org",
            ),
            ("http://192.0.2.10:80", "http://192.0.2.10"),
            (
                "https://consolebook.example.org:80",
                "https://consolebook.example.org:80",
            ),
            (
                "https://[2001:0DB8:0:0:0:0:0:1]:443",
                "https://[2001:db8::1]",
            ),
        ] {
            let url: PublicUrl = raw.parse().expect("valid");
            assert_eq!(url.origin(), origin, "{raw}");
        }
    }

    #[test]
    fn anything_but_an_origin_is_refused() {
        for raw in [
            "consolebook.example.org",
            "ftp://consolebook.example.org",
            "https://",
            "https://consolebook.example.org/training",
            "https://consolebook.example.org?x=1",
            "https://user@consolebook.example.org",
            "https://consolebook example.org",
            "https://:",
            "https://:443",
            "https://consolebook.example.org:",
            "https://consolebook.example.org:notaport",
            "https://consolebook.example.org:0",
            "https://consolebook.example.org:65536",
            "https://[broken",
            "https://[2001:db8::1",
            "https://[not-ipv6]",
            "https://[2001:db8::1]x",
            "https://consolebook..example.org",
            "https://consolebook_example.org",
            "https://999.999.999.999",
            "https://0x7f.0.0.1",
            "https://127.1",
            "https://010.0.0.1",
            "https://example.0x10",
        ] {
            assert!(raw.parse::<PublicUrl>().is_err(), "{raw} was accepted");
        }
    }
}
