//! 从章节名解析章节号（Tachiyomi ChapterRecognition 的子集，按 issue 点名的
//! 正则形态）：`第?N话`（含 話/回/卷）、`ch.?\s*N`（含 chapter）、`vol.?\s*N`
//! （含 volume）。三类按此序取首个命中；都落空返回 None，由调用方退自然序。

/// 章节号解析入口：CJK 计数形态优先，其次 chapter 系，最后 volume 系
pub fn parse_chapter_number(name: &str) -> Option<f32> {
	parse_cjk(name)
		.or_else(|| parse_keyword(name, &["chapter", "ch"]))
		.or_else(|| parse_keyword(name, &["volume", "vol"]))
}

/// `第?N话`：计数字（话/話/回/卷）前紧邻的十进制数，「第」可缺席
fn parse_cjk(name: &str) -> Option<f32> {
	let chars: Vec<char> = name.chars().collect();
	for (i, c) in chars.iter().enumerate() {
		if !matches!(c, '话' | '話' | '回' | '卷') {
			continue;
		}
		let mut j = i;
		while j > 0 && chars[j - 1].is_whitespace() {
			j -= 1;
		}
		let end = j;
		while j > 0 && (chars[j - 1].is_ascii_digit() || chars[j - 1] == '.') {
			j -= 1;
		}
		if j == end {
			continue;
		}
		let num: String = chars[j..end].iter().collect();
		if let Ok(n) = num.parse::<f32>() {
			return Some(n);
		}
	}
	None
}

/// 关键词后跟数字：关键词前不能紧贴字母数字（school 里的 ch 不算），
/// 后容许 `.` 与空白再跟数字；keywords 长的在前避免 chapter 被 ch 截胡
fn parse_keyword(name: &str, keywords: &[&str]) -> Option<f32> {
	let lower = name.to_lowercase();
	let bytes = lower.as_bytes();
	for i in 0..bytes.len() {
		if i > 0 && bytes[i - 1].is_ascii_alphanumeric() {
			continue;
		}
		for kw in keywords {
			let Some(after) = lower.get(i..).and_then(|s| s.strip_prefix(kw)) else {
				continue;
			};
			if after.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) {
				continue;
			}
			let after = after.trim_start_matches('.').trim_start();
			if let Some(n) = parse_leading_number(after) {
				return Some(n);
			}
		}
	}
	None
}

fn parse_leading_number(s: &str) -> Option<f32> {
	let mut end = 0;
	for (i, c) in s.char_indices() {
		if c.is_ascii_digit() || c == '.' {
			end = i + c.len_utf8();
		} else {
			break;
		}
	}
	s[..end].parse::<f32>().ok()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn cjk_forms() {
		assert_eq!(parse_chapter_number("第3话"), Some(3.0));
		assert_eq!(parse_chapter_number("3话"), Some(3.0));
		assert_eq!(parse_chapter_number("第 10 話"), Some(10.0));
		assert_eq!(parse_chapter_number("第12回"), Some(12.0));
		assert_eq!(parse_chapter_number("第10卷"), Some(10.0));
		assert_eq!(parse_chapter_number("Vol.2 第5话"), Some(5.0));
	}

	#[test]
	fn chapter_keywords() {
		assert_eq!(parse_chapter_number("ch 10"), Some(10.0));
		assert_eq!(parse_chapter_number("Ch. 10.5"), Some(10.5));
		assert_eq!(parse_chapter_number("Chapter 2"), Some(2.0));
		assert_eq!(parse_chapter_number("[group] title ch007"), Some(7.0));
	}

	#[test]
	fn volume_keywords() {
		assert_eq!(parse_chapter_number("vol 3"), Some(3.0));
		assert_eq!(parse_chapter_number("Volume 11"), Some(11.0));
		assert_eq!(parse_chapter_number("Vol.2"), Some(2.0));
	}

	#[test]
	fn no_match_falls_back() {
		assert_eq!(parse_chapter_number("extras"), None);
		assert_eq!(parse_chapter_number("page 10"), None);
		assert_eq!(parse_chapter_number("school 2"), None);
		assert_eq!(parse_chapter_number("chapters"), None);
		assert_eq!(parse_chapter_number(""), None);
	}
}
