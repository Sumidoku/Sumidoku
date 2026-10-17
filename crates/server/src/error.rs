use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error(transparent)]
	Db(#[from] sqlx::Error),
	#[error(transparent)]
	Migrate(#[from] sqlx::migrate::MigrateError),
	#[error(transparent)]
	Io(#[from] std::io::Error),
	#[error(transparent)]
	Source(#[from] aidoku_wasm::Error),
}

#[derive(Debug)]
pub struct ApiError {
	status: StatusCode,
	code: &'static str,
	message: String,
}

impl ApiError {
	fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
		Self {
			status,
			code,
			message: message.into(),
		}
	}

	pub fn not_found(message: impl Into<String>) -> Self {
		Self::new(StatusCode::NOT_FOUND, "not_found", message)
	}

	/// 源不支持某动态能力时按端点约定返回 404，但 code 仍是 unsupported
	pub fn unsupported_404(message: impl Into<String>) -> Self {
		Self::new(StatusCode::NOT_FOUND, "unsupported", message)
	}

	pub fn bad_request(message: impl Into<String>) -> Self {
		Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
	}

	pub fn source_disabled(key: &str) -> Self {
		Self::new(
			StatusCode::CONFLICT,
			"source_disabled",
			format!("source {key} is disabled"),
		)
	}
}

impl From<aidoku_wasm::Error> for ApiError {
	fn from(e: aidoku_wasm::Error) -> Self {
		use aidoku_wasm::Error as E;
		match e {
			E::Message(m) => Self::new(StatusCode::BAD_GATEWAY, "source_error", m),
			E::Unimplemented => Self::new(StatusCode::BAD_REQUEST, "unsupported", "feature not supported by source"),
			E::Unsupported(name) => Self::new(
				StatusCode::BAD_REQUEST,
				"unsupported",
				format!("feature not supported by source: {name}"),
			),
			E::Timeout => Self::new(StatusCode::GATEWAY_TIMEOUT, "timeout", "source call timed out"),
			E::Trap(m) => Self::new(StatusCode::BAD_GATEWAY, "source_error", format!("wasm trap: {m}")),
			E::Instance(m) => Self::new(StatusCode::BAD_GATEWAY, "source_error", m),
			other => Self::new(StatusCode::BAD_GATEWAY, "source_error", other.to_string()),
		}
	}
}

impl From<sqlx::Error> for ApiError {
	fn from(e: sqlx::Error) -> Self {
		Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string())
	}
}

impl From<serde_json::Error> for ApiError {
	fn from(e: serde_json::Error) -> Self {
		Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string())
	}
}

impl IntoResponse for ApiError {
	fn into_response(self) -> Response {
		(
			self.status,
			Json(serde_json::json!({
				"error": { "code": self.code, "message": self.message }
			})),
		)
			.into_response()
	}
}

pub type ApiResult<T> = std::result::Result<T, ApiError>;
