//! canvas host 模块：tiny-skia 软渲染实装。
//! 语义对齐 aidoku-rs test-runner 的 raqote 版（transform = 平移·缩放·旋转、
//! 颜色分量 0-255/alpha 0-1、path/style 为 postcard 载荷），错误码以
//! vendor/aidoku/src/imports/canvas.rs 的 CanvasError 为准。
use crate::{
	WasmEnv, fonts,
	runtime::store::{CanvasState, FFIResult, ImageData, Ptr, Rid, StoreItem},
};
use tiny_skia::{
	Color, FillRule, FilterQuality, Paint, PathBuilder, Pattern, Pixmap, Rect, SpreadMode, Stroke,
	StrokeDash, Transform,
};
use wasmer::FunctionEnvMut;

/// 画布像素总量上限：长条拼接页可能上万高，按 64M 像素（256MB RGBA）封顶
const MAX_CANVAS_PIXELS: u64 = 64 * 1024 * 1024;
const MAX_CANVAS_DIM: f32 = 32767.0;

enum Result {
	Success,
	InvalidContext,
	InvalidImagePointer,
	InvalidImage,
	InvalidSrcRect,
	InvalidPath,
	InvalidStyle,
	InvalidString,
	InvalidFont,
	FontLoadFailed,
}

impl From<Result> for i32 {
	fn from(result: Result) -> i32 {
		match result {
			Result::Success => 0,
			Result::InvalidContext => -1,
			Result::InvalidImagePointer => -2,
			Result::InvalidImage => -3,
			Result::InvalidSrcRect => -4,
			Result::InvalidPath => -7,
			Result::InvalidStyle => -8,
			Result::InvalidString => -9,
			Result::InvalidFont => -10,
			Result::FontLoadFailed => -11,
		}
	}
}

fn color_u8(v: f32) -> u8 {
	v.clamp(0.0, 255.0) as u8
}

fn alpha_u8(v: f32) -> u8 {
	(v.clamp(0.0, 1.0) * 255.0) as u8
}

fn solid_paint(r: f32, g: f32, b: f32, a: f32) -> Paint<'static> {
	let mut paint = Paint {
		anti_alias: true,
		..Paint::default()
	};
	paint.set_color(Color::from_rgba8(color_u8(r), color_u8(g), color_u8(b), alpha_u8(a)));
	paint
}

fn premultiplied(data: &[u8]) -> Vec<u8> {
	let mut out = Vec::with_capacity(data.len());
	for px in data.as_chunks::<4>().0 {
		let a = px[3] as u32;
		// 不透明是绝大多数，直出省一轮乘除
		if a == 255 {
			out.extend_from_slice(px);
		} else {
			out.push(((px[0] as u32 * a + 127) / 255) as u8);
			out.push(((px[1] as u32 * a + 127) / 255) as u8);
			out.push(((px[2] as u32 * a + 127) / 255) as u8);
			out.push(a as u8);
		}
	}
	out
}

fn demultiplied(data: &[u8]) -> Vec<u8> {
	let mut out = Vec::with_capacity(data.len());
	for px in data.as_chunks::<4>().0 {
		let a = px[3] as u32;
		if a == 255 || a == 0 {
			out.extend_from_slice(px);
		} else {
			out.push(((px[0] as u32 * 255 + a / 2) / a) as u8);
			out.push(((px[1] as u32 * 255 + a / 2) / a) as u8);
			out.push(((px[2] as u32 * 255 + a / 2) / a) as u8);
			out.push(a as u8);
		}
	}
	out
}

fn image_pixmap(image: &ImageData) -> Option<Pixmap> {
	let size = tiny_skia::IntSize::from_wh(image.width as u32, image.height as u32)?;
	Pixmap::from_vec(premultiplied(&image.data), size)
}

/// arc 按圆心/半径/起扫角（弧度，y 轴向下时正扫角为顺时针）逼近成
/// ≤90° 的 cubic 段；路径已有当前点时先 line 到弧起点，与 raqote 一致
fn arc_to(pb: &mut PathBuilder, has_current: bool, cx: f32, cy: f32, radius: f32, start: f32, sweep: f32) {
	let start_x = cx + radius * start.cos();
	let start_y = cy + radius * start.sin();
	if has_current {
		pb.line_to(start_x, start_y);
	} else {
		pb.move_to(start_x, start_y);
	}
	let mut angle = start;
	let mut remaining = sweep;
	while remaining.abs() > 1e-6 {
		let step = remaining.clamp(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2);
		let next = angle + step;
		let k = 4.0 / 3.0 * (step / 4.0).tan() * radius;
		let (x0, y0) = (cx + radius * angle.cos(), cy + radius * angle.sin());
		let (x3, y3) = (cx + radius * next.cos(), cy + radius * next.sin());
		pb.cubic_to(
			x0 - k * angle.sin(),
			y0 + k * angle.cos(),
			x3 + k * next.sin(),
			y3 - k * next.cos(),
			x3,
			y3,
		);
		angle = next;
		remaining -= step;
	}
}

