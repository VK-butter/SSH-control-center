use sshcc_core::ssh_banner;
use std::{io::Write, net::TcpListener, thread};

/// A one-shot local listener that writes `reply` to the first connection.
fn listener(reply: &'static [u8]) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.write_all(reply);
        }
    });
    port
}

#[test]
fn reads_the_ssh_banner_line() {
    let port = listener(b"SSH-2.0-OpenSSH_for_Windows_9.5\r\n");
    assert_eq!(
        ssh_banner::read("127.0.0.1", port).unwrap(),
        "SSH-2.0-OpenSSH_for_Windows_9.5"
    );
}

#[test]
fn skips_lines_before_the_banner() {
    let port = listener(b"Welcome\r\nSSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.5\r\n");
    assert_eq!(
        ssh_banner::read("127.0.0.1", port).unwrap(),
        "SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.5"
    );
}

#[test]
fn a_port_without_an_ssh_banner_is_an_error() {
    let port = listener(b"HTTP/1.1 400 Bad Request\r\n\r\n");
    let error = ssh_banner::read("127.0.0.1", port).unwrap_err();
    assert!(error.contains("without an SSH banner"), "{error}");
}

#[test]
fn a_closed_port_is_an_error() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let error = ssh_banner::read("127.0.0.1", port).unwrap_err();
    assert!(error.starts_with(&format!("127.0.0.1:{port}: ")), "{error}");
}
