fn main() {
	// rust-embed 编译期要求目录存在；webui/build 被 gitignore，干净 checkout 尚未产出
	let dir = std::path::Path::new(
		&std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
	)
	.join("../../webui/build");
	std::fs::create_dir_all(&dir).expect("create webui build dir");
	println!("cargo:rerun-if-changed={}", dir.display());
}
