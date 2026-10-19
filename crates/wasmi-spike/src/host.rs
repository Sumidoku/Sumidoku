//! env/std/net 三族的引擎无关函数体：与生产 `crates/wasm/src/imports/` 同构，
//! 区别只在状态与内存的访问方式被收敛进 [`Ctx`]——两引擎各自实现 Ctx，
//! 函数体本身一份两用。parse_date 整段取自生产 `imports/std.rs`（chrono 翻译层
//! 与引擎零耦合），以此坐实「std 族移植是函数体平移」。

pub struct HostState {
	pub store: crate::store::GlobalStore,
	pub limits: crate::Limits,
	pub net_client: reqwest::blocking::Client,
	/// print/abort 的落点；生产走 HostLogger，spike 收集备查
	pub logs: Vec<String>,
}

impl HostState {
	pub fn new(limits: crate::Limits) -> Self {
		Self {
			store: crate::store::GlobalStore::new(),
			limits,
			net_client: reqwest::blocking::Client::builder()
				.use_rustls_tls()
				.build()
				.expect("build http client"),
			logs: Vec::new(),
		}
	}
}

/// 引擎 shim 与共享函数体之间的唯一接口：宿主状态 + guest 线性内存读写。
/// 每个 import 函数体内的 host()/mem_* 调用不得互相持有（&mut 重入），
/// 都是取到 owned 数据就放下
pub trait Ctx {
	fn host(&mut self) -> &mut HostState;
	fn mem_read(&self, ptr: u32, len: u32) -> Option<Vec<u8>>;
	fn mem_write(&mut self, ptr: u32, bytes: &[u8]) -> bool;
}

fn read_string(ctx: &impl Ctx, ptr: u32, len: u32) -> Option<String> {
	let bytes = ctx.mem_read(ptr, len)?;
	String::from_utf8(bytes).ok()
}

pub mod env {
	use super::Ctx;

	pub fn abort(ctx: &mut impl Ctx) {
		ctx.host().logs.push("error: abort called".to_string());
	}

	pub fn print(ctx: &mut impl Ctx, ptr: u32, len: u32) {
		let message = match super::read_string(ctx, ptr, len) {
			Some(s) => s,
			None => "error: failed to read string for printing".to_string(),
		};
		ctx.host().logs.push(message);
	}

	pub fn sleep(ctx: &mut impl Ctx, seconds: i32) {
		let cap = ctx.host().limits.sleep_max;
		let duration = std::time::Duration::from_secs(seconds.max(0) as u64).min(cap);
		std::thread::sleep(duration);
	}

	/// spike 不接流式 sink：与生产「无在途流式调用时按 ABI 丢弃」同一分支
	pub fn send_partial_result(_ctx: &mut impl Ctx, value: i32) {
		let _ = value;
	}
}

pub mod std {
	use super::Ctx;
	use crate::store::Rid;

	const SUCCESS: i32 = 0;
	const INVALID_DESCRIPTOR: i32 = -1;
	const FAILED_MEMORY_WRITE: i32 = -3;
	const INVALID_STRING: i32 = -4;
	const INVALID_DATE_STRING: i32 = -5;

	pub fn destroy(ctx: &mut impl Ctx, rid: Rid) {
		ctx.host().store.remove(rid);
	}

	pub fn buffer_len(ctx: &mut impl Ctx, rid: Rid) -> i32 {
		let Some(len) = ctx.host().store.get(rid).and_then(|item| item.as_encoded().map(|d| d.len())) else {
			return INVALID_DESCRIPTOR;
		};
		len as i32
	}

	pub fn read_buffer(ctx: &mut impl Ctx, rid: Rid, ptr: u32, size: u32) -> i32 {
		let Some(data) = ctx
			.host()
			.store
			.get(rid)
			.and_then(|item| item.as_encoded().map(|d| d.to_vec()))
		else {
			return INVALID_DESCRIPTOR;
		};
		if size as usize > data.len() {
			return FAILED_MEMORY_WRITE;
		}
		if ctx.mem_write(ptr, &data[..size as usize]) {
			SUCCESS
		} else {
			FAILED_MEMORY_WRITE
		}
	}

