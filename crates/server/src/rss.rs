//! 进程当前 RSS 读取。macOS 走 mach task_info，Linux 读 /proc/self/statm；
//! 其余平台恒 None，build_app 据此不启动 RSS 监控

#[cfg(target_os = "macos")]
pub fn current_rss() -> Option<u64> {
	// libc 的 mach_task_self() 标了 deprecated（指向 mach2）；符号只是读
	// mach_task_self_ 全局，为几行 FFI 新引一个 crate 不值，自己声明
	unsafe extern "C" {
		static mach_task_self_: libc::mach_port_t;
	}
	let mut info: libc::mach_task_basic_info = unsafe { std::mem::zeroed() };
	let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
	let kr = unsafe {
		libc::task_info(
			mach_task_self_,
			libc::MACH_TASK_BASIC_INFO,
			&mut info as *mut _ as libc::task_info_t,
			&mut count,
		)
	};
	if kr != libc::KERN_SUCCESS {
		return None;
	}
	// mach_task_basic_info 是 repr(packed(4))，按值拷贝避免未对齐引用
	let resident_size = info.resident_size;
	Some(resident_size)
}

#[cfg(target_os = "linux")]
pub fn current_rss() -> Option<u64> {
	// statm 第二字段是常驻内存页数
	let raw = std::fs::read_to_string("/proc/self/statm").ok()?;
	let pages: u64 = raw.split_whitespace().nth(1)?.parse().ok()?;
	let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
	if page_size <= 0 {
		return None;
	}
	Some(pages * page_size as u64)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn current_rss() -> Option<u64> {
	None
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
	#[test]
	fn current_rss_reports_a_live_process() {
		let rss = super::current_rss().expect("supported platform");
		assert!(rss > 0);
	}
}
