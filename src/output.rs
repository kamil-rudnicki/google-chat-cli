use crate::error::{AppError, ErrorBody};
use serde::Serialize;
use serde_json::{Value, json};
use std::io::{self, Write};

#[derive(Debug, Clone, Serialize)]
pub struct SuccessEnvelope {
    pub ok: bool,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    pub data: Value,
    pub meta: Value,
}

#[derive(Debug, Clone, Serialize)]
struct ErrorEnvelope {
    ok: bool,
    command: String,
    error: ErrorBody,
}

pub fn success(
    command: impl Into<String>,
    account: Option<String>,
    data: Value,
    meta: Value,
) -> SuccessEnvelope {
    SuccessEnvelope {
        ok: true,
        command: command.into(),
        account,
        data,
        meta,
    }
}

pub fn write_success(envelope: &SuccessEnvelope) -> io::Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer_pretty(&mut handle, envelope)?;
    writeln!(handle)
}

pub fn write_error(error: &AppError) -> io::Result<()> {
    let envelope = ErrorEnvelope {
        ok: false,
        command: error.command.clone(),
        error: error.body(),
    };
    let stderr = io::stderr();
    let mut handle = stderr.lock();
    serde_json::to_writer_pretty(&mut handle, &envelope)?;
    writeln!(handle)
}

pub fn write_progress(
    command: &str,
    stage: &str,
    current: usize,
    total: Option<usize>,
    meta: Value,
) {
    let event = json!({
        "ok": true,
        "command": command,
        "event": "progress",
        "stage": stage,
        "current": current,
        "total": total,
        "meta": meta,
    });
    if let Ok(line) = serde_json::to_string(&event) {
        let stderr = io::stderr();
        let mut handle = stderr.lock();
        let _ = writeln!(handle, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn success_envelope_is_pretty_json() {
        let envelope = success("version", None, json!({ "version": "1.0.0" }), json!({}));
        let rendered = serde_json::to_string_pretty(&envelope).unwrap();
        assert!(rendered.contains('\n'));
        assert!(rendered.contains("  \"ok\": true"));
    }
}
