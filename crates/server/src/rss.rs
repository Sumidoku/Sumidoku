//! Reads the process's current RSS. macOS goes through mach task_info, Linux reads /proc/self/statm;
//! other platforms always return None, and build_app uses that to skip the RSS monitor

#[cfg(target_os = "macos")]
pub fn current_rss() -> Option<u64> {
	// libc's mach_task_self() is marked deprecated (pointing at mach2); the symbol only reads
	// the mach_task_self_ global, and a new crate for a few lines of FFI is not worth it, so declare it here
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
	// mach_task_basic_info is repr(packed(4)); copy by value to avoid an unaligned reference
	let resident_size = info.resident_size;
	Some(resident_size)
}

#[cfg(target_os = "linux")]
pub fn current_rss() -> Option<u64> {
	// statm's second field is the resident memory page count
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
