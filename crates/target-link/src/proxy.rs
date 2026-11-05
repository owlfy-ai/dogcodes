//! OpenSSH ignores system proxy settings and has no proxy support of its own beyond
//! running a program. That program is this one: `ProxyCommand` starts it, it reaches
//! the server through the proxy, and its stdin and stdout become `ssh`'s connection.
//! Everything crossing the proxy is already encrypted by SSH, and host keys are still
//! checked, so a proxy can carry the connection but not read or alter it.
use agent_protocol::{SshProxy, SshProxyKind};
use std::{
    ffi::OsString,
    io::{self, BufRead, BufReader, Read, Write},
    net::{IpAddr, TcpStream, ToSocketAddrs},
    path::Path,
    time::Duration,
};

const FLAG: &str = "--ssh-proxy";
/// Marks what this program writes, so a proxy failure isn't reported as the server's.
pub(crate) const ERROR_PREFIX: &str = "dogcodes-proxy:";
const CONNECT_LIMIT: Duration = Duration::from_secs(10);
const MAX_HTTP_RESPONSE: u64 = 16 * 1024;

/// A proxy to go through, in the form this program takes it on its command line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedProxy {
    pub(crate) kind: SshProxyKind,
    pub(crate) host: String,
    pub(crate) port: u16,
    /// Destinations the system says go direct, comma separated.
    pub(crate) bypass: String,
}

fn kind_name(kind: SshProxyKind) -> &'static str {
    match kind {
        SshProxyKind::Socks5 => "socks5",
        SshProxyKind::Http => "http",
    }
}

/// The value goes through a shell on the way to this program, so only what can appear
/// in a host name is let through.
fn safe_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 255
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
}

fn safe_bypass(list: &str) -> bool {
    list.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | ',' | '*' | '/'))
}

fn parse_system_url(url: &str, bypass: &str) -> Result<ResolvedProxy, String> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| format!("unrecognized system proxy: {url}"))?;
    let kind = match scheme.to_ascii_lowercase().as_str() {
        "socks5" | "socks5h" | "socks" => SshProxyKind::Socks5,
        "http" | "https" => SshProxyKind::Http,
        other => return Err(format!("unsupported system proxy type: {other}")),
    };
    let address = rest.trim_end_matches('/');
    if address.contains('@') {
        return Err("the system proxy requires a username and password".into());
    }
    let (host, port) = address
        .rsplit_once(':')
        .ok_or_else(|| format!("the system proxy has no port: {address}"))?;
    let port = port
        .parse()
        .map_err(|_| format!("the system proxy has an invalid port: {address}"))?;
    Ok(ResolvedProxy {
        kind,
        host: host.trim_matches(['[', ']']).to_owned(),
        port,
        bypass: bypass.to_owned(),
    })
}

