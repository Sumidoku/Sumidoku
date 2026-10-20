//! Global font library: an embedded DejaVu Sans fallback (runs even in Docker/headless with
//! no system fonts); system font directories are scanned at process start to widen coverage
//! (especially CJK), and load_font loads by URL at runtime. One copy shared process-wide,
//! read-mostly across actors; load_font appends under the write lock.
use std::sync::{OnceLock, RwLock};

// DejaVu Sans 2.37, Bitstream Vera family license (see assets/DejaVuSans.LICENSE)
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

/// new_font's server-side semantics align with AidokuRunner's UIFont(name:): exact match on
/// PostScript name or family; when a family hits multiple faces, take the one closest to the
/// regular weight; no match returns None (the caller reports invalidFont) — never falls back
/// to the fallback font
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

/// system_font's server-side semantics: pick the best sans-serif by weight, falling back to the embedded font when nothing matches
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

/// Load font bytes (ttc collections supported, taking the first new face); None when every face fails to parse
pub fn load_bytes(data: Vec<u8>) -> Option<fontdb::ID> {
	let mut lib = library().write().expect("font library lock");
	let before = lib.db.faces().count();
	lib.db.load_font_data(data);
	lib.db.faces().nth(before).map(|face| face.id)
}

pub fn with_face<R>(id: fontdb::ID, f: impl FnOnce(&ttf_parser::Face<'_>) -> R) -> Option<R> {
	let lib = library().read().expect("font library lock");
	// A Face borrows bytes inside the library and can only be used within the closure
	lib.db
		.with_face_data(id, |data, index| {
			let face = ttf_parser::Face::parse(data, index).ok()?;
			Some(f(&face))
		})
		.flatten()
}
