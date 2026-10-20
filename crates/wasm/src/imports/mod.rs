mod canvas;
mod defaults;
mod env;
pub(crate) mod html;
mod js;
mod net;
mod std;

pub use net::{CookieJar, HttpMethod, NetRequest};

use crate::WasmEnv;
use crate::runtime::store::{Ptr, Rid};
use wasmer::*;

pub fn generate_imports(store: &mut Store, env: &FunctionEnv<WasmEnv>) -> Imports {
	imports! {
		"env" => {
			"abort" => Function::new_typed_with_env(store, env, |mut c: FunctionEnvMut<WasmEnv>| { env::abort(&mut c); }),
			"print" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, ptr: Ptr, len: u32| { env::print(&mut c, ptr, len); },
			),
			"sleep" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, seconds: i32| { env::sleep(&mut c, seconds); },
			),
			"send_partial_result" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, value: i32| { env::send_partial_result(&mut c, value); },
			),
		},
		"std" => {
			"destroy" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { std::destroy(&mut c, rid); },
			),
			"buffer_len" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { std::buffer_len(&mut c, rid) },
			),
			"read_buffer" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, ptr: Ptr, size: u32| { std::read_buffer(&mut c, rid, ptr, size) },
			),
			"current_date" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>| { std::current_date(&mut c) },
			),
			"utc_offset" => Function::new_typed_with_env(store, env, |mut c: FunctionEnvMut<WasmEnv>| { std::utc_offset(&mut c) }),
			"parse_date" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, date_str: Ptr, date_len: u32, format_str: Ptr, format_len: u32, locale_str: Ptr, locale_len: u32, timezone_str: Ptr, timezone_len: u32| { std::parse_date(&mut c, date_str, date_len, format_str, format_len, locale_str, locale_len, timezone_str, timezone_len) },
			),
		},
		"defaults" => {
			"get" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, key_ptr: Ptr, len: u32| { defaults::get(&mut c, key_ptr, len) },
			),
			"set" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, key_ptr: Ptr, len: u32, kind: u8, value_ptr: Ptr| { defaults::set(&mut c, key_ptr, len, kind, value_ptr) },
			),
		},
		"canvas" => {
			"new_context" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, width: f32, height: f32| { canvas::new_context(&mut c, width, height) },
			),
			"set_transform" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, context: Rid, translate_x: f32, translate_y: f32, scale_x: f32, scale_y: f32, rotate_angle: f32| { canvas::set_transform(&mut c, context, translate_x, translate_y, scale_x, scale_y, rotate_angle) },
			),
			"draw_image" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, context: Rid, image: Rid, dst_x: f32, dst_y: f32, dst_width: f32, dst_height: f32| { canvas::draw_image(&mut c, context, image, dst_x, dst_y, dst_width, dst_height) },
			),
			"copy_image" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, context: Rid, image: Rid, src_x: f32, src_y: f32, src_width: f32, src_height: f32, dst_x: f32, dst_y: f32, dst_width: f32, dst_height: f32| { canvas::copy_image(&mut c, context, image, src_x, src_y, src_width, src_height, dst_x, dst_y, dst_width, dst_height) },
			),
			"fill" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, context: Rid, path_ptr: Ptr, r: f32, g: f32, b: f32, a: f32| { canvas::fill(&mut c, context, path_ptr, r, g, b, a) },
			),
			"stroke" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, context: Rid, path_ptr: Ptr, style_ptr: Ptr| { canvas::stroke(&mut c, context, path_ptr, style_ptr) },
			),
			"draw_text" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, context: Rid, text_ptr: Ptr, text_len: u32, size: f32, x: f32, y: f32, font: Rid, r: f32, g: f32, b: f32, a: f32| { canvas::draw_text(&mut c, context, text_ptr, text_len, size, x, y, font, r, g, b, a) },
			),
			"get_image" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, context: Rid| { canvas::get_image(&mut c, context) },
			),
			"new_font" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, name_ptr: Ptr, name_len: u32| { canvas::new_font(&mut c, name_ptr, name_len) },
			),
			"system_font" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, weight: u8| { canvas::system_font(&mut c, weight) },
			),
			"load_font" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, url_ptr: Ptr, url_len: u32| { canvas::load_font(&mut c, url_ptr, url_len) },
			),
			"new_image" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, data_ptr: Ptr, data_len: u32| { canvas::new_image(&mut c, data_ptr, data_len) },
			),
			"get_image_data" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, image_rid: Rid| { canvas::get_image_data(&mut c, image_rid) },
			),
			"get_image_width" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, image_rid: Rid| { canvas::get_image_width(&mut c, image_rid) },
			),
			"get_image_height" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, image_rid: Rid| { canvas::get_image_height(&mut c, image_rid) },
			),
		},
		"js" => {
			"context_create" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>| { js::context_create(&mut c) },
			),
			"context_eval" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, string_ptr: Ptr, len: u32| { js::context_eval(&mut c, rid, string_ptr, len) },
			),
			"context_eval_async" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, string_ptr: Ptr, len: u32| { js::context_eval_async(&mut c, rid, string_ptr, len) },
			),
			"context_get" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, string_ptr: Ptr, len: u32| { js::context_get(&mut c, rid, string_ptr, len) },
			),
			"webview_create" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>| { js::webview_create(&mut c) },
			),
			"webview_set_rule_list" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, webview: Rid, _string_ptr: Ptr, _len: u32| { js::webview_set_rule_list(&mut c, webview, _string_ptr, _len) },
			),
			"webview_load" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, webview: Rid, request: Rid| { js::webview_load(&mut c, webview, request) },
			),
			"webview_load_html" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, webview: Rid, string_ptr: Ptr, len: u32, url_ptr: Ptr, url_len: u32| { js::webview_load_html(&mut c, webview, string_ptr, len, url_ptr, url_len) },
			),
			"webview_wait_for_load" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, webview: Rid| { js::webview_wait_for_load(&mut c, webview) },
			),
			"webview_eval" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, webview: Rid, string_ptr: Ptr, len: u32| { js::webview_eval(&mut c, webview, string_ptr, len) },
			),
			"webview_eval_async" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, webview: Rid, string_ptr: Ptr, len: u32| { js::webview_eval_async(&mut c, webview, string_ptr, len) },
			),
			"webview_add_user_script" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, webview: Rid, string_ptr: Ptr, len: u32, at_document_end: i32, _for_main_frame_only: i32| { js::webview_add_user_script(&mut c, webview, string_ptr, len, at_document_end, _for_main_frame_only) },
			),
			"webview_get_cookies" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, webview: Rid| { js::webview_get_cookies(&mut c, webview) },
			),
			"webview_delete_cookie" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, webview: Rid, name_ptr: Ptr, name_len: u32, value_ptr: Ptr, value_len: u32, domain_ptr: Ptr, domain_len: u32| { js::webview_delete_cookie(&mut c, webview, name_ptr, name_len, value_ptr, value_len, domain_ptr, domain_len) },
			),
		},
		"html" => {
			"parse" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, html_ptr: Ptr, html_len: u32, base_url_ptr: Ptr, base_url_len: u32| { html::parse(&mut c, html_ptr, html_len, base_url_ptr, base_url_len) },
			),
			"parse_fragment" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, html_ptr: Ptr, html_len: u32, base_url_ptr: Ptr, base_url_len: u32| { html::parse_fragment(&mut c, html_ptr, html_len, base_url_ptr, base_url_len) },
			),
			"escape" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, text_ptr: Ptr, text_len: u32| { html::escape(&mut c, text_ptr, text_len) },
			),
			"unescape" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, text_ptr: Ptr, text_len: u32| { html::unescape(&mut c, text_ptr, text_len) },
			),
			"kind" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { html::kind(&mut c, rid) },
			),
			"child_nodes" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { html::child_nodes(&mut c, rid) },
			),
			"has_attr" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, attr_ptr: Ptr, attr_len: u32| { html::has_attr(&mut c, rid, attr_ptr, attr_len) },
			),
			"set_attr" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, key_ptr: Ptr, key_len: u32, value_ptr: Ptr, value_len: u32| { html::set_attr(&mut c, rid, key_ptr, key_len, value_ptr, value_len) },
			),
			"remove_attr" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, attr_ptr: Ptr, attr_len: u32| { html::remove_attr(&mut c, rid, attr_ptr, attr_len) },
			),
			"set_text" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, text_ptr: Ptr, text_len: u32| { html::set_text(&mut c, rid, text_ptr, text_len) },
			),
			"set_html" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, html_ptr: Ptr, html_len: u32| { html::set_html(&mut c, rid, html_ptr, html_len) },
			),
			"prepend" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, html_ptr: Ptr, html_len: u32| { html::prepend(&mut c, rid, html_ptr, html_len) },
			),
			"append" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, html_ptr: Ptr, html_len: u32| { html::append(&mut c, rid, html_ptr, html_len) },
			),
			"children" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { html::children(&mut c, rid) },
			),
			"base_uri" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { html::base_uri(&mut c, rid) },
			),
			"own_text" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { html::own_text(&mut c, rid) },
			),
			"data" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { html::data(&mut c, rid) },
			),
			"id" => Function::new_typed_with_env(store, env, |mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { html::id(&mut c, rid) }),
			"tag_name" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { html::tag_name(&mut c, rid) },
			),
			"class_name" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { html::class_name(&mut c, rid) },
			),
			"has_class" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, class_ptr: Ptr, class_len: u32| { html::has_class(&mut c, rid, class_ptr, class_len) },
			),
			"add_class" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, class_ptr: Ptr, class_len: u32| { html::add_class(&mut c, rid, class_ptr, class_len) },
			),
			"remove_class" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, class_ptr: Ptr, class_len: u32| { html::remove_class(&mut c, rid, class_ptr, class_len) },
			),
			"parent" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { html::parent(&mut c, rid) },
			),
			"siblings" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { html::siblings(&mut c, rid) },
			),
			"next" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { html::next(&mut c, rid) },
			),
			"previous" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { html::previous(&mut c, rid) },
			),
			"first" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { html::first(&mut c, rid) },
			),
			"last" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { html::last(&mut c, rid) },
			),
			"get" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, index: u32| { html::get(&mut c, rid, index) },
			),
			"size" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { html::size(&mut c, rid) },
			),
			"attr" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, key_ptr: u32, key_len: u32| { html::attr(&mut c, rid, key_ptr, key_len) },
			),
			"outer_html" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { html::outer_html(&mut c, rid) },
			),
			"remove" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { html::remove(&mut c, rid) },
			),
			"select" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, query_ptr: Ptr, query_len: u32| { html::select(&mut c, rid, query_ptr, query_len) },
			),
			"select_first" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, query_ptr: Ptr, query_len: u32| { html::select_first(&mut c, rid, query_ptr, query_len) },
			),
			"text" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { html::text(&mut c, rid) },
			),
			"untrimmed_text" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { html::untrimmed_text(&mut c, rid) },
			),
			"html" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { html::html(&mut c, rid) },
			),
		},
		"net" => {
			"init" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, method: u8| { net::init(&mut c, method) },
			),
			"send" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { net::send(&mut c, rid) },
			),
			"send_all" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid_ptr: Ptr, len: u32| { net::send_all(&mut c, rid_ptr, len) },
			),
			"set_url" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, ptr: Ptr, len: u32| { net::set_url(&mut c, rid, ptr, len) },
			),
			"set_header" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, key_ptr: Ptr, key_len: u32, val_ptr: Ptr, val_len: u32| { net::set_header(&mut c, rid, key_ptr, key_len, val_ptr, val_len) },
			),
			"set_body" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, ptr: Ptr, len: u32| { net::set_body(&mut c, rid, ptr, len) },
			),
			"set_timeout" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, value: f64| { net::set_timeout(&mut c, rid, value) },
			),
			"data_len" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { net::data_len(&mut c, rid) },
			),
			"read_data" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, buffer: Ptr, size: u32| { net::read_data(&mut c, rid, buffer, size) },
			),
			"get_image" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { net::get_image(&mut c, rid) },
			),
			"get_status_code" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { net::get_status_code(&mut c, rid) },
			),
			"get_url" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { net::get_url(&mut c, rid) },
			),
			"get_header" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid, key_ptr: Ptr, key_len: u32| { net::get_header(&mut c, rid, key_ptr, key_len) },
			),
			"html" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, rid: Rid| { net::html(&mut c, rid) },
			),
			"set_rate_limit" => Function::new_typed_with_env(
				store, env,
				|mut c: FunctionEnvMut<WasmEnv>, permits: i32, period: i32, unit: i32| { net::set_rate_limit(&mut c, permits, period, unit); },
			),
		},
	}
}
