use crate::runtime::{
	env::Ctx,
	store::{FFIResult, Ptr, Rid},
};

enum Result {
	Success,
	InvalidDescriptor,
	FailedMemoryWrite,
	InvalidString,
	InvalidDateString,
}

impl From<Result> for i32 {
	fn from(result: Result) -> i32 {
		match result {
			Result::Success => 0,
			Result::InvalidDescriptor => -1,
			Result::FailedMemoryWrite => -3,
			Result::InvalidString => -4,
			Result::InvalidDateString => -5,
		}
	}
}

pub fn destroy(ctx: &mut impl Ctx, rid: Rid) {
	ctx.host().store.remove(rid);
	// A webview rid may have a browser context attached; reclaim it along with destroy
	if let Some(line) = ctx.host().browser.destroy(rid) {
		ctx.host().log(&line);
	}
}

pub fn buffer_len(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(data) = ctx.host().store.get(rid).and_then(|item| item.as_encoded()) else {
		return Result::InvalidDescriptor.into();
	};
	data.len() as FFIResult
}

pub fn read_buffer(ctx: &mut impl Ctx, rid: Rid, ptr: Ptr, size: u32) -> FFIResult {
	let Some(data) = ctx.host().store.get(rid).and_then(|item| item.as_encoded()) else {
		return Result::InvalidDescriptor.into();
	};
	if size as usize > data.len() {
		return Result::FailedMemoryWrite.into();
	}
	let data = data.iter().take(size as usize).copied().collect::<Vec<_>>();
	if ctx.write_buffer(ptr, &data).is_err() {
		Result::FailedMemoryWrite.into()
	} else {
		Result::Success.into()
	}
}

pub fn current_date(_ctx: &mut impl Ctx) -> f64 {
	chrono::Utc::now().timestamp() as f64
}

pub fn utc_offset(_ctx: &mut impl Ctx) -> i64 {
	chrono::Local::now().offset().utc_minus_local() as i64
}

// The signature is fixed by the source ABI; parameters cannot be merged
#[allow(clippy::too_many_arguments)]
pub fn parse_date(
	ctx: &mut impl Ctx,
	date_str: Ptr,
	date_len: u32,
	format_str: Ptr,
	format_len: u32,
	locale_str: Ptr,
	locale_len: u32,
	timezone_str: Ptr,
	timezone_len: u32,
) -> f64 {
	let invalid = || Into::<i32>::into(Result::InvalidDateString) as f64;
	let Ok(string) = ctx.read_string(date_str, date_len) else {
		return Into::<i32>::into(Result::InvalidString) as f64;
	};
	let Ok(format) = ctx.read_string(format_str, format_len) else {
		return Into::<i32>::into(Result::InvalidString) as f64;
	};
	let timezone_string = if timezone_len > 0 {
		ctx.read_string(timezone_str, timezone_len).ok()
	} else {
		None
	};
	let locale_string = if locale_len > 0 {
		ctx.read_string(locale_str, locale_len).ok()
	} else {
		None
	};
	let Some(timestamp) = parse_date_impl(&string, &format, locale_string.as_deref(), timezone_string.as_deref()) else {
		return invalid();
	};
	timestamp
}

fn parse_date_impl(string: &str, format: &str, locale: Option<&str>, timezone: Option<&str>) -> Option<f64> {
	let translated = swift_date_format_to_chrono(format);
	let input = if translated.has_month_name {
		&*normalize_month_names(string, locale)
	} else {
		string
	};

	let subsec = |nanos: u32| f64::from(nanos) / 1e9;
	// When the format has an offset token, the offset in the string wins over the timezone
	// parameter (same semantics as Swift DateFormatter); and parsing must go through
	// DateTime: NaiveDateTime silently drops the offset
	if translated.has_zone_offset {
		let dt = chrono::DateTime::parse_from_str(input, &translated.fmt).ok()?;
		return Some(dt.timestamp() as f64 + subsec(dt.timestamp_subsec_nanos()));
	}

	let naive = chrono::NaiveDateTime::parse_from_str(input, &translated.fmt)
		.ok()
		.or_else(|| {
			// A date-only string counts as 00:00 of that day (same semantics as DateFormatter)
			chrono::NaiveDate::parse_from_str(input, &translated.fmt)
				.ok()
				.and_then(|d| d.and_hms_opt(0, 0, 0))
		})?;
	let dt = naive.and_local_timezone(resolve_timezone(timezone)).single()?;
	Some(dt.timestamp() as f64 + subsec(dt.timestamp_subsec_nanos()))
}

