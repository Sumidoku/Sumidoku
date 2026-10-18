//! 全局字体库：内嵌 DejaVu Sans 兜底（Docker/headless 无系统字体也能跑），
//! 进程启动时顺带扫系统字体目录扩充覆盖（尤其 CJK），load_font 运行时按 URL 加载。
//! 全进程共享一份，actor 间只读为主；load_font 追加走写锁。
use std::sync::{OnceLock, RwLock};

// DejaVu Sans 2.37，Bitstream Vera 系许可（见 assets/DejaVuSans.LICENSE）
const FALLBACK_FONT: &[u8] = include_bytes!("../assets/DejaVuSans.ttf");

struct FontLibrary {
	db: fontdb::Database,
	fallback: fontdb::ID,
}

static LIBRARY: OnceLock<RwLock<FontLibrary>> = OnceLock::new();

fn library() -> &'static RwLock<FontLibrary> {
	LIBRARY.get_or_init(|| {
		let mut db = fontdb::Database::new();
		db.load_font_data(FALLBACK_FONT.to_vec());
		db.load_system_fonts();
		let fallback = db
			.query(&fontdb::Query {
				families: &[fontdb::Family::Name("DejaVu Sans")],
				..fontdb::Query::default()
			})
			.expect("embedded fallback font must parse");
		RwLock::new(FontLibrary { db, fallback })
	})
}

/// new_font 的服务端语义对齐 AidokuRunner 的 UIFont(name:)：PostScript 名或
/// family 精确匹配，family 命中多个 face 取最接近常规字重的；匹配不到返回
/// None（调用方报 invalidFont），不回退兜底字体
pub fn resolve_family_strict(name: &str) -> Option<fontdb::ID> {
	if name.is_empty() {
		return None;
	}
	let lib = library().read().expect("font library lock");
	let mut best: Option<(i32, fontdb::ID)> = None;
	for face in lib.db.faces() {
		if face.post_script_name == name {
			return Some(face.id);
		}
		if face.families.iter().any(|(family, _)| family == name) {
			let distance = (i32::from(face.weight.0) - 400).abs() * 2 + i32::from(face.style != fontdb::Style::Normal);
			if best.is_none_or(|(d, _)| distance < d) {
				best = Some((distance, face.id));
			}
		}
	}
	best.map(|(_, id)| id)
}

/// system_font 的服务端语义：sans-serif 按字重选优，落空回落内嵌字体
pub fn resolve_system(weight: u8) -> fontdb::ID {
	let weight = match aidoku::canvas::FontWeight::from(weight) {
		aidoku::canvas::FontWeight::UltraLight => fontdb::Weight::EXTRA_LIGHT,
		aidoku::canvas::FontWeight::Thin => fontdb::Weight::THIN,
		aidoku::canvas::FontWeight::Light => fontdb::Weight::LIGHT,
		aidoku::canvas::FontWeight::Regular => fontdb::Weight::NORMAL,
		aidoku::canvas::FontWeight::Medium => fontdb::Weight::MEDIUM,
		aidoku::canvas::FontWeight::Semibold => fontdb::Weight::SEMIBOLD,
		aidoku::canvas::FontWeight::Bold => fontdb::Weight::BOLD,
		aidoku::canvas::FontWeight::Heavy => fontdb::Weight::EXTRA_BOLD,
		aidoku::canvas::FontWeight::Black => fontdb::Weight::BLACK,
	};
	let lib = library().read().expect("font library lock");
	lib.db
		.query(&fontdb::Query {
			families: &[fontdb::Family::SansSerif],
			weight,
			..fontdb::Query::default()
		})
		.unwrap_or(lib.fallback)
}

/// 加载字体字节（支持 ttc 合集，取首个新 face）；全部解析失败返回 None
pub fn load_bytes(data: Vec<u8>) -> Option<fontdb::ID> {
	let mut lib = library().write().expect("font library lock");
	let before = lib.db.faces().count();
	lib.db.load_font_data(data);
	lib.db.faces().nth(before).map(|face| face.id)
}

pub fn with_face<R>(id: fontdb::ID, f: impl FnOnce(&ttf_parser::Face<'_>) -> R) -> Option<R> {
	let lib = library().read().expect("font library lock");
	// Face 借用库内字节，只能在闭包里用
	lib.db
		.with_face_data(id, |data, index| {
			let face = ttf_parser::Face::parse(data, index).ok()?;
			Some(f(&face))
		})
		.flatten()
}
