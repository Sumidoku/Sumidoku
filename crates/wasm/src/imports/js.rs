//! M4 前的桩：webview_* 在服务器上本无 WebView 语义，js context 到 M4 实装。
use crate::{
	WasmEnv,
	store::{FFIResult, Ptr, Rid},
};
use wasmer::FunctionEnvMut;

const UNIMPLEMENTED: i32 = -1;

pub fn context_create(_env: FunctionEnvMut<WasmEnv>) -> Rid {
	UNIMPLEMENTED
}

pub fn context_eval(
	_env: FunctionEnvMut<WasmEnv>,
	_rid: Rid,
	_string_ptr: Ptr,
	_len: u32,
) -> FFIResult {
	UNIMPLEMENTED
}

pub fn context_eval_async(
	_env: FunctionEnvMut<WasmEnv>,
	_context: Rid,
	_string_ptr: Ptr,
	_len: u32,
) -> FFIResult {
	UNIMPLEMENTED
}

pub fn context_get(
	_env: FunctionEnvMut<WasmEnv>,
	_rid: Rid,
	_string_ptr: Ptr,
	_len: u32,
) -> FFIResult {
	UNIMPLEMENTED
}

pub fn webview_create(_env: FunctionEnvMut<WasmEnv>) -> Rid {
	UNIMPLEMENTED
}

pub fn webview_set_rule_list(
	_env: FunctionEnvMut<WasmEnv>,
	_webview: Rid,
	_string_ptr: Ptr,
	_len: u32,
) -> FFIResult {
	UNIMPLEMENTED
}

pub fn webview_load(_env: FunctionEnvMut<WasmEnv>, _webview: Rid, _request: Rid) -> FFIResult {
	UNIMPLEMENTED
}

pub fn webview_load_html(
	_env: FunctionEnvMut<WasmEnv>,
	_webview: Rid,
	_string_ptr: Ptr,
	_len: u32,
	_url_ptr: Ptr,
	_url_len: u32,
) -> FFIResult {
	UNIMPLEMENTED
}

pub fn webview_wait_for_load(_env: FunctionEnvMut<WasmEnv>, _webview: Rid) -> FFIResult {
	UNIMPLEMENTED
}

pub fn webview_eval(
	_env: FunctionEnvMut<WasmEnv>,
	_webview: Rid,
	_string_ptr: Ptr,
	_len: u32,
) -> FFIResult {
	UNIMPLEMENTED
}

pub fn webview_eval_async(
	_env: FunctionEnvMut<WasmEnv>,
	_webview: Rid,
	_string_ptr: Ptr,
	_len: u32,
) -> FFIResult {
	UNIMPLEMENTED
}

pub fn webview_add_user_script(
	_env: FunctionEnvMut<WasmEnv>,
	_webview: Rid,
	_string_ptr: Ptr,
	_len: u32,
	_at_document_end: i32,
	_for_main_frame_only: i32,
) -> FFIResult {
	UNIMPLEMENTED
}

pub fn webview_get_cookies(_env: FunctionEnvMut<WasmEnv>, _webview: Rid) -> FFIResult {
	UNIMPLEMENTED
}

pub fn webview_delete_cookie(
	_env: FunctionEnvMut<WasmEnv>,
	_webview: Rid,
	_name_ptr: Ptr,
	_name_len: u32,
	_value_ptr: Ptr,
	_value_len: u32,
	_domain_ptr: Ptr,
	_domain_len: u32,
) -> FFIResult {
	UNIMPLEMENTED
}
