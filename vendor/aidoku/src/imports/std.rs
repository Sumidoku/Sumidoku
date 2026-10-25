//! Module for standard Aidoku source library functions.
use super::{FFIResult, Ptr, Rid};
use crate::{
	AidokuError,
	alloc::{String, Vec},
};
use core::ptr::null;
use serde::{Serialize, de::DeserializeOwned};

#[link(wasm_import_module = "std")]
unsafe extern "C" {
	pub(crate) fn destroy(rid: Rid);

	pub(crate) fn buffer_len(rid: Rid) -> FFIResult;

	#[link_name = "read_buffer"]
	fn _read_buffer(rid: Rid, buf: *mut u8, len: usize) -> FFIResult;

	#[link_name = "current_date"]
	fn _current_date() -> f64;

	fn utc_offset() -> i64;

	#[link_name = "parse_date"]
	fn _parse_date(
		string_ptr: *const u8,
		string_len: usize,
		format_ptr: *const u8,
		format_len: usize,
		locale_ptr: *const u8,
		locale_len: usize,
		timezone_ptr: *const u8,
		timezone_len: usize,
	) -> f64;
}

// env module
#[link(wasm_import_module = "env")]
unsafe extern "C" {
	// #[link_name = "abort"]
	// fn _abort();

	#[link_name = "print"]
	fn _print(string: *const u8, size: usize);

	#[link_name = "sleep"]
	fn _sleep(seconds: i32);

	#[link_name = "send_partial_result"]
	fn _send_partial_result(value: Ptr);
}

/// Error type for std functions.
#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum StdError {
	InvalidDescriptor,
	InvalidBufferSize,
	FailedMemoryWrite,
	InvalidString,
	InvalidDateString,
}

impl StdError {
	fn from(value: i32) -> Option<Self> {
		match value {
			-1 => Some(Self::InvalidDescriptor),
			-2 => Some(Self::InvalidBufferSize),
			-3 => Some(Self::FailedMemoryWrite),
			-4 => Some(Self::InvalidString),
			-5 => Some(Self::InvalidDateString),
			_ => None,
		}
	}
}

/// Prints a message to the Aidoku logs.
pub fn print<T: AsRef<str>>(string: T) {
	let string = string.as_ref();
	unsafe {
		_print(string.as_ptr(), string.len());
	}
}

/// Blocks the current thread for the specified number of seconds.
pub fn sleep(seconds: i32) {
	unsafe {
		_sleep(seconds);
	}
}

/// Encodes a value into a byte array and returns a pointer to it.
///
/// Used for sending results back to Aidoku. The encoded data is prefixed with its length.
/// Note that the byte vector is forgotten after encoding, and must be manually freed (with [free_result]).
///
/// # Safety
/// The returned pointer is forgotten, and must be manually freed with [free_result].
pub(crate) unsafe fn encode<T: Serialize>(result: &T) -> Ptr {
	let mut bytes = ::postcard::to_allocvec(result).unwrap();
	bytes.splice(0..0, [0, 0, 0, 0, 0, 0, 0, 0]);
	let len_bytes = (bytes.len() as i32).to_le_bytes();
	bytes[0..4].copy_from_slice(&len_bytes);
	let cap_bytes = (bytes.capacity() as i32).to_le_bytes();
	bytes[4..8].copy_from_slice(&cap_bytes);
	let ptr = bytes.as_ptr() as Ptr;
	::core::mem::forget(bytes);
	ptr
}

/// Frees a byte array pointer returned by `encode`.
///
/// This function is exposed for the functions that the [register_source](crate::register_source)
/// macro generates and should not be used directly.
///
/// # Safety
/// The pointer must be a valid pointer to a byte array returned by `encode`.
pub unsafe fn free_result(ptr: Ptr) {
	let ptr = ptr as *const u8;
	let (len, cap) = unsafe {
		let cap_and_len = ::core::slice::from_raw_parts(ptr, 8);
		let len = i32::from_le_bytes([
			cap_and_len[0],
			cap_and_len[1],
			cap_and_len[2],
			cap_and_len[3],
		]);
		let cap = i32::from_le_bytes([
			cap_and_len[4],
			cap_and_len[5],
			cap_and_len[6],
			cap_and_len[7],
		]);
		if len == -1 {
			let real_len_slice = ::core::slice::from_raw_parts(ptr.offset(8), 4);
			let real_len = i32::from_le_bytes([
				real_len_slice[0],
				real_len_slice[1],
				real_len_slice[2],
				real_len_slice[3],
			]);
			(real_len, cap)
		} else {
			(len, cap)
		}
	};
	let original_vec: Vec<u8> =
		unsafe { Vec::from_raw_parts(ptr as *mut u8, len as usize, cap as usize) };
	drop(original_vec);
}