/// AidokuRunner semantics: an absent timezone, "current", and an unparseable one all land
/// on the system timezone; when even the system timezone name is unavailable, fall back to
/// UTC to keep results deterministic
fn resolve_timezone(timezone: Option<&str>) -> chrono_tz::Tz {
	timezone
		.filter(|tz| !tz.is_empty() && *tz != "current")
		.and_then(|tz| tz.parse().ok())
		.or_else(|| iana_time_zone::get_timezone().ok()?.parse().ok())
		.unwrap_or(chrono_tz::UTC)
}

struct ChronoFormat {
	fmt: String,
	has_zone_offset: bool,
	has_month_name: bool,
}

fn swift_date_format_to_chrono(format: &str) -> ChronoFormat {
	let mut fmt = String::new();
	let mut has_zone_offset = false;
	let mut has_month_name = false;
	let mut chars = format.chars().peekable();

	while let Some(c) = chars.next() {
		if c == '\'' {
			// TR35 literals: '...' passes through verbatim, '' escapes to a single quote
			if chars.peek() == Some(&'\'') {
				chars.next();
				fmt.push('\'');
				continue;
			}
			while let Some(lc) = chars.next() {
				if lc == '\'' {
					if chars.peek() == Some(&'\'') {
						chars.next();
						fmt.push('\'');
					} else {
						break;
					}
				} else {
					push_literal(&mut fmt, lc);
				}
			}
			continue;
		}
		let token = match c {
			'y' => {
				let mut count = 1;
				while chars.peek() == Some(&'y') {
					chars.next();
					count += 1;
				}
				if count == 2 { "%y" } else { "%Y" }
			}
			'M' => {
				let mut count = 1;
				while chars.peek() == Some(&'M') {
					chars.next();
					count += 1;
				}
				if count >= 3 {
					has_month_name = true;
				}
				match count {
					1 | 2 => "%m",
					3 => "%b",
					_ => "%B",
				}
			}
			'd' => {
				while chars.peek() == Some(&'d') {
					chars.next();
				}
				"%d"
			}
			'H' => {
				while chars.peek() == Some(&'H') {
					chars.next();
				}
				"%H"
			}
			'h' => {
				while chars.peek() == Some(&'h') {
					chars.next();
				}
				"%I"
			}
			'm' => {
				while chars.peek() == Some(&'m') {
					chars.next();
				}
				"%M"
			}
			's' => {
				while chars.peek() == Some(&'s') {
					chars.next();
				}
				"%S"
			}
			'S' => {
				while chars.peek() == Some(&'S') {
					chars.next();
				}
				// chrono's %f counts nanoseconds, while %.f is the fractional second and
				// brings its own dot: the literal dot before the S token in the format is
				// merged into the token, otherwise both dots would need consuming
				if fmt.ends_with('.') {
					fmt.pop();
				}
				"%.f"
			}
			'a' => "%p",
			'E' => {
				let mut count = 1;
				while chars.peek() == Some(&'E') {
					chars.next();
					count += 1;
				}
				if count >= 4 { "%A" } else { "%a" }
			}
			'z' => {
				let mut count = 1;
				while chars.peek() == Some(&c) {
					chars.next();
					count += 1;
				}
				if count >= 4 {
					"%Z"
				} else {
					has_zone_offset = true;
					"%#z"
				}
			}
			'X' | 'x' => {
				while chars.peek() == Some(&c) {
					chars.next();
				}
				// %#z is chrono's only specifier that accepts Z, ±HH, ±HHMM, and ±HH:MM alike
				has_zone_offset = true;
				"%#z"
			}
			'Z' => {
				while chars.peek() == Some(&'Z') {
					chars.next();
				}
				"%Z"
			}
			c => {
				push_literal(&mut fmt, c);
				continue;
			}
		};
		fmt.push_str(token);
	}
	ChronoFormat {
		fmt,
		has_zone_offset,
		has_month_name,
	}
}