/// What the saved choice means right now. A proxy that can't be used is an error, never
/// a silent direct connection: someone who chose a proxy may depend on it.
pub(crate) fn resolve(
    proxy: Option<&SshProxy>,
    system: impl FnOnce() -> SystemProxy,
) -> Result<Option<ResolvedProxy>, String> {
    let resolved = match proxy {
        None => return Ok(None),
        Some(SshProxy::Custom { kind, host, port }) => ResolvedProxy {
            kind: *kind,
            host: host.clone(),
            port: *port,
            bypass: String::new(),
        },
        Some(SshProxy::System) => match system() {
            SystemProxy::None => return Ok(None),
            SystemProxy::AutoConfig => {
                return Err(
                    "the system proxy uses an automatic configuration script, which isn't supported; specify a proxy instead"
                        .into(),
                );
            }
            SystemProxy::Url { url, bypass } => parse_system_url(&url, &bypass)?,
        },
    };
    if !safe_host(&resolved.host) || resolved.port == 0 {
        return Err(format!(
            "invalid proxy address: {}:{}",
            resolved.host, resolved.port
        ));
    }
    let bypass = if safe_bypass(&resolved.bypass) {
        resolved.bypass
    } else {
        String::new()
    };
    Ok(Some(ResolvedProxy { bypass, ..resolved }))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SystemProxy {
    None,
    AutoConfig,
    Url { url: String, bypass: String },
}

#[must_use]
pub fn system_proxy() -> SystemProxy {
    match agent_runtime::system_proxy() {
        agent_runtime::SystemProxy::None => SystemProxy::None,
        agent_runtime::SystemProxy::AutoConfig => SystemProxy::AutoConfig,
        agent_runtime::SystemProxy::Url { url, no_proxy } => SystemProxy::Url {
            url,
            bypass: no_proxy,
        },
    }
}

/// `ssh` runs this through a shell on Unix and directly on Windows. A literal `%` would
/// be read as one of its tokens.
pub(crate) fn proxy_command(program: &Path, proxy: &ResolvedProxy) -> OsString {
    let path = program.to_string_lossy().replace('%', "%%");
    let quoted = if cfg!(windows) {
        format!("\"{path}\"")
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    };
    let bypass = if proxy.bypass.is_empty() {
        "-"
    } else {
        &proxy.bypass
    };
    format!(
        "ProxyCommand={quoted} {FLAG} {} {}:{} {} %h %p",
        kind_name(proxy.kind),
        bracketed(&proxy.host),
        proxy.port,
        bypass.replace('%', "%%"),
    )
    .into()
}

/// Dispatch before anything else starts.
#[must_use]
pub fn run_if_requested() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [flag, kind, proxy, bypass, host, port] = args.as_slice() else {
        return false;
    };
    if flag != FLAG {
        return false;
    }
    let outcome = open(kind, proxy, bypass, host, port).and_then(relay);
    if let Err(error) = outcome {
        // `ssh` passes this on, so it ends up in the connection log.
        eprintln!("{ERROR_PREFIX} {error}");
        std::process::exit(1);
    }
    std::process::exit(0);
}

fn open(kind: &str, proxy: &str, bypass: &str, host: &str, port: &str) -> io::Result<TcpStream> {
    let port: u16 = port
        .parse()
        .map_err(|_| io::Error::other(format!("invalid destination port: {port}")))?;
    if bypassed(bypass, host) {
        return connect_to(&format!("{}:{port}", bracketed(host)))
            .map_err(|error| io::Error::new(error.kind(), format!("{host}:{port}: {error}")));
    }
    let mut stream = connect_to(proxy)
        .map_err(|error| io::Error::new(error.kind(), format!("proxy {proxy}: {error}")))?;
    stream.set_read_timeout(Some(CONNECT_LIMIT))?;
    stream.set_write_timeout(Some(CONNECT_LIMIT))?;
    match kind {
        "socks5" => socks5_handshake(&mut stream, host, port),
        "http" => http_connect(&mut stream, host, port),
        other => Err(io::Error::other(format!("unknown proxy type: {other}"))),
    }
    .map_err(|error| io::Error::new(error.kind(), format!("proxy {proxy}: {error}")))?;
    stream.set_read_timeout(None)?;
    stream.set_write_timeout(None)?;
    Ok(stream)
}

fn connect_to(address: &str) -> io::Result<TcpStream> {
    let mut failure = None;
    for candidate in address.to_socket_addrs()? {
        match TcpStream::connect_timeout(&candidate, CONNECT_LIMIT) {
            Ok(stream) => return Ok(stream),
            Err(error) => failure = Some(error),
        }
    }
    Err(failure.unwrap_or_else(|| io::Error::other("no address found")))
}

fn bracketed(host: &str) -> String {
    if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    }
}

/// `example.com`, `.example.com` and `*.example.com` all cover its subdomains, as
/// proxy lists conventionally do. Ranges such as `10.0.0.0/8` aren't matched.
fn bypassed(list: &str, host: &str) -> bool {
    if list == "-" {
        return false;
    }
    let host = host.to_ascii_lowercase();
    list.split(',').map(str::trim).any(|entry| {
        let entry = entry.to_ascii_lowercase();
        let domain = entry
            .strip_prefix("*.")
            .or_else(|| entry.strip_prefix('.'))
            .unwrap_or(&entry);
        !domain.is_empty() && (host == domain || host.ends_with(&format!(".{domain}")))
    })
}

