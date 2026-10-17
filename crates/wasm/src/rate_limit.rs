use std::time::{Duration, Instant};

/// 每源令牌桶。默认不限流；源调 set_rate_limit 后按 permits/period 限速。
#[derive(Default)]
pub struct RateLimiter {
	bucket: Option<Bucket>,
}

struct Bucket {
	tokens: f64,
	capacity: f64,
	refill_per_sec: f64,
	last_refill: Instant,
}

impl RateLimiter {
	pub fn set_rate(&mut self, permits: i32, period: i32, unit: i32) {
		let period_secs = match unit {
			0 => period as f64,
			1 => period as f64 * 60.0,
			2 => period as f64 * 3600.0,
			_ => return,
		};
		if permits <= 0 || period_secs <= 0.0 {
			self.bucket = None;
			return;
		}
		self.bucket = Some(Bucket {
			tokens: permits as f64,
			capacity: permits as f64,
			refill_per_sec: permits as f64 / period_secs,
			last_refill: Instant::now(),
		});
	}

	pub fn acquire(&mut self) {
		let Some(bucket) = self.bucket.as_mut() else {
			return;
		};
		let now = Instant::now();
		let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
		bucket.tokens = (bucket.tokens + elapsed * bucket.refill_per_sec).min(bucket.capacity);
		bucket.last_refill = now;
		if bucket.tokens < 1.0 {
			let wait = Duration::from_secs_f64((1.0 - bucket.tokens) / bucket.refill_per_sec);
			std::thread::sleep(wait);
			bucket.tokens = 0.0;
		} else {
			bucket.tokens -= 1.0;
		}
	}
}

