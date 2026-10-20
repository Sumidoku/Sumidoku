//! 大小写不敏感自然序比较器：数字段按数值比，其余按字符小写比。
//! 移植自 kmrs komga-core natural_sort.rs（grey-panther natural-comparator 的
//! CaseInsensitiveSimpleNaturalComparator 复刻），页序与章节名排序共用。
//! 语义要点：两侧都是数字段时按 u64 解析比较（相等则继续比后文，a01b < a1c）；
//! 一侧数字一侧非数字，数字侧小；同步耗尽才算相等（a1 与 a01 判等，非全序）。

use std::cmp::Ordering;

pub fn compare(a: &str, b: &str) -> Ordering {
	let s1: Vec<u16> = a.encode_utf16().collect();
	let s2: Vec<u16> = b.encode_utf16().collect();
	let (mut i1, mut i2) = (0usize, 0usize);
	loop {
		if i1 >= s1.len() {
			return if i2 >= s2.len() {
				Ordering::Equal
			} else {
				Ordering::Less
			};
		}
		if i2 >= s2.len() {
			return Ordering::Greater;
		}
		let d1 = is_digit(s1[i1]);
		let d2 = is_digit(s2[i2]);
		match (d1, d2) {
			(true, true) => {
				let (n1, next1) = parse_digits(&s1, i1);
				let (n2, next2) = parse_digits(&s2, i2);
				if n1 != n2 {
					return n1.cmp(&n2);
				}
				i1 = next1;
				i2 = next2;
			}
			(true, false) => return Ordering::Less,
			(false, true) => return Ordering::Greater,
			(false, false) => {
				let l1 = lower(s1[i1]);
				let l2 = lower(s2[i2]);
				match l1.cmp(&l2) {
					Ordering::Equal => {
						i1 += 1;
						i2 += 1;
					}
					ord => return ord,
				}
			}
		}
	}
}

fn is_digit(c: u16) -> bool {
	(b'0' as u16..=b'9' as u16).contains(&c)
}

/// 贪婪解析数字段，u64 溢出回绕（同 Java long 位模式按无符号比较）
fn parse_digits(s: &[u16], start: usize) -> (u64, usize) {
	let mut n: u64 = 0;
	let mut i = start;
	while i < s.len() && is_digit(s[i]) {
		n = n.wrapping_mul(10).wrapping_add((s[i] - b'0' as u16) as u64);
		i += 1;
	}
	(n, i)
}

/// Character.toLowerCase 等价：BMP 取首个小写映射，代理项原样保留
fn lower(c: u16) -> u32 {
	char::from_u32(c as u32)
		.and_then(|ch| ch.to_lowercase().next())
		.map(|ch| ch as u32)
		.unwrap_or(c as u32)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn numeric_runs() {
		assert_eq!(compare("2", "10"), Ordering::Less);
		assert_eq!(compare("a2", "a10"), Ordering::Less);
		assert_eq!(compare("a10", "a2"), Ordering::Greater);
	}

	#[test]
	fn leading_zeros_equal_then_continue() {
		assert_eq!(compare("a01b", "a1c"), Ordering::Less);
		assert_eq!(compare("a1", "a01"), Ordering::Equal);
		assert_eq!(compare("a01", "a1b"), Ordering::Less);
	}

	#[test]
	fn digit_before_non_digit() {
		assert_eq!(compare("1", "a"), Ordering::Less);
		assert_eq!(compare("a", "1"), Ordering::Greater);
	}

	#[test]
	fn case_insensitive() {
		assert_eq!(compare("ABC", "abc"), Ordering::Equal);
		assert_eq!(compare("B", "a"), Ordering::Greater);
	}

	#[test]
	fn typical_page_names() {
		let mut files = vec!["page 10.jpg", "page 2.jpg", "Page 1.jpg", "page 10a.jpg", "cover.jpg"];
		files.sort_by(|a, b| compare(a, b));
		assert_eq!(
			files,
			vec!["cover.jpg", "Page 1.jpg", "page 2.jpg", "page 10.jpg", "page 10a.jpg"]
		);
	}
}