fn socks5_handshake(stream: &mut (impl Read + Write), host: &str, port: u16) -> io::Result<()> {
    stream.write_all(&[5, 1, 0])?;
    let mut choice = [0; 2];
    stream.read_exact(&mut choice)?;
    if choice[0] != 5 {
        return Err(io::Error::other("not a SOCKS5 proxy"));
    }
    if choice[1] != 0 {
        return Err(io::Error::other(
            "the proxy requires a username and password, which isn't supported",
        ));
    }
    let mut request = vec![5, 1, 0];
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(address)) => {
            request.push(1);
            request.extend_from_slice(&address.octets());
        }
        Ok(IpAddr::V6(address)) => {
            request.push(4);
            request.extend_from_slice(&address.octets());
        }
        // The proxy resolves the name, so one only it can see still works.
        Err(_) => {
            let name = u8::try_from(host.len())
                .map_err(|_| io::Error::other("the destination name is too long"))?;
            request.push(3);
            request.push(name);
            request.extend_from_slice(host.as_bytes());
        }
    }
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request)?;
    let mut reply = [0; 4];
    stream.read_exact(&mut reply)?;
    if reply[1] != 0 {
        return Err(io::Error::other(format!(
            "the proxy refused the connection: {}",
            socks5_reason(reply[1])
        )));
    }
    let address_length = match reply[3] {
        1 => 4,
        4 => 16,
        3 => {
            let mut length = [0; 1];
            stream.read_exact(&mut length)?;
            usize::from(length[0])
        }
        _ => return Err(io::Error::other("the proxy sent an invalid reply")),
    };
    let mut bound = vec![0; address_length + 2];
    stream.read_exact(&mut bound)
}

fn socks5_reason(code: u8) -> &'static str {
    match code {
        2 => "not allowed by its rules",
        3 => "network unreachable",
        4 => "host unreachable",
        5 => "connection refused by the destination",
        6 => "timed out",
        _ => "general failure",
    }
}

fn http_connect(stream: &mut TcpStream, host: &str, port: u16) -> io::Result<()> {
    let target = format!("{}:{port}", bracketed(host));
    write!(
        stream,
        "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n"
    )?;
    // One byte at a time: anything read past the headers is already the server's.
    let mut reader = BufReader::with_capacity(1, stream.take(MAX_HTTP_RESPONSE));
    let mut status = String::new();
    reader.read_line(&mut status)?;
    let code = status.split_whitespace().nth(1).unwrap_or_default();
    if !code.starts_with('2') {
        let status = status.trim();
        return Err(io::Error::other(if code == "407" {
            "the proxy requires a username and password, which isn't supported".to_owned()
        } else {
            format!("the proxy refused the connection: {status}")
        }));
    }
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Err(io::Error::other("the proxy closed the connection"));
        }
        if line == "\r\n" || line == "\n" {
            return Ok(());
        }
    }
}