// % is chrono's format lead-in; occurrences inside literals must be escaped
fn push_literal(fmt: &mut String, c: char) {
	if c == '%' {
		fmt.push_str("%%");
	} else {
		fmt.push(c);
	}
}

/// Month-name spellings actually used by sources (parse_date call sites in
/// Aidoku-Community/sources); chrono only recognizes English, so other spellings are
/// replaced with English abbreviations before parsing (both %b/%B accept abbreviations)
struct LocaleMonths {
	full: [&'static str; 12],
	abbr: [&'static str; 12],
	// Other common spellings of the same month (e.g. Spanish September: sep/sept)
	extra: &'static [(&'static str, usize)],
}

const EN_ABBR: [&str; 12] = [
	"Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

const ES: LocaleMonths = LocaleMonths {
	full: [
		"enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre",
		"noviembre", "diciembre",
	],
	abbr: [
		"ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sept", "oct", "nov", "dic",
	],
	extra: &[("sep", 8)],
};

const IT: LocaleMonths = LocaleMonths {
	full: [
		"gennaio", "febbraio", "marzo", "aprile", "maggio", "giugno", "luglio", "agosto", "settembre", "ottobre",
		"novembre", "dicembre",
	],
	abbr: [
		"gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic",
	],
	extra: &[],
};

const PT: LocaleMonths = LocaleMonths {
	full: [
		"janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro",
		"novembro", "dezembro",
	],
	abbr: [
		"jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez",
	],
	extra: &[],
};

const TR: LocaleMonths = LocaleMonths {
	full: [
		"ocak", "şubat", "mart", "nisan", "mayıs", "haziran", "temmuz", "ağustos", "eylül", "ekim", "kasım",
		"aralık",
	],
	abbr: [
		"oca", "şub", "mar", "nis", "may", "haz", "tem", "ağu", "eyl", "eki", "kas", "ara",
	],
	extra: &[],
};

const FR: LocaleMonths = LocaleMonths {
	full: [
		"janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre",
		"décembre",
	],
	abbr: [
		"janv", "févr", "mars", "avr", "mai", "juin", "juil", "août", "sept", "oct", "nov", "déc",
	],
	extra: &[],
};

const ID: LocaleMonths = LocaleMonths {
	full: [
		"januari", "februari", "maret", "april", "mei", "juni", "juli", "agustus", "september", "oktober", "november",
		"desember",
	],
	abbr: [
		"jan", "feb", "mar", "apr", "mei", "jun", "jul", "agu", "sep", "okt", "nov", "des",
	],
	extra: &[],
};

const AR: LocaleMonths = LocaleMonths {
	full: [
		"يناير", "فبراير", "مارس", "أبريل", "مايو", "يونيو", "يوليو", "أغسطس", "سبتمبر", "أكتوبر", "نوفمبر", "ديسمبر",
	],
	abbr: [
		"يناير", "فبراير", "مارس", "أبريل", "مايو", "يونيو", "يوليو", "أغسطس", "سبتمبر", "أكتوبر", "نوفمبر", "ديسمبر",
	],
	extra: &[],
};

// Chinese/Japanese "N月" and Korean "N월": month names are numeric — the locale only affects the spelling, not the value
const CJK: LocaleMonths = LocaleMonths {
	full: [
		"1月", "2月", "3月", "4月", "5月", "6月", "7月", "8月", "9月", "10月", "11月", "12月",
	],
	abbr: [
		"1月", "2月", "3月", "4月", "5月", "6月", "7月", "8月", "9月", "10月", "11月", "12月",
	],
	extra: &[],
};

const KO: LocaleMonths = LocaleMonths {
	full: [
		"1월", "2월", "3월", "4월", "5월", "6월", "7월", "8월", "9월", "10월", "11월", "12월",
	],
	abbr: [
		"1월", "2월", "3월", "4월", "5월", "6월", "7월", "8월", "9월", "10월", "11월", "12월",
	],
	extra: &[],
};

fn locale_months(lang: &str) -> Option<&'static LocaleMonths> {
	Some(match lang {
		"es" => &ES,
		"it" => &IT,
		"pt" => &PT,
		"tr" => &TR,
		"fr" => &FR,
		"id" => &ID,
		"ar" => &AR,
		"ja" | "zh" => &CJK,
		"ko" => &KO,
		_ => return None,
	})
}

