use crate::{
    ipc,
    protocol::{read_frame, write_frame, Request, Response},
};
use anyhow::Result;
use std::io::BufReader;
pub fn call(request: &Request) -> Result<serde_json::Value> {
    let mut connection = ipc::connect()?;
    write_frame(request, &mut connection)?;
    let response: Response = read_frame(&mut BufReader::new(connection))?;
    response.into_result()
}