fn relay(stream: TcpStream) -> io::Result<()> {
    let mut to_server = stream.try_clone()?;
    std::thread::spawn(move || {
        let _ = io::copy(&mut io::stdin().lock(), &mut to_server);
        let _ = to_server.shutdown(std::net::Shutdown::Write);
    });
    let mut from_server = stream;
    let mut stdout = io::stdout().lock();
    let mut buffer = [0; 16 * 1024];
    loop {
        let count = from_server.read(&mut buffer)?;
        if count == 0 {
            return Ok(());
        }
        stdout.write_all(&buffer[..count])?;
        // Standard output holds data until a newline, which SSH's packets rarely have.
        stdout.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Replies from a script, keeping what it was sent.
    struct Script {
        reply: Cursor<Vec<u8>>,
        sent: Vec<u8>,
    }
    impl Read for Script {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.reply.read(buffer)
        }
    }
    impl Write for Script {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.sent.extend_from_slice(buffer);
            Ok(buffer.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    fn script(reply: &[u8]) -> Script {
        Script {
            reply: Cursor::new(reply.to_vec()),
            sent: vec![],
        }
    }

    #[test]
    fn socks5_sends_the_name_so_the_proxy_resolves_it() {
        let mut proxy = script(&[5, 0, 5, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        socks5_handshake(&mut proxy, "only.internal", 22).unwrap();
        let mut expected = vec![5, 1, 0, 5, 1, 0, 3, 13];
        expected.extend_from_slice(b"only.internal");
        expected.extend_from_slice(&[0, 22]);
        assert_eq!(proxy.sent, expected);
    }

    #[test]
    fn socks5_sends_an_address_as_one() {
        let mut proxy = script(&[5, 0, 5, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        socks5_handshake(&mut proxy, "47.101.41.40", 22).unwrap();
        assert_eq!(&proxy.sent[3..], &[5, 1, 0, 1, 47, 101, 41, 40, 0, 22]);
    }

    #[test]
    fn socks5_explains_a_refusal_and_a_request_for_credentials() {
        let refused = socks5_handshake(&mut script(&[5, 0, 5, 5, 0, 1]), "host", 22).unwrap_err();
        assert!(refused.to_string().contains("refused"), "{refused}");
        let credentials = socks5_handshake(&mut script(&[5, 0xff]), "host", 22).unwrap_err();
        assert!(
            credentials.to_string().contains("username"),
            "{credentials}"
        );
    }

    #[test]
    fn a_shell_cannot_be_reached_through_a_saved_address() {
        let custom = |host: &str| SshProxy::Custom {
            kind: SshProxyKind::Socks5,
            host: host.to_owned(),
            port: 7890,
        };
        let unused = || unreachable!();
        for host in ["a;touch x", "$(id)", "a b", "`id`", "", "a'b"] {
            assert!(resolve(Some(&custom(host)), unused).is_err(), "{host}");
        }
        for host in ["127.0.0.1", "proxy.example.com", "::1"] {
            assert!(resolve(Some(&custom(host)), unused).is_ok(), "{host}");
        }
    }

    #[test]
    fn no_choice_leaves_ssh_alone_and_never_reads_the_system() {
        assert_eq!(resolve(None, || unreachable!()), Ok(None));
    }

    #[test]
    fn the_system_setting_is_read_when_chosen() {
        let chosen = Some(&SshProxy::System);
        assert_eq!(resolve(chosen, || SystemProxy::None), Ok(None));
        assert!(resolve(chosen, || SystemProxy::AutoConfig).is_err());
        let url = |url: &str| SystemProxy::Url {
            url: url.to_owned(),
            bypass: "localhost,*.local".to_owned(),
        };
        let proxy = resolve(chosen, || url("socks5://127.0.0.1:7890"))
            .unwrap()
            .unwrap();
        assert_eq!(proxy.kind, SshProxyKind::Socks5);
        assert_eq!((proxy.host.as_str(), proxy.port), ("127.0.0.1", 7890));
        assert_eq!(proxy.bypass, "localhost,*.local");
        let proxy = resolve(chosen, || url("http://proxy:8080/"))
            .unwrap()
            .unwrap();
        assert_eq!(proxy.kind, SshProxyKind::Http);
        assert!(resolve(chosen, || url("http://user:pw@proxy:8080")).is_err());
    }

    #[test]
    fn bypass_covers_a_domain_and_its_subdomains_only() {
        let list = "localhost,*.local,.corp.example,127.0.0.1";
        for host in [
            "localhost",
            "a.local",
            "corp.example",
            "x.corp.example",
            "127.0.0.1",
        ] {
            assert!(bypassed(list, host), "{host}");
        }
        for host in ["47.101.41.40", "notlocal", "example"] {
            assert!(!bypassed(list, host), "{host}");
        }
        assert!(!bypassed("-", "localhost"));
    }

    #[test]
    fn the_command_quotes_the_program_and_escapes_tokens() {
        let proxy = ResolvedProxy {
            kind: SshProxyKind::Http,
            host: "127.0.0.1".into(),
            port: 8080,
            bypass: String::new(),
        };
        let command = proxy_command(Path::new("/Applications/dog 100%.app/d"), &proxy);
        let command = command.to_string_lossy();
        assert!(
            command.ends_with("--ssh-proxy http 127.0.0.1:8080 - %h %p"),
            "{command}"
        );
        assert!(command.contains("dog 100%%.app"), "{command}");
    }
}
