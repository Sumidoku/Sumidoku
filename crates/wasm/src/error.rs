#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("{0}")]
	Message(String),
	#[error("unimplemented")]
	Unimplemented,
	#[error("request error")]
	Request,
	#[error("html error")]
	Html,
	#[error("js error")]
	Js,
	#[error("canvas error")]
	Canvas,
	#[error("utf8 error")]
	Utf8,
	#[error("json parse error")]
	JsonParse,
	#[error("deserialize error")]
	Deserialize,
	#[error("wasm trap: {0}")]
	Trap(String),
	#[error("call timeout")]
	Timeout,
	#[error("feature not supported by source: {0}")]
	Unsupported(&'static str),
	#[error("instance error: {0}")]
	Instance(String),
}

impl Error {
	pub fn from_code(code: i32) -> Self {
		match code {
			-2 => Self::Unimplemented,
			-3 => Self::Request,
			-4 => Self::Html,
			-5 => Self::Js,
			-6 => Self::Canvas,
			-7 => Self::Utf8,
			-8 => Self::JsonParse,
			-9 => Self::Deserialize,
			_ => Self::Instance(format!("unknown error code {code}")),
		}
	}
}

pub type Result<T> = std::result::Result<T, Error>;