	pub fn current_date(_ctx: &mut impl Ctx) -> f64 {
		chrono::Utc::now().timestamp() as f64
	}

	pub fn utc_offset(_ctx: &mut impl Ctx) -> i64 {
		chrono::Local::now().offset().utc_minus_local() as i64
	}

	#[allow(clippy::too_many_arguments)]
	pub fn parse_date(
		ctx: &mut impl Ctx,
		date_str: u32,
		date_len: u32,
		format_str: u32,
		format_len: u32,
		locale_str: u32,
		locale_len: u32,
		timezone_str: u32,
		timezone_len: u32,
	) -> f64 {
		let invalid = INVALID_DATE_STRING as f64;
		let Some(string) = super::read_string(ctx, date_str, date_len) else {
			return INVALID_STRING as f64;
		};
		let Some(format) = super::read_string(ctx, format_str, format_len) else {
			return INVALID_STRING as f64;
		};
		let timezone_string = if timezone_len > 0 {
			super::read_string(ctx, timezone_str, timezone_len)
		} else {
			None
		};
		let locale_string = if locale_len > 0 {
			super::read_string(ctx, locale_str, locale_len)
		} else {
			None
		};
		parse_date_impl(&string, &format, locale_string.as_deref(), timezone_string.as_deref()).unwrap_or(invalid)
	}

	// ── 以下与生产 crates/wasm/src/imports/std.rs 的 parse_date_impl 逐行相同 ──

	fn parse_date_impl(string: &str, format: &str, locale: Option<&str>, timezone: Option<&str>) -> Option<f64> {
		let translated = swift_date_format_to_chrono(format);
		let input = if translated.has_month_name {
			&*normalize_month_names(string, locale)
		} else {
			string
		};

		let subsec = |nanos: u32| f64::from(nanos) / 1e9;
		if translated.has_zone_offset {
			let dt = chrono::DateTime::parse_from_str(input, &translated.fmt).ok()?;
			return Some(dt.timestamp() as f64 + subsec(dt.timestamp_subsec_nanos()));
		}

		let naive = chrono::NaiveDateTime::parse_from_str(input, &translated.fmt)
			.ok()
			.or_else(|| {
				chrono::NaiveDate::parse_from_str(input, &translated.fmt)
					.ok()
					.and_then(|d| d.and_hms_opt(0, 0, 0))
			})?;
		let dt = naive.and_local_timezone(resolve_timezone(timezone)).single()?;
		Some(dt.timestamp() as f64 + subsec(dt.timestamp_subsec_nanos()))
	}

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

	fn push_literal(fmt: &mut String, c: char) {
		if c == '%' {
			fmt.push_str("%%");
		} else {
			fmt.push(c);
		}
	}

	struct LocaleMonths {
		full: [&'static str; 12],
		abbr: [&'static str; 12],
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

	fn normalize_month_names<'a>(string: &'a str, locale: Option<&str>) -> std::borrow::Cow<'a, str> {
		let lang = locale
			.and_then(|l| l.split(['_', '-']).next())
			.map(str::to_ascii_lowercase)
			.filter(|l| *l != "current" && !l.starts_with("en"));
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
}

pub mod net {
	use super::Ctx;
	use crate::net::{HttpMethod, NetRequest, SendFailure};
	use crate::store::{Rid, StoreItem};

	const SUCCESS: i32 = 0;
	const INVALID_DESCRIPTOR: i32 = -1;
	const INVALID_STRING: i32 = -2;
	const INVALID_URL: i32 = -4;
	const MISSING_DATA: i32 = -7;
	const MISSING_RESPONSE: i32 = -8;
	const REQUEST_ERROR: i32 = -10;
	const FAILED_MEMORY_WRITE: i32 = -11;