/// Sends a partial result to the source runner.
///
/// This function is used to send partial home layours and manga results
/// back to the runner for faster loading.
///
/// Only [HomePartialResult](crate::HomePartialResult) and [Manga](crate::Manga)
/// structs should be passed as arguments.
pub fn send_partial_result<T: Serialize>(value: &T) {
	let value_ptr = unsafe { encode(value) };
	unsafe {
		_send_partial_result(value_ptr);
	}
	unsafe { free_result(value_ptr) };
}

/// Gets the current time as a Unix timestamp.
pub fn current_date() -> i64 {
	unsafe { _current_date() as i64 }
}

/// Reads an object from a descriptor.
///
/// This function is exposed for the functions that the [register_source](crate::register_source)
/// macro generates and should not be used directly.
pub fn read<T: DeserializeOwned>(rid: Rid) -> Result<T, AidokuError> {
	read_buffer(rid)
		.and_then(|buffer| postcard::from_bytes(&buffer).ok())
		.ok_or(AidokuError::DeserializeError)
}

/// Reads a string from a descriptor.
///
/// This function is exposed for the functions that the [register_source](crate::register_source)
/// macro generates and should not be used directly.
pub fn read_string(rid: Rid) -> Option<String> {
	let buffer = read_buffer(rid)?;
	let string = String::from_utf8(buffer).unwrap_or_default();
	if string.is_empty() {
		None
	} else {
		Some(string)
	}
}

pub(crate) fn read_string_and_destroy(rid: Rid) -> Option<String> {
	let buffer = read_buffer(rid);
	unsafe { destroy(rid) };
	let buffer = buffer?;
	let string = String::from_utf8(buffer).unwrap_or_default();
	if string.is_empty() {
		None
	} else {
		Some(string)
	}
}

/// Reads a buffer from a descriptor.
pub(crate) fn read_buffer(rid: Rid) -> Option<Vec<u8>> {
	let len = unsafe { buffer_len(rid) };
	if len < 0 {
		return None;
	}
	let len = len as usize;
	let mut buffer = Vec::with_capacity(len);
	let error = unsafe { _read_buffer(rid, buffer.as_mut_ptr(), len) };
	// the values read are externally managed by the source runner, so we don't need to free them
	// unsafe { destroy(rid) };
	if error != 0 {
		return None;
	}
	unsafe { buffer.set_len(len) };
	Some(buffer)
}

pub fn get_utc_offset() -> i64 {
	unsafe { utc_offset() }
}

/// Parses a date string into a Unix timestamp (seconds since epoch) using the specified format.
///
/// The result will be in UTC. The format string should be valid for Swift's DateFormatter.
///
/// # Examples
///
/// ```ignore
/// use aidoku::imports::std::parse_date;
/// let timestamp = parse_date("07-01-2025 13:00", "MM-dd-yyyy HH:mm");
/// assert_eq!(timestamp, Some(1751374800));
/// ```
pub fn parse_date<T: AsRef<str>, U: AsRef<str>>(date_str: T, format: U) -> Option<i64> {
	let string = date_str.as_ref();
	let format = format.as_ref();
	let timezone = "UTC";
	let result = unsafe {
		_parse_date(
			string.as_ptr(),
			string.len(),
			format.as_ptr(),
			format.len(),
			null(),
			0,
			timezone.as_ptr(),
			timezone.len(),
		)
	};
	if StdError::from(result as i32).is_some() {
		return None;
	}
	Some(result as i64)
}

