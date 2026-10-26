fn main() {
	// rust-embed requires the folder to exist at compile time; webui/build is
	// gitignored, so a fresh checkout does not have it yet
	let dir =
		std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
			.join("../../webui/build");
	std::fs::create_dir_all(&dir).expect("create webui build dir");
	println!("cargo:rerun-if-changed={}", dir.display());

	// Explorer, taskbar and shortcut icons all come from the exe's embedded
	// resources; without this sumidoku-server.exe shows the stock application icon
	if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
		println!("cargo:rerun-if-changed=resources/windows/sumidoku.rc");
		println!("cargo:rerun-if-changed=resources/windows/sumidoku.ico");
		// manifest_optional skips hosts without a resource compiler (local cross
		// `cargo check`) instead of failing the build over a cosmetic asset;
		// windows-latest CI has rc.exe, so a genuine compile error there still
		// fails the build
		embed_resource::compile("resources/windows/sumidoku.rc", embed_resource::NONE)
			.manifest_optional()
			.unwrap();
	}
}