	pub fn init(ctx: &mut impl Ctx, method: i32) -> i32 {
		// wasmi 的 host fn 参数不支持 u8，校验挪进函数体（生产由 wasmer 签名挡）
		let Ok(method) = u8::try_from(method) else {
			return -3;
		};
		if method > 8 {
			return -3;
		}
		ctx.host()
			.store
			.store(StoreItem::Request(Box::new(NetRequest::new(HttpMethod::from(method)))))
	}

	fn common_send(ctx: &mut impl Ctx, rid: Rid) -> i32 {
		let (timeout, body_max) = {
			let host = ctx.host();
			let Some(request) = host.store.get_mut(rid).and_then(|item| item.as_request()) else {
				return INVALID_DESCRIPTOR;
			};
			let timeout = request
				.timeout
				.map(|t| std::time::Duration::from_secs_f64(t.max(0.0)))
				.map(|t| t.min(host.limits.net_timeout_max))
				.unwrap_or(host.limits.net_timeout_default);
			(timeout, host.limits.net_body_max)
		};
		// 请求对象临时取出再发（生产 draw_image 同款 take/restore）：
		// 发出后要写回 response，与 client 同格借用会顶 borrow checker
		let Some(mut item) = ctx.host().store.take(rid) else {
			return INVALID_DESCRIPTOR;
		};
		let Some(request) = item.as_request() else {
			return INVALID_DESCRIPTOR;
		};
		let result = crate::net::perform(&ctx.host().net_client, request, timeout, body_max);
		ctx.host().store.restore(rid, item);
		match result {
			Ok(()) => SUCCESS,
			Err(SendFailure::InvalidUrl) => INVALID_URL,
			Err(SendFailure::Request | SendFailure::BodyTooLarge) => REQUEST_ERROR,
		}
	}

	pub fn send(ctx: &mut impl Ctx, rid: Rid) -> i32 {
		common_send(ctx, rid)
	}

	pub fn send_all(ctx: &mut impl Ctx, rid_ptr: u32, len: u32) -> i32 {
		let Some(bytes) = ctx.mem_read(rid_ptr, len * 4) else {
			return INVALID_DESCRIPTOR;
		};
		let rids: Vec<i32> = bytes.as_chunks::<4>().0.iter().map(|c| i32::from_le_bytes(*c)).collect();
		let mut was_error = false;
		let mut errors = Vec::new();
		for rid in rids {
			let result = common_send(ctx, rid);
			if result != SUCCESS {
				was_error = true;
			}
			errors.push(result);
		}
		let mut out = Vec::with_capacity(errors.len() * 4);
		for e in errors {
			out.extend_from_slice(&e.to_le_bytes());
		}
		if !ctx.mem_write(rid_ptr, &out) {
			FAILED_MEMORY_WRITE
		} else if was_error {
			REQUEST_ERROR
		} else {
			SUCCESS
		}
	}

	pub fn set_url(ctx: &mut impl Ctx, rid: Rid, ptr: u32, len: u32) -> i32 {
		let Some(string) = super::read_string(ctx, ptr, len) else {
			return INVALID_STRING;
		};
		let Ok(url) = url::Url::parse(&string) else {
			return INVALID_URL;
		};
		let Some(request) = ctx.host().store.get_mut(rid).and_then(|item| item.as_request()) else {
			return INVALID_DESCRIPTOR;
		};
		request.url = Some(url);
		SUCCESS
	}

	pub fn set_header(ctx: &mut impl Ctx, rid: Rid, key_ptr: u32, key_len: u32, val_ptr: u32, val_len: u32) -> i32 {
		let Some(key) = super::read_string(ctx, key_ptr, key_len) else {
			return INVALID_STRING;
		};
		let Some(val) = super::read_string(ctx, val_ptr, val_len) else {
			return INVALID_STRING;
		};
		let Some(request) = ctx.host().store.get_mut(rid).and_then(|item| item.as_request()) else {
			return INVALID_DESCRIPTOR;
		};
		request.headers.push((key, val));
		SUCCESS
	}

