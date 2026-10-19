#[cfg(windows)]
#[test]
fn named_pipe_reads_and_writes_do_not_block_each_other() {
    use serde_json::{json, Value};
    use sshcc_core::{config, ipc, protocol, security};
    use std::{io::BufReader, sync::mpsc, time::Duration};
    let directory = tempfile::tempdir().unwrap();
    let owner = security::current_principal().unwrap();
    let cfg = config::init(directory.path(), "master", &owner, "duplex", &[]).unwrap();
    std::env::set_var(
        "SSHCC_SOCKET",
        format!(r"\\.\pipe\sshcc-duplex-{}", uuid::Uuid::new_v4()),
    );
    let mut listener = ipc::Listener::bind(&cfg).unwrap();
    let server = std::thread::spawn(move || {
        let mut connection = listener.accept().unwrap();
        let mut reader = BufReader::new(connection.try_clone().unwrap());
        let (ready, started) = mpsc::channel();
        let input = std::thread::spawn(move || {
            ready.send(()).unwrap();
            protocol::read_frame::<Value>(&mut reader).unwrap()
        });
        started.recv().unwrap();
        std::thread::sleep(Duration::from_millis(30));
        // The read is already waiting. Synchronous duplicated Windows handles
        // deadlock here; overlapped operations permit the output to arrive.
        protocol::write_frame(&json!({"output":"ready"}), &mut connection).unwrap();
        assert_eq!(input.join().unwrap()["input"], "ping");
        protocol::write_frame(&json!({"ack":1}), &mut connection).unwrap();
    });
    let mut connection = ipc::connect().unwrap();
    let mut reader = BufReader::new(connection.try_clone().unwrap());
    let (tx, rx) = mpsc::channel();
    let output = std::thread::spawn(move || {
        tx.send(protocol::read_frame::<Value>(&mut reader).unwrap())
            .unwrap();
        tx.send(protocol::read_frame::<Value>(&mut reader).unwrap())
            .unwrap();
    });
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(3)).unwrap()["output"],
        "ready"
    );
    std::thread::sleep(Duration::from_millis(30));
    protocol::write_frame(&json!({"input":"ping"}), &mut connection).unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap()["ack"], 1);
    output.join().unwrap();
    server.join().unwrap();
    std::env::remove_var("SSHCC_SOCKET");
}