fn convert_path(path: &aidoku::canvas::Path) -> Option<tiny_skia::Path> {
	let mut pb = PathBuilder::new();
	let mut has_current = false;
	for op in &path.ops {
		match op {
			aidoku::canvas::PathOp::MoveTo(p) => {
				pb.move_to(p.x, p.y);
				has_current = true;
			}
			aidoku::canvas::PathOp::LineTo(p) => {
				pb.line_to(p.x, p.y);
				has_current = true;
			}
			aidoku::canvas::PathOp::QuadTo(to, control) => {
				pb.quad_to(control.x, control.y, to.x, to.y);
				has_current = true;
			}
			aidoku::canvas::PathOp::CubicTo(to, c1, c2) => {
				pb.cubic_to(c1.x, c1.y, c2.x, c2.y, to.x, to.y);
				has_current = true;
			}
			aidoku::canvas::PathOp::Arc(center, radius, start, sweep) => {
				arc_to(&mut pb, has_current, center.x, center.y, *radius, *start, *sweep);
				has_current = true;
			}
			aidoku::canvas::PathOp::Close => {
				pb.close();
				has_current = false;
			}
		}
	}
	pb.finish()
}

fn read_postcard<'a, T: serde::Deserialize<'a>>(bytes: &'a [u8]) -> Option<T> {
	postcard::from_bytes(bytes).ok()
}

/// pattern 变换是图像空间→用户空间（tiny-skia 内部取逆采样）：
/// 让 (src_x, src_y) 落到 (dst_x, dst_y)，(src 尺寸) 铺满 (dst 尺寸)
#[allow(clippy::too_many_arguments)]
fn blit(
	canvas: &mut CanvasState,
	image: &ImageData,
	src_x: f32,
	src_y: f32,
	src_w: f32,
	src_h: f32,
	dst_x: f32,
	dst_y: f32,
	dst_w: f32,
	dst_h: f32,
) {
	if dst_w <= 0.0 || dst_h <= 0.0 || src_w <= 0.0 || src_h <= 0.0 {
		return;
	}
	let Some(pixmap) = image_pixmap(image) else { return };
	let Some(rect) = Rect::from_xywh(dst_x, dst_y, dst_w, dst_h) else {
		return;
	};
	let shader = Pattern::new(
		pixmap.as_ref(),
		SpreadMode::Pad,
		FilterQuality::Bilinear,
		1.0,
		Transform::from_row(
			dst_w / src_w,
			0.0,
			0.0,
			dst_h / src_h,
			dst_x - src_x * dst_w / src_w,
			dst_y - src_y * dst_h / src_h,
		),
	);
	let paint = Paint {
		shader,
		anti_alias: true,
		..Paint::default()
	};
	canvas
		.pixmap
		.fill_rect(rect, &paint, canvas.transform, None);
}

pub fn new_context(mut env: FunctionEnvMut<WasmEnv>, width: f32, height: f32) -> Rid {
	if !width.is_finite()
		|| !height.is_finite()
		|| width <= 0.0
		|| height <= 0.0
		|| width > MAX_CANVAS_DIM
		|| height > MAX_CANVAS_DIM
		|| width as u64 * height as u64 > MAX_CANVAS_PIXELS
	{
		return Result::InvalidContext.into();
	}
	let Some(pixmap) = Pixmap::new(width as u32, height as u32) else {
		return Result::InvalidContext.into();
	};
	env.data_mut().store.store(StoreItem::Canvas(Box::new(CanvasState {
		pixmap,
		transform: Transform::identity(),
	})))
}

pub fn set_transform(
	mut env: FunctionEnvMut<WasmEnv>,
	context: Rid,
	translate_x: f32,
	translate_y: f32,
	scale_x: f32,
	scale_y: f32,
	rotate_angle: f32,
) -> FFIResult {
	let Some(canvas) = env
		.data_mut()
		.store
		.get_mut(context)
		.and_then(|item| item.as_canvas())
	else {
		return Result::InvalidContext.into();
	};
	// ABI 传的是源侧分解好的平移/缩放/旋转，按上游 T·S·R 顺序重组；
	// rotate_angle 单位是弧度，tiny-skia 按度计
	canvas.transform = Transform::from_translate(translate_x, translate_y)
		.post_scale(scale_x, scale_y)
		.post_rotate(rotate_angle.to_degrees());
	Result::Success.into()
}

