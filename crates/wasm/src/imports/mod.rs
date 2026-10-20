pub(crate) mod canvas;
mod defaults;
mod env;
pub(crate) mod html;
mod js;
mod net;
mod std;

pub use net::{CookieJar, HttpMethod, NetRequest};

use crate::WasmEnv;
use crate::runtime::env::WasmiCtx;
use crate::runtime::store::{Ptr, Rid};
use wasmi::{Caller, Linker};

/// Register all source ABI imports: the closures only do the Caller→Ctx engine plumbing; the bodies all live in the shared Ctx layer
pub fn register_imports(linker: &mut Linker<WasmEnv>) -> Result<(), wasmi::Error> {
	linker.func_wrap("env", "abort", |mut c: Caller<'_, WasmEnv>| { env::abort(&mut WasmiCtx::new(&mut c)); })?;
	linker.func_wrap(
		"env", "print",
		|mut c: Caller<'_, WasmEnv>, ptr: Ptr, len: u32| { env::print(&mut WasmiCtx::new(&mut c), ptr, len); },
	)?;
	linker.func_wrap(
		"env", "sleep",
		|mut c: Caller<'_, WasmEnv>, seconds: i32| { env::sleep(&mut WasmiCtx::new(&mut c), seconds); },
	)?;
	linker.func_wrap(
		"env", "send_partial_result",
		|mut c: Caller<'_, WasmEnv>, value: i32| { env::send_partial_result(&mut WasmiCtx::new(&mut c), value); },
	)?;
	linker.func_wrap(
		"std", "destroy",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { std::destroy(&mut WasmiCtx::new(&mut c), rid); },
	)?;
	linker.func_wrap(
		"std", "buffer_len",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { std::buffer_len(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"std", "read_buffer",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, ptr: Ptr, size: u32| { std::read_buffer(&mut WasmiCtx::new(&mut c), rid, ptr, size) },
	)?;
	linker.func_wrap(
		"std", "current_date",
		|mut c: Caller<'_, WasmEnv>| { std::current_date(&mut WasmiCtx::new(&mut c)) },
	)?;
	linker.func_wrap("std", "utc_offset", |mut c: Caller<'_, WasmEnv>| { std::utc_offset(&mut WasmiCtx::new(&mut c)) })?;
	linker.func_wrap(
		"std", "parse_date",
		|mut c: Caller<'_, WasmEnv>, date_str: Ptr, date_len: u32, format_str: Ptr, format_len: u32, locale_str: Ptr, locale_len: u32, timezone_str: Ptr, timezone_len: u32| { std::parse_date(&mut WasmiCtx::new(&mut c), date_str, date_len, format_str, format_len, locale_str, locale_len, timezone_str, timezone_len) },
	)?;
	linker.func_wrap(
		"defaults", "get",
		|mut c: Caller<'_, WasmEnv>, key_ptr: Ptr, len: u32| { defaults::get(&mut WasmiCtx::new(&mut c), key_ptr, len) },
	)?;
	linker.func_wrap(
		"defaults", "set",
		|mut c: Caller<'_, WasmEnv>, key_ptr: Ptr, len: u32, kind: i32, value_ptr: Ptr| { defaults::set(&mut WasmiCtx::new(&mut c), key_ptr, len, kind, value_ptr) },
	)?;
	linker.func_wrap(
		"canvas", "new_context",
		|mut c: Caller<'_, WasmEnv>, width: f32, height: f32| { canvas::new_context(&mut WasmiCtx::new(&mut c), width, height) },
	)?;
	linker.func_wrap(
		"canvas", "set_transform",
		|mut c: Caller<'_, WasmEnv>, context: Rid, translate_x: f32, translate_y: f32, scale_x: f32, scale_y: f32, rotate_angle: f32| { canvas::set_transform(&mut WasmiCtx::new(&mut c), context, translate_x, translate_y, scale_x, scale_y, rotate_angle) },
	)?;
	linker.func_wrap(
		"canvas", "draw_image",
		|mut c: Caller<'_, WasmEnv>, context: Rid, image: Rid, dst_x: f32, dst_y: f32, dst_width: f32, dst_height: f32| { canvas::draw_image(&mut WasmiCtx::new(&mut c), context, image, dst_x, dst_y, dst_width, dst_height) },
	)?;
	linker.func_wrap(
		"canvas", "copy_image",
		|mut c: Caller<'_, WasmEnv>, context: Rid, image: Rid, src_x: f32, src_y: f32, src_width: f32, src_height: f32, dst_x: f32, dst_y: f32, dst_width: f32, dst_height: f32| { canvas::copy_image(&mut WasmiCtx::new(&mut c), context, image, src_x, src_y, src_width, src_height, dst_x, dst_y, dst_width, dst_height) },
	)?;
	linker.func_wrap(
		"canvas", "fill",
		|mut c: Caller<'_, WasmEnv>, context: Rid, path_ptr: Ptr, r: f32, g: f32, b: f32, a: f32| { canvas::fill(&mut WasmiCtx::new(&mut c), context, path_ptr, r, g, b, a) },
	)?;
	linker.func_wrap(
		"canvas", "stroke",
		|mut c: Caller<'_, WasmEnv>, context: Rid, path_ptr: Ptr, style_ptr: Ptr| { canvas::stroke(&mut WasmiCtx::new(&mut c), context, path_ptr, style_ptr) },
	)?;
	linker.func_wrap(
		"canvas", "draw_text",
		|mut c: Caller<'_, WasmEnv>, context: Rid, text_ptr: Ptr, text_len: u32, size: f32, x: f32, y: f32, font: Rid, r: f32, g: f32, b: f32, a: f32| { canvas::draw_text(&mut WasmiCtx::new(&mut c), context, text_ptr, text_len, size, x, y, font, r, g, b, a) },
	)?;
	linker.func_wrap(
		"canvas", "get_image",
		|mut c: Caller<'_, WasmEnv>, context: Rid| { canvas::get_image(&mut WasmiCtx::new(&mut c), context) },
	)?;
	linker.func_wrap(
		"canvas", "new_font",
		|mut c: Caller<'_, WasmEnv>, name_ptr: Ptr, name_len: u32| { canvas::new_font(&mut WasmiCtx::new(&mut c), name_ptr, name_len) },
	)?;
	linker.func_wrap(
		"canvas", "system_font",
		|mut c: Caller<'_, WasmEnv>, weight: i32| { canvas::system_font(&mut WasmiCtx::new(&mut c), weight) },
	)?;
	linker.func_wrap(
		"canvas", "load_font",
		|mut c: Caller<'_, WasmEnv>, url_ptr: Ptr, url_len: u32| { canvas::load_font(&mut WasmiCtx::new(&mut c), url_ptr, url_len) },
	)?;
	linker.func_wrap(
		"canvas", "new_image",
		|mut c: Caller<'_, WasmEnv>, data_ptr: Ptr, data_len: u32| { canvas::new_image(&mut WasmiCtx::new(&mut c), data_ptr, data_len) },
	)?;
	linker.func_wrap(
		"canvas", "get_image_data",
		|mut c: Caller<'_, WasmEnv>, image_rid: Rid| { canvas::get_image_data(&mut WasmiCtx::new(&mut c), image_rid) },
	)?;
	linker.func_wrap(
		"canvas", "get_image_width",
		|mut c: Caller<'_, WasmEnv>, image_rid: Rid| { canvas::get_image_width(&mut WasmiCtx::new(&mut c), image_rid) },
	)?;
	linker.func_wrap(
		"canvas", "get_image_height",
		|mut c: Caller<'_, WasmEnv>, image_rid: Rid| { canvas::get_image_height(&mut WasmiCtx::new(&mut c), image_rid) },
	)?;
	linker.func_wrap(
		"js", "context_create",
		|mut c: Caller<'_, WasmEnv>| { js::context_create(&mut WasmiCtx::new(&mut c)) },
	)?;
	linker.func_wrap(
		"js", "context_eval",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, string_ptr: Ptr, len: u32| { js::context_eval(&mut WasmiCtx::new(&mut c), rid, string_ptr, len) },
	)?;
	linker.func_wrap(
		"js", "context_eval_async",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, string_ptr: Ptr, len: u32| { js::context_eval_async(&mut WasmiCtx::new(&mut c), rid, string_ptr, len) },
	)?;
	linker.func_wrap(
		"js", "context_get",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, string_ptr: Ptr, len: u32| { js::context_get(&mut WasmiCtx::new(&mut c), rid, string_ptr, len) },
	)?;
	linker.func_wrap(
		"js", "webview_create",
		|mut c: Caller<'_, WasmEnv>| { js::webview_create(&mut WasmiCtx::new(&mut c)) },
	)?;
	linker.func_wrap(
		"js", "webview_set_rule_list",
		|mut c: Caller<'_, WasmEnv>, webview: Rid, _string_ptr: Ptr, _len: u32| { js::webview_set_rule_list(&mut WasmiCtx::new(&mut c), webview, _string_ptr, _len) },
	)?;
	linker.func_wrap(
		"js", "webview_load",
		|mut c: Caller<'_, WasmEnv>, webview: Rid, request: Rid| { js::webview_load(&mut WasmiCtx::new(&mut c), webview, request) },
	)?;
	linker.func_wrap(
		"js", "webview_load_html",
		|mut c: Caller<'_, WasmEnv>, webview: Rid, string_ptr: Ptr, len: u32, url_ptr: Ptr, url_len: u32| { js::webview_load_html(&mut WasmiCtx::new(&mut c), webview, string_ptr, len, url_ptr, url_len) },
	)?;
	linker.func_wrap(
		"js", "webview_wait_for_load",
		|mut c: Caller<'_, WasmEnv>, webview: Rid| { js::webview_wait_for_load(&mut WasmiCtx::new(&mut c), webview) },
	)?;
	linker.func_wrap(
		"js", "webview_eval",
		|mut c: Caller<'_, WasmEnv>, webview: Rid, string_ptr: Ptr, len: u32| { js::webview_eval(&mut WasmiCtx::new(&mut c), webview, string_ptr, len) },
	)?;
	linker.func_wrap(
		"js", "webview_eval_async",
		|mut c: Caller<'_, WasmEnv>, webview: Rid, string_ptr: Ptr, len: u32| { js::webview_eval_async(&mut WasmiCtx::new(&mut c), webview, string_ptr, len) },
	)?;
	linker.func_wrap(
		"js", "webview_add_user_script",
		|mut c: Caller<'_, WasmEnv>, webview: Rid, string_ptr: Ptr, len: u32, at_document_end: i32, _for_main_frame_only: i32| { js::webview_add_user_script(&mut WasmiCtx::new(&mut c), webview, string_ptr, len, at_document_end, _for_main_frame_only) },
	)?;
	linker.func_wrap(
		"js", "webview_get_cookies",
		|mut c: Caller<'_, WasmEnv>, webview: Rid| { js::webview_get_cookies(&mut WasmiCtx::new(&mut c), webview) },
	)?;
	linker.func_wrap(
		"js", "webview_delete_cookie",
		|mut c: Caller<'_, WasmEnv>, webview: Rid, name_ptr: Ptr, name_len: u32, value_ptr: Ptr, value_len: u32, domain_ptr: Ptr, domain_len: u32| { js::webview_delete_cookie(&mut WasmiCtx::new(&mut c), webview, name_ptr, name_len, value_ptr, value_len, domain_ptr, domain_len) },
	)?;
	linker.func_wrap(
		"html", "parse",
		|mut c: Caller<'_, WasmEnv>, html_ptr: Ptr, html_len: u32, base_url_ptr: Ptr, base_url_len: u32| { html::parse(&mut WasmiCtx::new(&mut c), html_ptr, html_len, base_url_ptr, base_url_len) },
	)?;
	linker.func_wrap(
		"html", "parse_fragment",
		|mut c: Caller<'_, WasmEnv>, html_ptr: Ptr, html_len: u32, base_url_ptr: Ptr, base_url_len: u32| { html::parse_fragment(&mut WasmiCtx::new(&mut c), html_ptr, html_len, base_url_ptr, base_url_len) },
	)?;
	linker.func_wrap(
		"html", "escape",
		|mut c: Caller<'_, WasmEnv>, text_ptr: Ptr, text_len: u32| { html::escape(&mut WasmiCtx::new(&mut c), text_ptr, text_len) },
	)?;
	linker.func_wrap(
		"html", "unescape",
		|mut c: Caller<'_, WasmEnv>, text_ptr: Ptr, text_len: u32| { html::unescape(&mut WasmiCtx::new(&mut c), text_ptr, text_len) },
	)?;
	linker.func_wrap(
		"html", "kind",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { html::kind(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"html", "child_nodes",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { html::child_nodes(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"html", "has_attr",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, attr_ptr: Ptr, attr_len: u32| { html::has_attr(&mut WasmiCtx::new(&mut c), rid, attr_ptr, attr_len) },
	)?;
	linker.func_wrap(
		"html", "set_attr",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, key_ptr: Ptr, key_len: u32, value_ptr: Ptr, value_len: u32| { html::set_attr(&mut WasmiCtx::new(&mut c), rid, key_ptr, key_len, value_ptr, value_len) },
	)?;
	linker.func_wrap(
		"html", "remove_attr",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, attr_ptr: Ptr, attr_len: u32| { html::remove_attr(&mut WasmiCtx::new(&mut c), rid, attr_ptr, attr_len) },
	)?;
	linker.func_wrap(
		"html", "set_text",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, text_ptr: Ptr, text_len: u32| { html::set_text(&mut WasmiCtx::new(&mut c), rid, text_ptr, text_len) },
	)?;
	linker.func_wrap(
		"html", "set_html",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, html_ptr: Ptr, html_len: u32| { html::set_html(&mut WasmiCtx::new(&mut c), rid, html_ptr, html_len) },
	)?;
	linker.func_wrap(
		"html", "prepend",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, html_ptr: Ptr, html_len: u32| { html::prepend(&mut WasmiCtx::new(&mut c), rid, html_ptr, html_len) },
	)?;
	linker.func_wrap(
		"html", "append",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, html_ptr: Ptr, html_len: u32| { html::append(&mut WasmiCtx::new(&mut c), rid, html_ptr, html_len) },
	)?;
	linker.func_wrap(
		"html", "children",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { html::children(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"html", "base_uri",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { html::base_uri(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"html", "own_text",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { html::own_text(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"html", "data",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { html::data(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap("html", "id", |mut c: Caller<'_, WasmEnv>, rid: Rid| { html::id(&mut WasmiCtx::new(&mut c), rid) })?;
	linker.func_wrap(
		"html", "tag_name",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { html::tag_name(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"html", "class_name",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { html::class_name(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"html", "has_class",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, class_ptr: Ptr, class_len: u32| { html::has_class(&mut WasmiCtx::new(&mut c), rid, class_ptr, class_len) },
	)?;
	linker.func_wrap(
		"html", "add_class",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, class_ptr: Ptr, class_len: u32| { html::add_class(&mut WasmiCtx::new(&mut c), rid, class_ptr, class_len) },
	)?;
	linker.func_wrap(
		"html", "remove_class",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, class_ptr: Ptr, class_len: u32| { html::remove_class(&mut WasmiCtx::new(&mut c), rid, class_ptr, class_len) },
	)?;
	linker.func_wrap(
		"html", "parent",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { html::parent(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"html", "siblings",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { html::siblings(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"html", "next",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { html::next(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"html", "previous",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { html::previous(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"html", "first",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { html::first(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"html", "last",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { html::last(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"html", "get",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, index: u32| { html::get(&mut WasmiCtx::new(&mut c), rid, index) },
	)?;
	linker.func_wrap(
		"html", "size",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { html::size(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"html", "attr",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, key_ptr: u32, key_len: u32| { html::attr(&mut WasmiCtx::new(&mut c), rid, key_ptr, key_len) },
	)?;
	linker.func_wrap(
		"html", "outer_html",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { html::outer_html(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"html", "remove",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { html::remove(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"html", "select",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, query_ptr: Ptr, query_len: u32| { html::select(&mut WasmiCtx::new(&mut c), rid, query_ptr, query_len) },
	)?;
	linker.func_wrap(
		"html", "select_first",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, query_ptr: Ptr, query_len: u32| { html::select_first(&mut WasmiCtx::new(&mut c), rid, query_ptr, query_len) },
	)?;
	linker.func_wrap(
		"html", "text",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { html::text(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"html", "untrimmed_text",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { html::untrimmed_text(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"html", "html",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { html::html(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"net", "init",
		|mut c: Caller<'_, WasmEnv>, method: i32| { net::init(&mut WasmiCtx::new(&mut c), method) },
	)?;
	linker.func_wrap(
		"net", "send",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { net::send(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"net", "send_all",
		|mut c: Caller<'_, WasmEnv>, rid_ptr: Ptr, len: u32| { net::send_all(&mut WasmiCtx::new(&mut c), rid_ptr, len) },
	)?;
	linker.func_wrap(
		"net", "set_url",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, ptr: Ptr, len: u32| { net::set_url(&mut WasmiCtx::new(&mut c), rid, ptr, len) },
	)?;
	linker.func_wrap(
		"net", "set_header",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, key_ptr: Ptr, key_len: u32, val_ptr: Ptr, val_len: u32| { net::set_header(&mut WasmiCtx::new(&mut c), rid, key_ptr, key_len, val_ptr, val_len) },
	)?;
	linker.func_wrap(
		"net", "set_body",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, ptr: Ptr, len: u32| { net::set_body(&mut WasmiCtx::new(&mut c), rid, ptr, len) },
	)?;
	linker.func_wrap(
		"net", "set_timeout",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, value: f64| { net::set_timeout(&mut WasmiCtx::new(&mut c), rid, value) },
	)?;
	linker.func_wrap(
		"net", "data_len",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { net::data_len(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"net", "read_data",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, buffer: Ptr, size: u32| { net::read_data(&mut WasmiCtx::new(&mut c), rid, buffer, size) },
	)?;
	linker.func_wrap(
		"net", "get_image",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { net::get_image(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"net", "get_status_code",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { net::get_status_code(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"net", "get_url",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { net::get_url(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"net", "get_header",
		|mut c: Caller<'_, WasmEnv>, rid: Rid, key_ptr: Ptr, key_len: u32| { net::get_header(&mut WasmiCtx::new(&mut c), rid, key_ptr, key_len) },
	)?;
	linker.func_wrap(
		"net", "html",
		|mut c: Caller<'_, WasmEnv>, rid: Rid| { net::html(&mut WasmiCtx::new(&mut c), rid) },
	)?;
	linker.func_wrap(
		"net", "set_rate_limit",
		|mut c: Caller<'_, WasmEnv>, permits: i32, period: i32, unit: i32| { net::set_rate_limit(&mut WasmiCtx::new(&mut c), permits, period, unit); },
	)?;
	Ok(())
}