/// Replace localized month names in the string with English abbreviations: scan char by
/// char, longest match first ("marzo" is not cut off by "mar"); no match at boundaries
/// adjacent to letters; an abbreviation may consume a trailing period (ene. / janv.)
fn normalize_month_names<'a>(string: &'a str, locale: Option<&str>) -> std::borrow::Cow<'a, str> {
	let lang = locale
		.and_then(|l| l.split(['_', '-']).next())
		.map(str::to_ascii_lowercase)
		.filter(|l| l != "current" && !l.starts_with("en"));
	let Some(months) = lang.as_deref().and_then(locale_months) else {
		return std::borrow::Cow::Borrowed(string);
	};

	let mut aliases: Vec<(&'static str, usize, bool)> = Vec::new();
	for (i, (full, abbr)) in months.full.iter().zip(&months.abbr).enumerate() {
		aliases.push((full, i, false));
		aliases.push((abbr, i, true));
	}
	aliases.extend(months.extra.iter().map(|(alias, i)| (*alias, *i, true)));
	aliases.sort_by_key(|(alias, ..)| std::cmp::Reverse(alias.chars().count()));

	let mut out = String::with_capacity(string.len());
	let mut rest = string;
	let mut prev_alpha = false;
	while !rest.is_empty() {
		let hit = aliases.iter().find_map(|(alias, month, is_abbr)| {
			let (alias, month, is_abbr) = (*alias, *month, *is_abbr);
			if prev_alpha && alias.starts_with(|c: char| c.is_alphabetic()) {
				return None;
			}
			let consumed = match_ignore_case(rest, alias)?;
			let after = &rest[consumed..];
			if alias.ends_with(|c: char| c.is_alphabetic()) && after.starts_with(|c: char| c.is_alphabetic()) {
				return None;
			}
			let dot = usize::from(is_abbr && after.starts_with('.'));
			Some((consumed + dot, month))
		});
		if let Some((consumed, month)) = hit {
			out.push_str(EN_ABBR[month]);
			rest = &rest[consumed..];
			prev_alpha = true;
		} else {
			let c = rest.chars().next().expect("rest is non-empty");
			prev_alpha = c.is_alphabetic();
			out.push(c);
			rest = &rest[c.len_utf8()..];
		}
	}
	std::borrow::Cow::Owned(out)
}

/// Char-by-char Unicode case-insensitive prefix match; returns the number of bytes consumed
fn match_ignore_case(rest: &str, alias: &str) -> Option<usize> {
	let mut consumed = 0;
	let mut rest_chars = rest.chars();
	for ac in alias.chars() {
		let rc = rest_chars.next()?;
		if rc != ac && !rc.to_lowercase().eq(ac.to_lowercase()) {
			return None;
		}
		consumed += rc.len_utf8();
	}
	Some(consumed)
}