/// take/restore 模式：canvas 与 image 可能同 store，先取出 image（所有权），
/// 画完放回原 rid；图被反复绘制时避免每次全量克隆像素
fn with_image(
	env: &mut FunctionEnvMut<WasmEnv>,
	context: Rid,
	image_rid: Rid,
	f: impl FnOnce(&mut CanvasState, &ImageData),
) -> FFIResult {
	let data = env.data_mut();
	let Some(StoreItem::ImageData(image)) = data.store.take(image_rid) else {
		if let Some(item) = data.store.take(image_rid) {
			data.store.restore(image_rid, item);
		}
		return Result::InvalidImagePointer.into();
	};
	let Some(canvas) = data.store.get_mut(context).and_then(|item| item.as_canvas()) else {
		data.store.restore(image_rid, StoreItem::ImageData(image));
		return Result::InvalidContext.into();
	};
	f(canvas, &image);
	data.store.restore(image_rid, StoreItem::ImageData(image));
	Result::Success.into()
}

pub fn draw_image(
	mut env: FunctionEnvMut<WasmEnv>,
	context: Rid,
	image: Rid,
	dst_x: f32,
	dst_y: f32,
	dst_width: f32,
	dst_height: f32,
) -> FFIResult {
	with_image(&mut env, context, image, |canvas, image| {
		blit(canvas, image, 0.0, 0.0, image.width as f32, image.height as f32, dst_x, dst_y, dst_width, dst_height);
	})
}

// 签名由源 ABI 固定，无法归并参数
#[allow(clippy::too_many_arguments)]
pub fn copy_image(
	mut env: FunctionEnvMut<WasmEnv>,
	context: Rid,
	image: Rid,
	src_x: f32,
	src_y: f32,
	src_width: f32,
	src_height: f32,
	dst_x: f32,
	dst_y: f32,
	dst_width: f32,
	dst_height: f32,
) -> FFIResult {
	if src_width <= 0.0 || src_height <= 0.0 {
		return Result::InvalidSrcRect.into();
	}
	with_image(&mut env, context, image, |canvas, image| {
		blit(canvas, image, src_x, src_y, src_width, src_height, dst_x, dst_y, dst_width, dst_height);
	})
}

pub fn fill(
	mut env: FunctionEnvMut<WasmEnv>,
	context: Rid,
	path_ptr: Ptr,
	r: f32,
	g: f32,
	b: f32,
	a: f32,
) -> FFIResult {
	let Some(path) = env
		.data()
		.read_item_bytes(&env, path_ptr)
		.ok()
		.and_then(|bytes| read_postcard::<aidoku::canvas::Path>(&bytes))
		.and_then(|path| convert_path(&path))
	else {
		return Result::InvalidPath.into();
	};
	let Some(canvas) = env
		.data_mut()
		.store
		.get_mut(context)
		.and_then(|item| item.as_canvas())
	else {
		return Result::InvalidContext.into();
	};
	let paint = solid_paint(r, g, b, a);
	canvas
		.pixmap
		.fill_path(&path, &paint, FillRule::Winding, canvas.transform, None);
	Result::Success.into()
}

