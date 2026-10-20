use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Limits {
	/// 单次导出调用的指令预算（wasmi fuel）：每次调用前重置，耗尽即 trap。
	/// 10B 会把 godamanga 这类病态源的长章列表（数千章 × 每章现编正则，实测
	/// 3875 章纯计算 ~7s ≈ 50B）打死；wall-clock 的 call_timeout 才是主兜底，
	/// fuel 只防「超时内的指令级失控」，放 100B
	pub fuel: u64,
	/// 调用级 wall-clock 超时，超时熔断重启实例
	pub call_timeout: Duration,
	/// env.sleep 单次上限
	pub sleep_max: Duration,
	pub net_timeout_default: Duration,
	pub net_timeout_max: Duration,
	/// net host 单次响应体上限（解码后字节）；0 = 不设限
	pub net_body_max: u64,
	/// 实例线性内存上限（StoreLimits），memory.grow 超过即失败
	pub memory_max: u64,
	/// 单次 js eval 的执行上限（脚本入口 wall-clock，budget 让出点检查）
	pub js_timeout: Duration,
	/// js 单循环迭代上限：promise reaction job 内死循环的最后兜底（VM 回边检查）
	pub js_loop_max: u64,
	/// webview 浏览器导航超时；必须低于 call_timeout，否则熔断先于超时生效
	pub browser_timeout: Duration,
}

impl Default for Limits {
	fn default() -> Self {
		Self {
			fuel: 100_000_000_000,
			call_timeout: Duration::from_secs(60),
			sleep_max: Duration::from_secs(10),
			net_timeout_default: Duration::from_secs(30),
			net_timeout_max: Duration::from_secs(120),
			// 生态里最大的合法单体是长条漫整页图（常见几 MiB，极端约 20 MiB），32 MiB 留余量
			net_body_max: 32 * 1024 * 1024,
			// Aidoku 源实测很少超 64 MiB；压太低会让 memory.grow 失败直接打死源
			memory_max: 128 * 1024 * 1024,
			js_timeout: Duration::from_secs(10),
			js_loop_max: 1_000_000_000,
			browser_timeout: Duration::from_secs(30),
		}
	}
}
