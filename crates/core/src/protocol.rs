use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, Read, Write};
pub const MAX_FRAME: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub method: String,
    #[serde(default)]
    pub params: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: String,
    pub message: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}
impl Response {
    pub fn from_result(result: Result<Value>) -> Self {
        match result {
            Ok(value) => Self {
                ok: true,
                result: Some(value),
                error: None,
            },
            Err(err) => {
                let message = format!("{err:#}");
                let code = message
                    .split(':')
                    .next()
                    .filter(|s| s.starts_with("E_") && s.len() < 60)
                    .unwrap_or("E_OPERATION")
                    .to_string();
                Self {
                    ok: false,
                    result: None,
                    error: Some(RpcError { code, message }),
                }
            }
        }
    }
    pub fn into_result(self) -> Result<Value> {
        if self.ok {
            Ok(self.result.unwrap_or(Value::Null))
        } else {
            let err = self.error.context("E_PROTOCOL: missing error")?;
            bail!("{}: {}", err.code, err.message)
        }
    }
}
pub fn read_frame<T: for<'de> Deserialize<'de>>(reader: &mut impl BufRead) -> Result<T> {
    let mut bytes = Vec::new();
    let count = (&mut *reader)
        .take((MAX_FRAME + 1) as u64)
        .read_until(b'\n', &mut bytes)?;
    if count == 0 {
        bail!("E_PROTOCOL: unexpected EOF");
    }
    if count > MAX_FRAME || bytes.last() != Some(&b'\n') {
        bail!("E_PROTOCOL: oversized or incomplete frame");
    }
    Ok(serde_json::from_slice(&bytes).context("E_PROTOCOL: malformed JSON")?)
}
pub fn write_frame(value: &impl Serialize, writer: &mut impl Write) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_FRAME {
        bail!("E_PROTOCOL: response exceeds frame limit");
    }
    writer.write_all(&bytes)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}
pub fn string<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .with_context(|| format!("E_ARGUMENT: {key} is required"))
}
