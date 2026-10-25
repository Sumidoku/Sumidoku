//! Case-insensitive natural-order comparator: digit runs compare by value, everything else by lowercased character.
//! Ported from kmrs komga-core natural_sort.rs (a re-implementation of grey-panther natural-comparator's
//! CaseInsensitiveSimpleNaturalComparator), shared by page order and chapter-name sorting.
//! Semantics: when both sides have a digit run, parse as u64 and compare (on equality continue with what follows, so a01b < a1c);
//! a digit run on only one side sorts smaller; only simultaneous exhaustion counts as equal (a1 equals a01 — not a total order).

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

/// Greedily parses a digit run; u64 overflow wraps (same bit pattern as a Java long, compared unsigned)
fn parse_digits(s: &[u16], start: usize) -> (u64, usize) {
	let mut n: u64 = 0;
	let mut i = start;
	while i < s.len() && is_digit(s[i]) {
		n = n.wrapping_mul(10).wrapping_add((s[i] - b'0' as u16) as u64);
		i += 1;
	}
	(n, i)
}

/// Character.toLowerCase equivalent: BMP takes the first lowercase mapping, surrogates kept as-is
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
