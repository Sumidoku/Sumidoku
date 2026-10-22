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

	/// Rendering safe for server logs: guest and reqwest error text can embed full URLs
	/// (with credentials or query strings), so payloads are scrubbed down to the host and
	/// capped; payload-less variants are static classes by construction
	pub fn log_safe(&self) -> String {
		match self {
			Self::Message(msg) => format!("message: {}", log_safe_text(msg)),
			Self::Trap(msg) => format!("trap: {}", log_safe_text(msg)),
			Self::Instance(msg) => format!("instance: {}", log_safe_text(msg)),
			Self::Unsupported(feature) => format!("unsupported: {feature}"),
			Self::Unimplemented => "unimplemented".into(),
			Self::Request => "request".into(),
			Self::Html => "html".into(),
			Self::Js => "js".into(),
			Self::Canvas => "canvas".into(),
			Self::Utf8 => "utf8".into(),
			Self::JsonParse => "json_parse".into(),
			Self::Deserialize => "deserialize".into(),
			Self::Timeout => "timeout".into(),
		}
	}
}

static URL_RE: std::sync::LazyLock<regex::Regex> =
	std::sync::LazyLock::new(|| regex::Regex::new(r#"https?://[^\s"'<>\)\]]+"#).expect("url regex"));

const LOG_PAYLOAD_MAX: usize = 200;

pub fn log_safe_text(text: &str) -> String {
	let scrubbed = URL_RE.replace_all(text, |caps: &regex::Captures| {
		url::Url::parse(&caps[0])
			.ok()
			.and_then(|u| u.host_str().map(str::to_owned))
			.unwrap_or_else(|| "<url>".into())
	});
	truncate(&scrubbed)
}

fn truncate(text: &str) -> String {
	if text.chars().count() <= LOG_PAYLOAD_MAX {
		text.into()
	} else {
		text.chars().take(LOG_PAYLOAD_MAX).collect()
	}
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn log_safe_keeps_only_url_host() {
		let err = Error::Message("fetch https://user:pass@manga.example.com/secret/path?token=abc failed".into());
		let safe = err.log_safe();
		assert_eq!(safe, "message: fetch manga.example.com failed");
		assert!(!safe.contains("token"));
		assert!(!safe.contains("pass"));
	}

	#[test]
	fn log_safe_masks_unparseable_urls() {
		let err = Error::Instance("proxy http://:bad down".into());
		assert_eq!(err.log_safe(), "instance: proxy <url> down");
	}

	#[test]
	fn log_safe_caps_payload_length() {
		let err = Error::Trap("x".repeat(500));
		let safe = err.log_safe();
		assert_eq!(safe.len(), "trap: ".len() + LOG_PAYLOAD_MAX);
	}

	#[test]
	fn log_safe_payloadless_variants_are_class_only() {
		assert_eq!(Error::Timeout.log_safe(), "timeout");
		assert_eq!(Error::Request.log_safe(), "request");
		assert_eq!(Error::Unsupported("get_home").log_safe(), "unsupported: get_home");
	}
}
