//! Reads the process's current RSS through sysinfo, which covers every supported platform;
//! build_app skips the RSS monitor only when this probe returns None

pub fn current_rss() -> Option<u64> {
	let pid = sysinfo::Pid::from(std::process::id() as usize);
	// A fresh System per call is fine: memory is an instantaneous reading with no cross-refresh
	// baseline (unlike CPU), and the monitor calls this on a 60s cadence
	let mut sys = sysinfo::System::new().ok()?;
	sys.refresh_processes_specifics(
		sysinfo::ProcessesToUpdate::Some(&[pid]),
		false,
		sysinfo::ProcessRefreshKind::nothing().with_memory(),
	);
	sys.process(pid).map(|p| p.memory())
}

#[cfg(test)]
mod tests {
	#[test]
	fn current_rss_reports_a_live_process() {
		let rss = super::current_rss().expect("sysinfo reads the current process");
		assert!(rss > 0);
	}
}
