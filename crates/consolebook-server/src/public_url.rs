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
        authority(rest).map_err(|problem| format!("public URL {raw:?} {problem}"))?;
        Ok(Self {
            origin: format!("{}://{rest}", if secure { "https" } else { "http" }),
            secure,
        })
    }
}

/// Checks `host[:port]`, where the host is a DNS name, an IPv4 address,
/// or a bracketed IPv6 address, and the port is 1 to 65535.
fn authority(rest: &str) -> Result<(), &'static str> {
    let (host, port) = if let Some(inner) = rest.strip_prefix('[') {
        let (address, after) = inner
            .split_once(']')
            .ok_or("has an unclosed [ in its host")?;
        if address.parse::<std::net::Ipv6Addr>().is_err() {
            return Err("has an invalid IPv6 address");
        }
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
            Some((host, port)) => (host, Some(port)),
            None => (rest, None),
        }
    };
    if host.is_empty() {
        return Err("has no host");
    }
    if !rest.starts_with('[')
        && !host.split('.').all(|label| {
            !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
    {
        return Err("has an invalid host name");
    }
    if let Some(port) = port
        && !port.parse::<u16>().is_ok_and(|port| port > 0)
    {
        return Err("has an invalid port (1 to 65535)");
    }
    Ok(())
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
        ] {
            assert!(raw.parse::<PublicUrl>().is_err(), "{raw} was accepted");
        }
    }
}
