//! Bounded framed duplex transport; fleet identities remain inside the service.
use crate::{
    ipc,
    protocol::{self, Request, Response},
};
use anyhow::Result;
use std::io::{BufRead, BufReader, Write};
pub fn bridge(
    mut input: impl BufRead + Send + 'static,
    mut output: impl Write,
    request: &Request,
) -> Result<()> {
    let mut connection = ipc::connect()?;
    protocol::write_frame(request, &mut connection)?;
    let writer = connection.try_clone()?;
    let mut reader = BufReader::new(connection);
    let initial: Response = protocol::read_frame(&mut reader)?;
    protocol::write_frame(&initial, &mut output)?;
    if !initial.ok {
        return Ok(());
    }
    std::thread::spawn(move || {
        let mut writer = writer;
        while let Ok(frame) = protocol::read_frame::<Request>(&mut input) {
            if protocol::write_frame(&frame, &mut writer).is_err() {
                break;
            }
        }
        let _ = protocol::write_frame(
            &Request {
                method: "detach".into(),
                params: serde_json::json!({}),
            },
            &mut writer,
        );
    });
    while let Ok(frame) = protocol::read_frame::<Response>(&mut reader) {
        protocol::write_frame(&frame, &mut output)?;
        if frame.result.as_ref().is_some_and(|v| v["event"] == "end") || !frame.ok {
            break;
        }
    }
    Ok(())
}
