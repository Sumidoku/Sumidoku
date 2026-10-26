//! Desktop-entry helpers: pure functions shared by `main`, `config` and the
//! Windows tray, so the argument parsing, address mapping and autostart command
//! stay testable on any host.

use std::ffi::OsString;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

/// A desktop install binds loopback only: the default `0.0.0.0` with auth
/// disabled would expose the instance to the LAN, which is right for a
/// container but not for a double-clicked app
pub const fn default_addr() -> &'static str {
	if cfg!(target_os = "windows") {
		"127.0.0.1:3000"
	} else {
		"0.0.0.0:3000"
	}
}

pub fn default_config_dir() -> PathBuf {
	// cfg! (not #[cfg]) keeps windows_config_dir referenced on every host — the
	// pure helper is compiled and tested everywhere, only its result is Windows-only
	if cfg!(target_os = "windows")
		&& let Some(dir) =
			windows_config_dir(std::env::var_os("LOCALAPPDATA"), std::env::var_os("USERPROFILE"))
	{
		return dir;
	}
	PathBuf::from("./data")
}

/// Per-user data root for the desktop build: a shortcut or logon start gets an
/// arbitrary working directory, so the CWD-relative `./data` default is
/// unpredictable there. LOCALAPPDATA is the canonical root; the USERPROFILE
/// derivation covers profiles where it is somehow unset.
fn windows_config_dir(local_appdata: Option<OsString>, userprofile: Option<OsString>) -> Option<PathBuf> {
	if let Some(dir) = local_appdata.filter(|v| !v.is_empty()) {
		return Some(PathBuf::from(dir).join("Sumidoku"));
	}
	userprofile
		.filter(|v| !v.is_empty())
		.map(|home| PathBuf::from(home).join(r"AppData\Local").join("Sumidoku"))
}

/// The only supported CLI argument is `--config-dir <DIR>` (also
/// `--config-dir=<DIR>`); the tray's Start-with-Windows entry relies on it.
/// Anything else — a missing value or an unknown argument — is a startup
/// error, since silently ignoring a mistyped flag would start the server
/// against an unintended data root.
pub fn parse_config_dir_args(args: impl IntoIterator<Item = OsString>) -> Result<Option<PathBuf>, String> {
	let mut config_dir: Option<PathBuf> = None;
	let mut args = args.into_iter();
	while let Some(arg) = args.next() {
		let value = if arg == "--config-dir" {
			match args.next() {
				// a value starting with `--` is a mistyped flag, not a path; a
				// non-UTF-8 value (legal in a Windows path) is still a value
				Some(v) if !v.to_str().is_some_and(|s| s.starts_with("--")) => Some(v),
				_ => return Err("option '--config-dir' requires a value".into()),
			}
		} else if let Some(rest) = arg.to_str().and_then(|s| s.strip_prefix("--config-dir=")) {
			if rest.is_empty() {
				return Err("option '--config-dir' requires a value".into());
			}
			Some(OsString::from(rest))
		} else {
			return Err(format!(
				"unknown argument '{}' (usage: sumidoku-server [--config-dir <DIR>])",
				arg.to_string_lossy()
			));
		};
		if config_dir.is_some() {
			return Err("option '--config-dir' given more than once".into());
		}
		config_dir = value.map(PathBuf::from);
	}
	Ok(config_dir)
}

/// Maps the bind address to a URL the local browser can navigate to: wildcard
/// addresses are listen-only, so `0.0.0.0`/`::` become loopback. Anything not
/// parsing as a socket address (a hostname) passes through untouched.
pub fn browser_url(addr: &str) -> String {
	match addr.parse::<SocketAddr>() {
		Ok(sa) => {
			let ip = if sa.ip().is_unspecified() {
				IpAddr::V4(Ipv4Addr::LOCALHOST)
			} else {
				sa.ip()
			};
			format!("http://{}/", SocketAddr::new(ip, sa.port()))
		}
		Err(_) => format!("http://{addr}/"),
	}
}

/// The Run-key command line for Start with Windows. The registry parses it as
/// a command line, so both paths are quoted (spaces); a logon start gets an
/// arbitrary working directory, so the config dir must travel with the command.
pub fn autostart_command(exe: &Path, config_dir: &Path) -> String {
	let mut command = String::new();
	push_quoted(&mut command, &exe.to_string_lossy());
	command.push_str(" --config-dir ");
	push_quoted(&mut command, &config_dir.to_string_lossy());
	command
}