#[cfg(test)]
mod tests {
	use super::parse_date_impl;

	fn local_tz() -> String {
		iana_time_zone::get_timezone().expect("local timezone name")
	}

	// The format/locale combinations below all come from real calls in
	// Aidoku-Community/sources; expected values are pinned with an explicit UTC timezone,
	// independent of the runtime environment

	#[test]
	fn full_datetime_with_explicit_timezone() {
		// Regression guard that is green as-is (zh.jmcomic)
		assert_eq!(
			parse_date_impl("2024-01-15 12:00:00", "yyyy-MM-dd HH:mm:ss", None, Some("Asia/Tokyo")),
			Some(1705287600.0)
		);
	}

	#[test]
	fn date_only_parses_as_midnight() {
		// en.mangademon: date-only string
		assert_eq!(
			parse_date_impl("2024-01-15", "yyyy-MM-dd", Some("en_US"), Some("UTC")),
			Some(1705276800.0)
		);
	}

	#[test]
	fn date_only_with_japanese_literals() {
		// ja.spoilerplus: the year/month/day in the format are bare literals
		assert_eq!(
			parse_date_impl("2024年01月15日", "yyyy年MM月dd日", Some("ja_JP"), Some("Asia/Tokyo")),
			Some(1705244400.0)
		);
	}

	#[test]
	fn quoted_literals_are_verbatim() {
		// TR35 single-quote literals (same as ja.rawdevart's 'T'); '' is an escaped single quote
		assert_eq!(
			parse_date_impl("2024年01月15日", "yyyy'年'MM'月'dd'日'", None, Some("UTC")),
			Some(1705276800.0)
		);
		assert_eq!(
			parse_date_impl("2024'01'15", "yyyy''MM''dd", None, Some("UTC")),
			Some(1705276800.0)
		);
	}

	#[test]
	fn fractional_seconds_token() {
		// S = the fractional-second digits, the SSS of rawdevart's DATE_FORMAT
		assert_eq!(
			parse_date_impl("2024-01-15T12:00:00.500", "yyyy-MM-dd'T'HH:mm:ss.SSS", None, Some("UTC")),
			Some(1705320000.5)
		);
	}

	#[test]
	fn iso8601_zone_tokens() {
		// The X/x family: all of Z, ±HH:MM, ±HHMM, ±HH (the XXX of rawdevart's DATE_FORMAT)
		assert_eq!(
			parse_date_impl("2024-01-15T12:00:00Z", "yyyy-MM-dd'T'HH:mm:ssXXX", None, Some("UTC")),
			Some(1705320000.0)
		);
		assert_eq!(
			parse_date_impl("2024-01-15T12:00:00+09:00", "yyyy-MM-dd'T'HH:mm:ssXXX", None, Some("UTC")),
			Some(1705287600.0)
		);
		assert_eq!(
			parse_date_impl("2024-01-15T12:00:00+09", "yyyy-MM-dd'T'HH:mm:ssX", None, Some("UTC")),
			Some(1705287600.0)
		);
		assert_eq!(
			parse_date_impl("2024-01-15 12:00:00 +0900", "yyyy-MM-dd HH:mm:ss x", None, Some("UTC")),
			Some(1705287600.0)
		);
		// The offset inside the string wins over the timezone parameter
		assert_eq!(
			parse_date_impl("2024-01-15T12:00:00+09:00", "yyyy-MM-dd'T'HH:mm:ssXXX", None, Some("UTC")),
			parse_date_impl("2024-01-15T12:00:00+09:00", "yyyy-MM-dd'T'HH:mm:ssXXX", None, Some("America/New_York"))
		);
	}

	#[test]
	fn missing_timezone_defaults_to_local() {
		let local = local_tz();
		let (s, f) = ("2024-06-15 12:00:00", "yyyy-MM-dd HH:mm:ss");
		assert_eq!(
			parse_date_impl(s, f, None, None),
			parse_date_impl(s, f, None, Some(&local)),
			"缺省时区应落本地时区（本机 {local}）"
		);
	}

