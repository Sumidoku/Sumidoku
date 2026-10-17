//! M4 前的桩：让引用 canvas 的源能实例化；调用到即报错，与
//! PageContent::Image 在 M4 前按 unimplemented 处理的语义一致。
use crate::{
	WasmEnv,
	store::{FFIResult, Ptr, Rid},
};
use wasmer::FunctionEnvMut;

const UNIMPLEMENTED: i32 = -1;

pub fn new_context(_env: FunctionEnvMut<WasmEnv>, _width: f32, _height: f32) -> Rid {
	UNIMPLEMENTED
}

pub fn set_transform(
	_env: FunctionEnvMut<WasmEnv>,
	_context: Rid,
	_translate_x: f32,
	_translate_y: f32,
	_scale_x: f32,
	_scale_y: f32,
	_rotate_angle: f32,
) -> FFIResult {
	UNIMPLEMENTED
}

pub fn draw_image(
	_env: FunctionEnvMut<WasmEnv>,
	_context: Rid,
	_image: Rid,
	_dst_x: f32,
	_dst_y: f32,
	_dst_width: f32,
	_dst_height: f32,
) -> FFIResult {
	UNIMPLEMENTED
}

// 签名由源 ABI 固定，无法归并参数
#[allow(clippy::too_many_arguments)]
pub fn copy_image(
	_env: FunctionEnvMut<WasmEnv>,
	_context: Rid,
	_image: Rid,
	_src_x: f32,
	_src_y: f32,
	_src_width: f32,
	_src_height: f32,
	_dst_x: f32,
	_dst_y: f32,
	_dst_width: f32,
	_dst_height: f32,
) -> FFIResult {
	UNIMPLEMENTED
}

pub fn fill(
	_env: FunctionEnvMut<WasmEnv>,
	_context: Rid,
	_path_ptr: Ptr,
	_r: f32,
	_g: f32,
	_b: f32,
	_a: f32,
) -> FFIResult {
	UNIMPLEMENTED
}

pub fn stroke(
	_env: FunctionEnvMut<WasmEnv>,
	_context: Rid,
	_path_ptr: Ptr,
	_style_ptr: Ptr,
) -> FFIResult {
	UNIMPLEMENTED
}

#[allow(clippy::too_many_arguments)]
pub fn draw_text(
	_env: FunctionEnvMut<WasmEnv>,
	_context: Rid,
	_text_ptr: Ptr,
	_text_len: u32,
	_size: f32,
	_x: f32,
	_y: f32,
	_font: Rid,
	_r: f32,
	_g: f32,
	_b: f32,
	_a: f32,
) -> FFIResult {
	UNIMPLEMENTED
}

pub fn get_image(_env: FunctionEnvMut<WasmEnv>, _context: Rid) -> Rid {
	UNIMPLEMENTED
}

pub fn new_font(_env: FunctionEnvMut<WasmEnv>, _name_ptr: Ptr, _name_len: u32) -> FFIResult {
	UNIMPLEMENTED
}

pub fn system_font(_env: FunctionEnvMut<WasmEnv>, _weight: u8) -> Rid {
	UNIMPLEMENTED
}

pub fn load_font(_env: FunctionEnvMut<WasmEnv>, _url_ptr: Ptr, _url_len: u32) -> FFIResult {
	UNIMPLEMENTED
}

pub fn new_image(_env: FunctionEnvMut<WasmEnv>, _data_ptr: Ptr, _data_len: u32) -> FFIResult {
	UNIMPLEMENTED
}

pub fn get_image_data(_env: FunctionEnvMut<WasmEnv>, _image_rid: Rid) -> FFIResult {
	UNIMPLEMENTED
}

pub fn get_image_width(_env: FunctionEnvMut<WasmEnv>, _image_rid: Rid) -> f32 {
	-1.0
}

pub fn get_image_height(_env: FunctionEnvMut<WasmEnv>, _image_rid: Rid) -> f32 {
	-1.0
}