fn push_quoted(out: &mut String, value: &str) {
	out.push('"');
	out.push_str(value);
	// CommandLineToArgvW reads a backslash right before the closing quote as
	// escaping it, so a trailing-backslash path must double it
	if value.ends_with('\\') {
		out.push('\\');
	}
	out.push('"');
}

#[cfg(test)]
mod tests {
	use super::*;

	fn args(list: &[&str]) -> Vec<OsString> {
		list.iter().map(OsString::from).collect()
	}

	#[test]
	fn windows_config_dir_prefers_local_appdata() {
		let dir = windows_config_dir(Some(r"C:\Users\me\AppData\Local".into()), Some(r"C:\Users\me".into()));
		assert_eq!(dir, Some(PathBuf::from(r"C:\Users\me\AppData\Local").join("Sumidoku")));
	}

	#[test]
	fn windows_config_dir_falls_back_to_userprofile() {
		let dir = windows_config_dir(None, Some(r"C:\Users\me".into()));
		assert_eq!(dir, Some(PathBuf::from(r"C:\Users\me").join(r"AppData\Local").join("Sumidoku")));
		// an empty LOCALAPPDATA counts as unset, same as absent
		let dir = windows_config_dir(Some("".into()), Some(r"C:\Users\me".into()));
		assert_eq!(dir, Some(PathBuf::from(r"C:\Users\me").join(r"AppData\Local").join("Sumidoku")));
		assert_eq!(windows_config_dir(None, None), None);
	}

	#[test]
	fn parse_config_dir_args_accepts_space_and_equals_forms() {
		assert_eq!(parse_config_dir_args(args(&[])).unwrap(), None);
		assert_eq!(
			parse_config_dir_args(args(&["--config-dir", r"C:\data sumi"])).unwrap(),
			Some(PathBuf::from(r"C:\data sumi"))
		);
		assert_eq!(
			parse_config_dir_args(args(&["--config-dir=relative/dir"])).unwrap(),
			Some(PathBuf::from("relative/dir"))
		);
	}

	#[test]
	fn parse_config_dir_args_rejects_missing_value_unknown_and_duplicates() {
		let err = parse_config_dir_args(args(&["--config-dir"])).unwrap_err();
		assert!(err.contains("requires a value"), "{err}");
		let err = parse_config_dir_args(args(&["--config-dir="])).unwrap_err();
		assert!(err.contains("requires a value"), "{err}");
		let err = parse_config_dir_args(args(&["--config-dir", "--verbose"])).unwrap_err();
		assert!(err.contains("requires a value"), "{err}");
		let err = parse_config_dir_args(args(&["--port", "3000"])).unwrap_err();
		assert!(err.contains("unknown argument"), "{err}");
		let err = parse_config_dir_args(args(&["positional"])).unwrap_err();
		assert!(err.contains("unknown argument"), "{err}");
		let err = parse_config_dir_args(args(&["--config-dir", "a", "--config-dir", "b"])).unwrap_err();
		assert!(err.contains("more than once"), "{err}");
	}

	#[test]
	fn browser_url_maps_wildcards_to_loopback() {
		assert_eq!(browser_url("0.0.0.0:3000"), "http://127.0.0.1:3000/");
		assert_eq!(browser_url("[::]:3000"), "http://127.0.0.1:3000/");
		assert_eq!(browser_url("127.0.0.1:8080"), "http://127.0.0.1:8080/");
		// specific and v6 addresses keep their identity, v6 stays bracketed
		assert_eq!(browser_url("192.168.1.10:3000"), "http://192.168.1.10:3000/");
		assert_eq!(browser_url("[::1]:3000"), "http://[::1]:3000/");
		// hostnames are not socket addresses and pass through
		assert_eq!(browser_url("localhost:3000"), "http://localhost:3000/");
	}

	#[test]
	fn autostart_command_quotes_both_paths() {
		let command = autostart_command(
			Path::new(r"C:\Program Files\Sumidoku\sumidoku-server.exe"),
			Path::new(r"C:\Users\me\AppData\Local\Sumidoku"),
		);
		assert_eq!(
			command,
			r#""C:\Program Files\Sumidoku\sumidoku-server.exe" --config-dir "C:\Users\me\AppData\Local\Sumidoku""#
		);
	}

	#[test]
	fn autostart_command_doubles_a_trailing_backslash() {
		let command = autostart_command(Path::new(r"C:\app\server.exe"), Path::new(r"C:\data\"));
		assert_eq!(command, r#""C:\app\server.exe" --config-dir "C:\data\\""#);
	}
}