	#[test]
	fn current_timezone_means_local() {
		let local = local_tz();
		let (s, f) = ("2024-06-15 12:00:00", "yyyy-MM-dd HH:mm:ss");
		assert_eq!(parse_date_impl(s, f, None, Some("current")), parse_date_impl(s, f, None, Some(&local)));
	}

	#[test]
	fn unknown_timezone_falls_back_to_local() {
		// In AidokuRunner, TimeZone(identifier:) failing = keep the current timezone
		let local = local_tz();
		let (s, f) = ("2024-06-15 12:00:00", "yyyy-MM-dd HH:mm:ss");
		assert_eq!(
			parse_date_impl(s, f, None, Some("Not/AZone")),
			parse_date_impl(s, f, None, Some(&local))
		);
	}

	#[test]
	fn locale_month_names_spanish() {
		// The actual format of es.latazamediollena
		assert_eq!(
			parse_date_impl("5 de enero de 2024", "d 'de' MMMM 'de' yyyy", Some("es_ES"), Some("UTC")),
			Some(1704412800.0)
		);
		// The abbreviated spelling with a period must also be consumed
		assert_eq!(
			parse_date_impl("5 ene. 2024", "d MMM yyyy", Some("es"), Some("UTC")),
			Some(1704412800.0)
		);
	}

	#[test]
	fn locale_month_names_italian() {
		// The mangaworld template
		assert_eq!(
			parse_date_impl("5 gennaio 2024", "d MMMM yyyy", Some("it"), Some("UTC")),
			Some(1704412800.0)
		);
	}

	#[test]
	fn locale_month_names_portuguese() {
		// pt.flowermanga (the MMMMM narrow form is parsed by full name)
		assert_eq!(
			parse_date_impl("5 de janeiro de 2024", "d 'de' MMMMM 'de' yyyy", Some("pt_BR"), Some("UTC")),
			Some(1704412800.0)
		);
	}

	#[test]
	fn locale_month_names_turkish() {
		// tr.tortugaceviri
		assert_eq!(
			parse_date_impl("5 Ocak 2024", "d MMMM yyyy", Some("tr_TR"), Some("UTC")),
			Some(1704412800.0)
		);
	}

	#[test]
	fn locale_month_names_french() {
		// fr.sushiscan
		assert_eq!(
			parse_date_impl("5 janvier 2024", "d MMMM yyyy", Some("fr_FR"), Some("UTC")),
			Some(1704412800.0)
		);
	}

	#[test]
	fn locale_month_names_indonesian() {
		// id.komiktap (the mangathemesia template's default format)
		assert_eq!(
			parse_date_impl("Januari 05, 2024", "MMMM dd, yyyy", Some("id"), Some("UTC")),
			Some(1704412800.0)
		);
	}

	#[test]
	fn locale_month_names_arabic() {
		// ar.aasq (Arabic comma U+060C + three-digit y)
		assert_eq!(
			parse_date_impl("5 يناير\u{060c} 2024", "d MMM\u{060c} yyy", Some("ar"), Some("UTC")),
			Some(1704412800.0)
		);
	}

	#[test]
	fn locale_month_names_japanese() {
		// ja.rawfree: MMM in the ja locale is the numeric "N月" form
		assert_eq!(
			parse_date_impl("1月 5, 2024", "MMM d, yyyy", Some("ja_JP"), Some("UTC")),
			Some(1704412800.0)
		);
	}

	#[test]
	fn english_month_names_work_regardless_of_locale() {
		// en_US_POSIX guard: English month names work in every case (green as-is)
		assert_eq!(
			parse_date_impl("Jan 5, 2024", "MMM d, yyyy", Some("en_US_POSIX"), Some("UTC")),
			Some(1704412800.0)
		);
	}
}