	pub fn set_body(ctx: &mut impl Ctx, rid: Rid, ptr: u32, len: u32) -> i32 {
		let Some(body) = ctx.mem_read(ptr, len) else {
			return INVALID_STRING;
		};
		let Some(request) = ctx.host().store.get_mut(rid).and_then(|item| item.as_request()) else {
			return INVALID_DESCRIPTOR;
		};
		request.body = Some(body);
		SUCCESS
	}

	pub fn set_timeout(ctx: &mut impl Ctx, rid: Rid, value: f64) -> i32 {
		let Some(request) = ctx.host().store.get_mut(rid).and_then(|item| item.as_request()) else {
			return INVALID_DESCRIPTOR;
		};
		request.timeout = Some(value);
		SUCCESS
	}

	fn with_response<T>(ctx: &mut impl Ctx, rid: Rid, missing: i32, f: impl FnOnce(&crate::net::NetResponse) -> T) -> Result<T, i32> {
		let Some(request) = ctx.host().store.get_mut(rid).and_then(|item| item.as_request()) else {
			return Err(INVALID_DESCRIPTOR);
		};
		let Some(response) = request.response.as_ref() else {
			return Err(missing);
		};
		Ok(f(response))
	}

	pub fn data_len(ctx: &mut impl Ctx, rid: Rid) -> i32 {
		match with_response(ctx, rid, MISSING_RESPONSE, |r| r.data.len() as i32) {
			Ok(v) => v,
			Err(e) => e,
		}
	}

	pub fn read_data(ctx: &mut impl Ctx, rid: Rid, buffer: u32, size: u32) -> i32 {
		let data = match with_response(ctx, rid, MISSING_RESPONSE, |r| r.data.clone()) {
			Ok(d) => d,
			Err(e) => return e,
		};
		if size as usize > data.len() {
			return FAILED_MEMORY_WRITE;
		}
		if ctx.mem_write(buffer, &data[..size as usize]) {
			SUCCESS
		} else {
			FAILED_MEMORY_WRITE
		}
	}

	pub fn get_status_code(ctx: &mut impl Ctx, rid: Rid) -> i32 {
		match with_response(ctx, rid, MISSING_RESPONSE, |r| r.status as i32) {
			Ok(v) => v,
			Err(e) => e,
		}
	}

	pub fn get_url(ctx: &mut impl Ctx, rid: Rid) -> i32 {
		let url = match with_response(ctx, rid, MISSING_RESPONSE, |r| r.url.clone()) {
			Ok(u) => u,
			Err(e) => return e,
		};
		ctx.host().store.store(StoreItem::String(url))
	}

	pub fn get_header(ctx: &mut impl Ctx, rid: Rid, key_ptr: u32, key_len: u32) -> i32 {
		let Some(key) = super::read_string(ctx, key_ptr, key_len) else {
			return INVALID_STRING;
		};
		let key = key.to_ascii_lowercase();
		let value = match with_response(ctx, rid, MISSING_RESPONSE, |r| {
			r.headers
				.iter()
				.filter(|(k, _)| k.eq_ignore_ascii_case(&key))
				.map(|(_, v)| v.clone())
				.collect::<Vec<_>>()
				.join(", ")
		}) {
			Ok(v) => v,
			Err(e) => return e,
		};
		if value.is_empty() {
			return MISSING_DATA;
		}
		ctx.host().store.store(StoreItem::String(value))
	}

	/// 生产同款函数（引擎无关的 RateLimiter），spike 不重演节流行为
	pub fn set_rate_limit(_ctx: &mut impl Ctx, _permits: i32, _period: i32, _unit: i32) {}
}
