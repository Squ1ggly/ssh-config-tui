//! Port forwards, parsed into something a form can edit.
//!
//! ssh spells these as `LocalForward [bind:]port host:hostport`,
//! `RemoteForward [bind:]port host:hostport` and `DynamicForward [bind:]port`,
//! with unix-socket paths allowed in place of either side. We parse the common
//! TCP shapes and hand anything stranger back as raw text.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardKind {
    /// Listen here, connect from the far end. `ssh -L`.
    Local,
    /// Listen on the far end, connect from here. `ssh -R`.
    Remote,
    /// A SOCKS proxy on a local port. `ssh -D`.
    Dynamic,
}

impl ForwardKind {
    pub const ALL: [ForwardKind; 3] = [
        ForwardKind::Local,
        ForwardKind::Remote,
        ForwardKind::Dynamic,
    ];

    pub fn keyword(self) -> &'static str {
        match self {
            ForwardKind::Local => "LocalForward",
            ForwardKind::Remote => "RemoteForward",
            ForwardKind::Dynamic => "DynamicForward",
        }
    }

    pub fn from_keyword(keyword: &str) -> Option<Self> {
        ForwardKind::ALL
            .into_iter()
            .find(|k| k.keyword().eq_ignore_ascii_case(keyword))
    }

    pub fn label(self) -> &'static str {
        match self {
            ForwardKind::Local => "local",
            ForwardKind::Remote => "remote",
            ForwardKind::Dynamic => "socks",
        }
    }

    /// What this forward does, in a sentence, for the form.
    pub fn summary(self) -> &'static str {
        match self {
            ForwardKind::Local => "a port on this machine reaches a service over there",
            ForwardKind::Remote => "a port on the server reaches a service on this machine",
            ForwardKind::Dynamic => "a SOCKS proxy here, tunnelling through the server",
        }
    }

    pub fn listen_label(self) -> &'static str {
        match self {
            ForwardKind::Local | ForwardKind::Dynamic => "Listen on this machine",
            ForwardKind::Remote => "Listen on the server",
        }
    }

    pub fn dest_label(self) -> &'static str {
        match self {
            ForwardKind::Local => "Reaching, from the server",
            ForwardKind::Remote => "Reaching, from this machine",
            ForwardKind::Dynamic => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forward {
    pub kind: ForwardKind,
    /// Optional bind address on the listening side (`127.0.0.1`, `*`, ...).
    pub bind: String,
    /// The listening port (or a unix socket path).
    pub listen: String,
    /// Where traffic comes out. Empty for [`ForwardKind::Dynamic`].
    pub host: String,
    pub port: String,
}

impl Forward {
    pub fn new(kind: ForwardKind) -> Self {
        Forward {
            kind,
            bind: String::new(),
            listen: String::new(),
            host: if kind == ForwardKind::Dynamic {
                String::new()
            } else {
                "localhost".into()
            },
            port: String::new(),
        }
    }

    /// Parses the value part of a forward directive. Returns `None` for shapes
    /// we would not be able to write back faithfully.
    pub fn parse(keyword: &str, value: &str) -> Option<Self> {
        let kind = ForwardKind::from_keyword(keyword)?;
        let parts: Vec<&str> = value.split_whitespace().collect();

        let (listen_part, dest) = match (kind, parts.as_slice()) {
            (ForwardKind::Dynamic, [listen]) => (*listen, None),
            (_, [listen, dest]) => (*listen, Some((*dest).to_string())),
            // The historical `LocalForward 8080 host 80` spelling.
            (_, [listen, host, port]) => (*listen, Some(format!("{host}:{port}"))),
            _ => return None,
        };

        let (bind, listen) = split_host_port(listen_part);
        if listen.is_empty() || listen.contains('/') {
            return None; // a unix socket, or nothing we understand
        }

        let (host, port) = match dest {
            Some(dest) => {
                let (host, port) = split_host_port(&dest);
                if host.is_empty() || port.is_empty() || dest.contains('/') {
                    return None;
                }
                (host, port)
            }
            None => (String::new(), String::new()),
        };

        Some(Forward {
            kind,
            bind,
            listen,
            host,
            port,
        })
    }

    /// The value part of the directive, e.g. `127.0.0.1:8081 localhost:8081`.
    pub fn value(&self) -> String {
        let listen = join_host_port(&self.bind, &self.listen);
        match self.kind {
            ForwardKind::Dynamic => listen,
            _ => format!("{listen} {}", join_host_port(&self.host, &self.port)),
        }
    }

    pub fn line(&self) -> String {
        format!("{} {}", self.kind.keyword(), self.value())
    }

    /// A human-readable one-liner: `local  8081 → 127.0.0.1:8081`.
    pub fn describe(&self) -> String {
        let listen = join_host_port(&self.bind, &self.listen);
        match self.kind {
            ForwardKind::Dynamic => format!("SOCKS proxy on {listen}"),
            ForwardKind::Local => {
                format!("{listen} → {}", join_host_port(&self.host, &self.port))
            }
            ForwardKind::Remote => {
                format!(
                    "server {listen} → {}",
                    join_host_port(&self.host, &self.port)
                )
            }
        }
    }

    pub fn is_complete(&self) -> bool {
        let listen_ok = is_port(&self.listen);
        match self.kind {
            ForwardKind::Dynamic => listen_ok,
            _ => listen_ok && !self.host.trim().is_empty() && is_port(&self.port),
        }
    }
}

pub fn is_port(value: &str) -> bool {
    value.trim().parse::<u16>().is_ok_and(|p| p > 0)
}

/// Splits `host:port` into its parts, coping with bare ports and bracketed
/// IPv6 literals like `[::1]:8080`.
fn split_host_port(value: &str) -> (String, String) {
    let value = value.trim();
    if let Some((host, port)) = value
        .strip_prefix('[')
        .and_then(|rest| rest.split_once("]:"))
    {
        return (host.to_string(), port.to_string());
    }
    match value.rsplit_once(':') {
        Some((host, port)) => (host.to_string(), port.to_string()),
        None => (String::new(), value.to_string()),
    }
}

fn join_host_port(host: &str, port: &str) -> String {
    let host = host.trim();
    if host.is_empty() {
        return port.trim().to_string();
    }
    // Re-bracket IPv6 literals so the result round-trips.
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{}", port.trim())
    } else {
        format!("{host}:{}", port.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Option<Forward> {
        let (keyword, value) = line.split_once(' ').unwrap();
        Forward::parse(keyword, value.trim())
    }

    #[test]
    fn parses_a_plain_local_forward() {
        let forward = parse("LocalForward 8081 127.0.0.1:8081").unwrap();
        assert_eq!(forward.kind, ForwardKind::Local);
        assert_eq!(
            (forward.bind.as_str(), forward.listen.as_str()),
            ("", "8081")
        );
        assert_eq!(
            (forward.host.as_str(), forward.port.as_str()),
            ("127.0.0.1", "8081")
        );
        assert_eq!(forward.describe(), "8081 → 127.0.0.1:8081");
    }

    #[test]
    fn round_trips_every_shape_we_claim_to_understand() {
        for line in [
            "LocalForward 8081 127.0.0.1:8081",
            "LocalForward 127.0.0.1:5555 web.internal:8080",
            "LocalForward *:5000 localhost:5000",
            "RemoteForward 8080 localhost:3000",
            "DynamicForward 1080",
            "DynamicForward 127.0.0.1:1080",
            "LocalForward [::1]:8080 [fe80::1]:80",
        ] {
            let forward = parse(line).unwrap_or_else(|| panic!("failed to parse {line}"));
            assert_eq!(forward.line(), line, "round trip of {line}");
        }
    }

    #[test]
    fn normalises_the_three_token_spelling() {
        let forward = parse("LocalForward 8080 db.internal 5432").unwrap();
        assert_eq!(forward.line(), "LocalForward 8080 db.internal:5432");
    }

    #[test]
    fn declines_unix_sockets_and_nonsense() {
        assert!(parse("LocalForward /tmp/mysql.sock /var/run/mysqld.sock").is_none());
        assert!(parse("LocalForward 8080").is_none());
        assert!(parse("RemoteForward 8080 localhost").is_none());
        assert!(Forward::parse("Compression", "yes").is_none());
    }

    #[test]
    fn knows_when_a_forward_is_still_incomplete() {
        let mut forward = Forward::new(ForwardKind::Local);
        assert!(!forward.is_complete());
        forward.listen = "8080".into();
        forward.port = "80".into();
        assert!(forward.is_complete());
        forward.port = "not-a-port".into();
        assert!(!forward.is_complete());
    }

    #[test]
    fn dynamic_forwards_need_only_a_port() {
        let mut forward = Forward::new(ForwardKind::Dynamic);
        forward.listen = "1080".into();
        assert!(forward.is_complete());
        assert_eq!(forward.line(), "DynamicForward 1080");
        assert_eq!(forward.describe(), "SOCKS proxy on 1080");
    }
}
