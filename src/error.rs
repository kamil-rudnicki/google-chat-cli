use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub details: Value,
}

#[derive(Debug, Clone)]
pub struct AppError {
    pub command: String,
    pub code: String,
    pub message: String,
    pub details: Value,
    pub exit_code: i32,
}

impl AppError {
    pub fn usage(command: impl Into<String>, message: impl Into<String>, details: Value) -> Self {
        Self {
            command: command.into(),
            code: "usage_error".to_string(),
            message: message.into(),
            details,
            exit_code: 2,
        }
    }

    pub fn missing_auth(
        command: impl Into<String>,
        message: impl Into<String>,
        details: Value,
    ) -> Self {
        Self {
            command: command.into(),
            code: "missing_auth".to_string(),
            message: message.into(),
            details,
            exit_code: 2,
        }
    }

    pub fn google_api(command: impl Into<String>, status: u16, body: Value) -> Self {
        let google_error = body.get("error").unwrap_or(&body);
        let google_status = google_error
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("UNKNOWN");
        let message = google_error
            .get("message")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("Google Chat API returned HTTP {status}."));

        Self {
            command: command.into(),
            code: "google_api_error".to_string(),
            message,
            details: json!({
                "status": status,
                "googleStatus": google_status,
                "body": body,
            }),
            exit_code: 4,
        }
    }

    pub fn local_io(
        command: impl Into<String>,
        message: impl Into<String>,
        details: Value,
    ) -> Self {
        Self {
            command: command.into(),
            code: "local_io_error".to_string(),
            message: message.into(),
            details,
            exit_code: 5,
        }
    }

    pub fn oauth(command: impl Into<String>, message: impl Into<String>, details: Value) -> Self {
        Self {
            command: command.into(),
            code: "oauth_error".to_string(),
            message: message.into(),
            details,
            exit_code: 6,
        }
    }

    pub fn status(&self) -> Option<u16> {
        self.details
            .get("status")
            .and_then(Value::as_u64)
            .and_then(|status| u16::try_from(status).ok())
    }

    pub fn body(&self) -> ErrorBody {
        ErrorBody {
            code: self.code.clone(),
            message: self.message.clone(),
            details: self.details.clone(),
        }
    }
}

impl From<serde_json::Error> for AppError {
    fn from(error: serde_json::Error) -> Self {
        Self::local_io(
            "json",
            "failed to process JSON",
            json!({ "jsonError": error.to_string() }),
        )
    }
}