/// Parses a date string into a Unix timestamp using the specified format and the user's local timezone.
///
/// The format string should be valid for Swift's DateFormatter.
pub fn parse_local_date<T: AsRef<str>, U: AsRef<str>>(date_str: T, format: U) -> Option<i64> {
	let string = date_str.as_ref();
	let format = format.as_ref();
	let timezone = "current";
	let result = unsafe {
		_parse_date(
			string.as_ptr(),
			string.len(),
			format.as_ptr(),
			format.len(),
			null(),
			0,
			timezone.as_ptr(),
			timezone.len(),
		)
	};
	if StdError::from(result as i32).is_some() {
		return None;
	}
	Some(result as i64)
}

/// Parses a date string into a Unix timestamp using the specified format, locale, and timezone.
///
/// The format string should be valid for Swift's DateFormatter. The locale and timezone should be
/// identifiers accepted by Swift's Locale and TimeZone.
pub fn parse_date_with_options<T: AsRef<str>, U: AsRef<str>, V: AsRef<str>, W: AsRef<str>>(
	date_str: T,
	format: U,
	locale: V,
	timezone: W,
) -> Option<i64> {
	let string = date_str.as_ref();
	let format = format.as_ref();
	let locale = locale.as_ref();
	let timezone = timezone.as_ref();
	let result = unsafe {
		_parse_date(
			string.as_ptr(),
			string.len(),
			format.as_ptr(),
			format.len(),
			locale.as_ptr(),
			locale.len(),
			timezone.as_ptr(),
			timezone.len(),
		)
	};
	if StdError::from(result as i32).is_some() {
		return None;
	}
	Some(result as i64)
}

/// Link placeholders for non-wasm targets: host code can reach these externs
/// (e.g. `ImageRef::drop` reaches `destroy`), but descriptor bookkeeping there
/// is driven by the host runtime itself, so the stubs only satisfy the linker.
/// `get`, `get_image` and `html` are declared by two import modules each; off
/// wasm the import module is ignored, so one stub covers both declarations.
/// `send`, `select` and `remove` deliberately get no stub: a no-op under one
/// of those names gets pulled by the binary's own references to the real C
/// runtime/socket function and hijacks it process-wide (every socket write
/// binds to the stub); those names always resolve from the system libraries
/// std already links (libc, ws2_32, the UCRT).
#[cfg(not(target_arch = "wasm32"))]
mod host_stubs {
	#[unsafe(no_mangle)]
	pub extern "C" fn destroy(_rid: i32) {}

	#[unsafe(no_mangle)]
	pub extern "C" fn add_class() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn append() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn attr() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn base_uri() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn buffer_len() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn child_nodes() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn children() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn class_name() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn context_create() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn context_eval() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn context_eval_async() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn context_get() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn copy_image() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn current_date() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn data() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn data_len() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn draw_image() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn draw_text() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn escape() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn fill() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn first() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn get() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn get_header() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn get_image() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn get_image_data() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn get_image_height() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn get_image_width() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn get_status_code() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn get_url() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn has_attr() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn has_class() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn html() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn id() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn init() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn kind() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn last() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn load_font() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn new_context() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn new_font() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn new_image() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn next() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn outer_html() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn own_text() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn parent() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn parse() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn parse_date() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn parse_fragment() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn prepend() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn previous() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn print() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn read_buffer() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn read_data() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn remove_attr() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn remove_class() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn select_first() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn send_all() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn send_partial_result() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn set() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn set_attr() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn set_body() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn set_header() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn set_html() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn set_rate_limit() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn set_text() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn set_timeout() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn set_transform() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn set_url() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn siblings() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn size() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn stroke() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn system_font() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn tag_name() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn text() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn unescape() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn untrimmed_text() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn utc_offset() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn webview_add_user_script() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn webview_create() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn webview_delete_cookie() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn webview_eval() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn webview_eval_async() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn webview_get_cookies() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn webview_load() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn webview_load_html() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn webview_set_rule_list() {}
	#[unsafe(no_mangle)]
	pub extern "C" fn webview_wait_for_load() {}
}

/// `sleep` has no Windows provider (the UCRT exports only `_sleep`, and
/// `Sleep` is a different symbol), while unix libc always provides it — so the
/// no-op stub exists only where the link would otherwise fail, and unix builds
/// bind the aidoku call site to the real function instead.
#[cfg(all(not(target_arch = "wasm32"), windows))]
mod host_stubs_win {
	#[unsafe(no_mangle)]
	pub extern "C" fn sleep() {}
}
