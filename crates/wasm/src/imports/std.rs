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
	// webview rid 可能挂着浏览器 context，随 destroy 一并回收
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

// 签名由源 ABI 固定，无法归并参数
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
	// 格式含偏移 token 时串里的偏移优先于 timezone 参数（Swift DateFormatter 同语义）；
	// 且必须走 DateTime 解析：NaiveDateTime 会静默丢弃偏移量
	if translated.has_zone_offset {
		let dt = chrono::DateTime::parse_from_str(input, &translated.fmt).ok()?;
		return Some(dt.timestamp() as f64 + subsec(dt.timestamp_subsec_nanos()));
	}

	let naive = chrono::NaiveDateTime::parse_from_str(input, &translated.fmt)
		.ok()
		.or_else(|| {
			// 纯日期串按当天 00:00 计（DateFormatter 同语义）
			chrono::NaiveDate::parse_from_str(input, &translated.fmt)
				.ok()
				.and_then(|d| d.and_hms_opt(0, 0, 0))
		})?;
	let dt = naive.and_local_timezone(resolve_timezone(timezone)).single()?;
	Some(dt.timestamp() as f64 + subsec(dt.timestamp_subsec_nanos()))
}

/// AidokuRunner 语义：时区缺席、"current"、不可解析都落系统当前时区；
/// 系统时区名都拿不到时退回 UTC，保持结果确定
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
			// TR35 字面量：'...' 原样透传，'' 转义出一个引号
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
				// chrono 的 %f 按纳秒计数，%.f 才是秒小数且自带小数点：
				// 格式里 S 前的字面点并入 token，否则两个点都要吃
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
				// %#z 是 chrono 唯一能同吃 Z、±HH、±HHMM、±HH:MM 的解析符
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

// % 是 chrono 的格式前导符，字面量里出现的要转义
fn push_literal(fmt: &mut String, c: char) {
	if c == '%' {
		fmt.push_str("%%");
	} else {
		fmt.push(c);
	}
}

/// 源实际用到的月名写法（Aidoku-Community/sources 的 parse_date 调用点）；
/// chrono 只认英文，解析前把其它写法替换成英文缩写（%b/%B 都接受缩写）
struct LocaleMonths {
	full: [&'static str; 12],
	abbr: [&'static str; 12],
	// 同一月份的其它常见写法（如西语九月 sep/sept）
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

// 中日「N月」、韩「N월」：月名是数字式，locale 只影响写法不影响数值
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

/// 把串里的本地化月名替换成英文缩写：逐字符扫、最长优先（"marzo" 不被 "mar" 截胡）、
/// 与字母相邻的边界处不命中，缩写可顺带吃掉一个句点（ene. / janv.）
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

/// 逐字符 Unicode 大小写不敏感的前缀匹配，返回吃掉的字节数
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

	// 以下格式/ locale 组合全部取自 Aidoku-Community/sources 的真实调用，
	// 期望值以 UTC 显式时区钉死，与运行环境无关

	#[test]
	fn full_datetime_with_explicit_timezone() {
		// 现状即绿的回归护栏（zh.jmcomic）
		assert_eq!(
			parse_date_impl("2024-01-15 12:00:00", "yyyy-MM-dd HH:mm:ss", None, Some("Asia/Tokyo")),
			Some(1705287600.0)
		);
	}

	#[test]
	fn date_only_parses_as_midnight() {
		// en.mangademon: 纯日期串
		assert_eq!(
			parse_date_impl("2024-01-15", "yyyy-MM-dd", Some("en_US"), Some("UTC")),
			Some(1705276800.0)
		);
	}

	#[test]
	fn date_only_with_japanese_literals() {
		// ja.spoilerplus: 格式里的年月日是裸字面量
		assert_eq!(
			parse_date_impl("2024年01月15日", "yyyy年MM月dd日", Some("ja_JP"), Some("Asia/Tokyo")),
			Some(1705244400.0)
		);
	}

	#[test]
	fn quoted_literals_are_verbatim() {
		// TR35 单引号字面量（ja.rawdevart 的 'T' 同款）；'' 是转义的单个引号
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
		// S = 秒的小数位，rawdevart DATE_FORMAT 的 SSS
		assert_eq!(
			parse_date_impl("2024-01-15T12:00:00.500", "yyyy-MM-dd'T'HH:mm:ss.SSS", None, Some("UTC")),
			Some(1705320000.5)
		);
	}

	#[test]
	fn iso8601_zone_tokens() {
		// X/x 系列：Z、±HH:MM、±HHMM、±HH 全形态（rawdevart DATE_FORMAT 的 XXX）
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
		// 串内偏移量优先于 timezone 参数
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
		// AidokuRunner 里 TimeZone(identifier:) 失败 = 保持当前时区
		let local = local_tz();
		let (s, f) = ("2024-06-15 12:00:00", "yyyy-MM-dd HH:mm:ss");
		assert_eq!(
			parse_date_impl(s, f, None, Some("Not/AZone")),
			parse_date_impl(s, f, None, Some(&local))
		);
	}

	#[test]
	fn locale_month_names_spanish() {
		// es.latazamediollena 的实际格式
		assert_eq!(
			parse_date_impl("5 de enero de 2024", "d 'de' MMMM 'de' yyyy", Some("es_ES"), Some("UTC")),
			Some(1704412800.0)
		);
		// 缩写带句号的写法也要吃
		assert_eq!(
			parse_date_impl("5 ene. 2024", "d MMM yyyy", Some("es"), Some("UTC")),
			Some(1704412800.0)
		);
	}

	#[test]
	fn locale_month_names_italian() {
		// mangaworld 模板
		assert_eq!(
			parse_date_impl("5 gennaio 2024", "d MMMM yyyy", Some("it"), Some("UTC")),
			Some(1704412800.0)
		);
	}

	#[test]
	fn locale_month_names_portuguese() {
		// pt.flowermanga（MMMMM 窄形式按全名解析）
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
		// id.komiktap（mangathemesia 模板默认格式）
		assert_eq!(
			parse_date_impl("Januari 05, 2024", "MMMM dd, yyyy", Some("id"), Some("UTC")),
			Some(1704412800.0)
		);
	}

	#[test]
	fn locale_month_names_arabic() {
		// ar.aasq（阿拉伯文逗号 U+060C + 三位 y）
		assert_eq!(
			parse_date_impl("5 يناير\u{060c} 2024", "d MMM\u{060c} yyy", Some("ar"), Some("UTC")),
			Some(1704412800.0)
		);
	}

	#[test]
	fn locale_month_names_japanese() {
		// ja.rawfree：ja locale 的 MMM 是「N月」数字式
		assert_eq!(
			parse_date_impl("1月 5, 2024", "MMM d, yyyy", Some("ja_JP"), Some("UTC")),
			Some(1704412800.0)
		);
	}

	#[test]
	fn english_month_names_work_regardless_of_locale() {
		// en_US_POSIX 护栏：英文月名任何情况下都可用（现状即绿）
		assert_eq!(
			parse_date_impl("Jan 5, 2024", "MMM d, yyyy", Some("en_US_POSIX"), Some("UTC")),
			Some(1704412800.0)
		);
	}
}