pub fn stroke(
	mut env: FunctionEnvMut<WasmEnv>,
	context: Rid,
	path_ptr: Ptr,
	style_ptr: Ptr,
) -> FFIResult {
	let Some(path) = env
		.data()
		.read_item_bytes(&env, path_ptr)
		.ok()
		.and_then(|bytes| read_postcard::<aidoku::canvas::Path>(&bytes))
		.and_then(|path| convert_path(&path))
	else {
		return Result::InvalidPath.into();
	};
	let Some(style) = env
		.data()
		.read_item_bytes(&env, style_ptr)
		.ok()
		.and_then(|bytes| read_postcard::<aidoku::canvas::StrokeStyle>(&bytes))
	else {
		return Result::InvalidStyle.into();
	};
	let Some(canvas) = env
		.data_mut()
		.store
		.get_mut(context)
		.and_then(|item| item.as_canvas())
	else {
		return Result::InvalidContext.into();
	};
	let mut stroke = Stroke {
		width: style.width,
		miter_limit: style.miter_limit,
		line_cap: match style.cap {
			aidoku::canvas::LineCap::Butt => tiny_skia::LineCap::Butt,
			aidoku::canvas::LineCap::Round => tiny_skia::LineCap::Round,
			aidoku::canvas::LineCap::Square => tiny_skia::LineCap::Square,
		},
		line_join: match style.join {
			aidoku::canvas::LineJoin::Miter => tiny_skia::LineJoin::Miter,
			aidoku::canvas::LineJoin::Round => tiny_skia::LineJoin::Round,
			aidoku::canvas::LineJoin::Bevel => tiny_skia::LineJoin::Bevel,
		},
		..Stroke::default()
	};
	if !style.dash_array.is_empty() {
		stroke.dash = StrokeDash::new(style.dash_array, style.dash_offset);
	}
	let paint = solid_paint(
		style.color.red,
		style.color.green,
		style.color.blue,
		style.color.alpha,
	);
	canvas
		.pixmap
		.stroke_path(&path, &paint, &stroke, canvas.transform, None);
	Result::Success.into()
}

#[allow(clippy::too_many_arguments)]
pub fn draw_text(
	mut env: FunctionEnvMut<WasmEnv>,
	context: Rid,
	text_ptr: Ptr,
	text_len: u32,
	size: f32,
	x: f32,
	y: f32,
	font: Rid,
	r: f32,
	g: f32,
	b: f32,
	a: f32,
) -> FFIResult {
	let Ok(text) = env.data().read_string(&env, text_ptr, text_len) else {
		return Result::InvalidString.into();
	};
	let Some(font_id) = env.data().store.get(font).and_then(|item| item.as_font()) else {
		return Result::InvalidFont.into();
	};
	let Some(canvas) = env
		.data_mut()
		.store
		.get_mut(context)
		.and_then(|item| item.as_canvas())
	else {
		return Result::InvalidContext.into();
	};
	let paint = solid_paint(r, g, b, a);
	let transform = canvas.transform;
	fonts::with_face(font_id, |face| {
		draw_glyphs(&mut canvas.pixmap, face, &text, size, x, y, &paint, transform);
	});
	Result::Success.into()
}

/// 逐字形取轮廓转 path 填充；(x, y) 是基线起点（与 raqote draw_text 一致）。
/// 无 shaping/kerning：canvas 文本基本是水印式短句，简单排版够用
#[allow(clippy::too_many_arguments)]
fn draw_glyphs(
	pixmap: &mut Pixmap,
	face: &ttf_parser::Face<'_>,
	text: &str,
	size: f32,
	x: f32,
	y: f32,
	paint: &Paint,
	transform: Transform,
) {
	let scale = size / face.units_per_em() as f32;
	let mut pen_x = x;
	for ch in text.chars() {
		let glyph = face.glyph_index(ch).unwrap_or(ttf_parser::GlyphId(0));
		let mut builder = GlyphBuilder {
			pb: PathBuilder::new(),
			scale,
			dx: pen_x,
			baseline: y,
		};
		if face.outline_glyph(glyph, &mut builder).is_some()
			&& let Some(path) = builder.pb.finish()
		{
			pixmap.fill_path(&path, paint, FillRule::Winding, transform, None);
		}
		pen_x += face.glyph_hor_advance(glyph).unwrap_or(0) as f32 * scale;
	}
}

struct GlyphBuilder {
	pb: PathBuilder,
	scale: f32,
	dx: f32,
	baseline: f32,
}

impl GlyphBuilder {
	fn x(&self, v: f32) -> f32 {
		self.dx + v * self.scale
	}

	// 字体坐标 y 向上，屏幕 y 向下
	fn y(&self, v: f32) -> f32 {
		self.baseline - v * self.scale
	}
}

impl ttf_parser::OutlineBuilder for GlyphBuilder {
	fn move_to(&mut self, x: f32, y: f32) {
		self.pb.move_to(self.x(x), self.y(y));
	}

	fn line_to(&mut self, x: f32, y: f32) {
		self.pb.line_to(self.x(x), self.y(y));
	}

	fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
		self.pb.quad_to(self.x(x1), self.y(y1), self.x(x), self.y(y));
	}

	fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
		self.pb
			.cubic_to(self.x(x1), self.y(y1), self.x(x2), self.y(y2), self.x(x), self.y(y));
	}

	fn close(&mut self) {
		self.pb.close();
	}
}

