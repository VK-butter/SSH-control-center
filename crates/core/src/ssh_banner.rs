//! Banner-only SSH probe: TCP connect and read the server's identification line, never log in,
//! so the probed host sees no session, process or logon event.
use std::{
    io::Read,
    net::{TcpStream, ToSocketAddrs},
    time::Duration,
};

/// Per connect and per read.
pub const TIMEOUT: Duration = Duration::from_secs(3);
/// RFC 4253 caps the identification line at 255 bytes; allow a few lines before it.
const MAX_HEAD: usize = 1024;

/// The `SSH-…` identification line at `address:port`, without the line ending.
/// Errors read `address:port: reason`.
pub fn read(address: &str, port: u16) -> Result<String, String> {
    let fail = |error: &dyn std::fmt::Display| format!("{address}:{port}: {error}");
    let mut last = fail(&"address did not resolve");
    for addr in (address, port).to_socket_addrs().map_err(|e| fail(&e))? {
        let mut stream = match TcpStream::connect_timeout(&addr, TIMEOUT) {
            Ok(stream) => stream,
            Err(error) => {
                last = fail(&error);
                continue;
            }
        };
        stream
            .set_read_timeout(Some(TIMEOUT))
            .map_err(|e| fail(&e))?;
        return banner_line(&mut stream)
            .ok_or_else(|| fail(&"port answered without an SSH banner"));
    }
    Err(last)
}

/// Reads until the `SSH-` line is complete, the peer stops sending, or `MAX_HEAD` is reached.
fn banner_line(stream: &mut impl Read) -> Option<String> {
    let mut head = Vec::new();
    let mut chunk = [0u8; 256];
    loop {
        let read = match stream.read(&mut chunk) {
            Ok(0) | Err(_) => 0,
            Ok(read) => read,
        };
        head.extend_from_slice(&chunk[..read]);
        let start = head.windows(4).position(|w| w == b"SSH-");
        let line = start.map(|s| &head[s..]);
        let end = line.and_then(|l| l.iter().position(|b| *b == b'\n' || *b == b'\r'));
        if read == 0 || end.is_some() || head.len() >= MAX_HEAD {
            let line = line?;
            let line = &line[..end.unwrap_or(line.len())];
            return Some(String::from_utf8_lossy(line).into_owned());
        }
    }
}