pub fn get_image(mut env: FunctionEnvMut<WasmEnv>, context: Rid) -> Rid {
	let Some(canvas) = env
		.data_mut()
		.store
		.get_mut(context)
		.and_then(|item| item.as_canvas())
	else {
		return Result::InvalidContext.into();
	};
	let image = ImageData {
		data: demultiplied(canvas.pixmap.data()),
		width: canvas.pixmap.width() as i32,
		height: canvas.pixmap.height() as i32,
	};
	env.data_mut().store.store(StoreItem::ImageData(image))
}

pub fn new_font(mut env: FunctionEnvMut<WasmEnv>, name_ptr: Ptr, name_len: u32) -> FFIResult {
	let Ok(name) = env.data().read_string(&env, name_ptr, name_len) else {
		return Result::InvalidString.into();
	};
	let id = fonts::resolve_family(&name);
	env.data_mut().store.store(StoreItem::Font(id))
}

pub fn system_font(mut env: FunctionEnvMut<WasmEnv>, weight: u8) -> Rid {
	let id = fonts::resolve_system(weight);
	env.data_mut().store.store(StoreItem::Font(id))
}

pub fn load_font(mut env: FunctionEnvMut<WasmEnv>, url_ptr: Ptr, url_len: u32) -> FFIResult {
	let Ok(url_str) = env.data().read_string(&env, url_ptr, url_len) else {
		return Result::InvalidString.into();
	};
	let Ok(url) = url::Url::parse(&url_str) else {
		return Result::FontLoadFailed.into();
	};
	// 字体下载与 net 模块同一条出口，代理/限流策略不绕开
	let data = env.data_mut();
	let mut request = crate::imports::NetRequest::new(crate::imports::HttpMethod::Get);
	request.url = Some(url);
	let timeout = data.limits.net_timeout_default;
	if crate::imports::net::perform(&mut data.net, &mut request, timeout).is_err() {
		return Result::FontLoadFailed.into();
	}
	let Some(response) = request.response else {
		return Result::FontLoadFailed.into();
	};
	if !response.status.is_success() {
		return Result::FontLoadFailed.into();
	}
	let Some(id) = fonts::load_bytes(response.data) else {
		return Result::FontLoadFailed.into();
	};
	env.data_mut().store.store(StoreItem::Font(id))
}

pub fn new_image(mut env: FunctionEnvMut<WasmEnv>, data_ptr: Ptr, data_len: u32) -> FFIResult {
	let Ok(data) = env.data().read_bytes(&env, data_ptr, data_len) else {
		return Result::InvalidImage.into();
	};
	let cursor = std::io::Cursor::new(data);
	let Some(rgba) = image::ImageReader::new(cursor)
		.with_guessed_format()
		.ok()
		.and_then(|r| r.decode().ok())
		.map(|img| img.to_rgba8())
	else {
		return Result::InvalidImage.into();
	};
	let (width, height) = rgba.dimensions();
	let image = ImageData::from_rgba8(rgba.into_raw(), width, height);
	env.data_mut().store.store(StoreItem::ImageData(image))
}

pub fn get_image_data(mut env: FunctionEnvMut<WasmEnv>, image_rid: Rid) -> FFIResult {
	let Some(encoded) = env
		.data()
		.store
		.get(image_rid)
		.and_then(|item| match item {
			// 常规 ImageData：编码为 PNG 再存 Encoded（原行为）
			StoreItem::ImageData(image) => image.encode_png(),
			// 处理器入参的原始字节透传：decode_into_store 解码失败时以 Encoded 入 store，
			// data() 必须原样返回（源的解密逻辑在 export 里），而不是 InvalidImagePointer
			StoreItem::Encoded(bytes) => Some(bytes.clone()),
			_ => None,
		})
	else {
		return Result::InvalidImagePointer.into();
	};
	env.data_mut().store.store(StoreItem::Encoded(encoded))
}

pub fn get_image_width(env: FunctionEnvMut<WasmEnv>, image_rid: Rid) -> f32 {
	let Some(image) = env
		.data()
		.store
		.get(image_rid)
		.and_then(|item| item.as_image_data())
	else {
		return Into::<i32>::into(Result::InvalidImagePointer) as f32;
	};
	image.width as f32
}

pub fn get_image_height(env: FunctionEnvMut<WasmEnv>, image_rid: Rid) -> f32 {
	let Some(image) = env
		.data()
		.store
		.get(image_rid)
		.and_then(|item| item.as_image_data())
	else {
		return Into::<i32>::into(Result::InvalidImagePointer) as f32;
	};
	image.height as f32
}
