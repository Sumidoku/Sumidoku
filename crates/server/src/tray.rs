//! Windows shell integration: release builds target the windows subsystem (no
//! console), so a running server surfaces through a notification-area icon with
//! an Open/config/logs/autostart/Exit menu. Non-interactive sessions (Windows
//! service, scheduled task) get no icon and the server runs headless.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
#[cfg(not(debug_assertions))]
use windows_sys::Win32::System::Console::FreeConsole;
use windows_sys::Win32::System::Console::{AttachConsole, GetConsoleWindow, ATTACH_PARENT_PROCESS};
use windows_sys::Win32::System::Registry::{
	RegCloseKey, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
	KEY_SET_VALUE, REG_SZ, RRF_RT_REG_SZ,
};
use windows_sys::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
	DispatchMessageW, GetMessageW, MessageBoxW, PeekMessageW, PostThreadMessageW, TranslateMessage, MB_ICONERROR,
	MB_OK, MSG, PM_NOREMOVE, SW_SHOWNORMAL, WM_QUIT, WM_USER,
};

const TRAY_ICON_PNG: &[u8] = include_bytes!("../resources/windows/tray-icon.png");

const AUTOSTART_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const AUTOSTART_VALUE: &str = "Sumidoku";

thread_local! {
	// the menu event handler must be Send+Sync, so the check item (Rc-based)
	// cannot move into it; muda dispatches menu events synchronously on this
	// thread, where the item is reachable again
	static AUTOSTART_ITEM: RefCell<Option<CheckMenuItem>> = const { RefCell::new(None) };
}

pub struct Tray {
	thread_id: u32,
	handle: JoinHandle<()>,
}

/// Folders the tray menu opens in Explorer. Grouped because both are PathBuf —
/// positional parameters of the same type would invite a swapped call site.
pub struct TrayPaths {
	pub config_dir: PathBuf,
	pub logs_dir: PathBuf,
}

/// Reattaches to the parent terminal when launched from one, so CLI errors and
/// startup output have somewhere to print. The shell never waits on a
/// windows-subsystem binary and closing the terminal would kill an attached
/// process, so detach_console() cuts over before serving starts.
pub fn attach_parent_console() {
	unsafe {
		AttachConsole(ATTACH_PARENT_PROCESS);
	}
}

pub fn detach_console() {
	// debug builds stay on the console subsystem (crate attribute in main.rs):
	// there the console is the log output and detaching would strand it
	#[cfg(not(debug_assertions))]
	unsafe {
		FreeConsole();
	}
}

/// Last-resort error surface for a fatal startup error: without a console the
/// process would otherwise vanish without a word. No-op for terminal launches,
/// which already see the error printed.
pub fn show_startup_error(error: &dyn std::error::Error) {
	if unsafe { GetConsoleWindow() }.is_null() {
		let text = wide(&format!("{error}\n\nLog files are in the \"logs\" folder of the data directory."));
		let title = wide("Sumidoku failed to start");
		unsafe {
			MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_ICONERROR | MB_OK);
		}
	}
}

/// Opens the web UI in the default browser.
pub fn open_in_browser(url: &str) {
	shell_open(&wide(url));
}

/// Returns None in non-interactive sessions (Windows service, headless task):
/// there is no shell to show the icon in, and the server runs unaffected.
pub fn start(url: String, paths: TrayPaths, shutdown: tokio::sync::watch::Sender<bool>) -> Option<Tray> {
	let interactive = unsafe {
		let desktop = OpenInputDesktop(0, 0, DESKTOP_READOBJECTS);
		if desktop.is_null() {
			false
		} else {
			CloseDesktop(desktop);
			true
		}
	};
	if !interactive {
		return None;
	}
	let (id_tx, id_rx) = mpsc::channel();
	let (ready_tx, ready_rx) = mpsc::channel();
	let handle = std::thread::Builder::new()
		.name("tray".into())
		.spawn(move || run(url, paths, shutdown, id_tx, ready_tx))
		.ok()?;
	let thread_id = match id_rx.recv_timeout(Duration::from_secs(10)) {
		Ok(id) => id,
		Err(_) => {
			tracing::warn!("system tray disabled: message thread did not start");
			let _ = handle.join();
			return None;
		}
	};
	match ready_rx.recv_timeout(Duration::from_secs(10)) {
		Ok(Ok(())) => Some(Tray { thread_id, handle }),
		Ok(Err(e)) => {
			tracing::warn!("system tray disabled: {e}");
			let _ = handle.join();
			None
		}
		Err(_) => {
			tracing::warn!("system tray disabled: initialization timed out");
			unsafe {
				PostThreadMessageW(thread_id, WM_QUIT, 0, 0);
			}
			let _ = handle.join();
			None
		}
	}
}

impl Tray {
	/// Quits the tray thread's message loop and joins it, so the icon is removed
	/// from the notification area instead of lingering as a ghost until hovered.
	pub fn stop(self) {
		unsafe {
			PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
		}
		let _ = self.handle.join();
	}
}

fn run(
	url: String,
	paths: TrayPaths,
	shutdown: tokio::sync::watch::Sender<bool>,
	thread_id: mpsc::Sender<u32>,
	ready: mpsc::Sender<Result<(), String>>,
) {
	let mut msg = MSG::default();
	unsafe {
		// create the queue before any fallible setup so a start timeout can always
		// post WM_QUIT to this thread instead of leaving it behind
		PeekMessageW(&mut msg, std::ptr::null_mut(), WM_USER, WM_USER, PM_NOREMOVE);
	}
	let _ = thread_id.send(unsafe { GetCurrentThreadId() });
	let open_item = MenuItem::with_id("open", "Open", true, None);
	let config_item = MenuItem::with_id("config", "Open config dir", true, None);
	let logs_item = MenuItem::with_id("logs", "Open logs dir", true, None);
	let autostart_dir = paths.config_dir.clone();
	let autostart_item = CheckMenuItem::with_id("autostart", "Start with Windows", true, autostart_enabled(), None);
	let exit_item = MenuItem::with_id("exit", "Exit", true, None);
	let menu = Menu::new();
	let _ = menu.append(&open_item);
	let config_target = wide_os(paths.config_dir.as_os_str());
	let _ = menu.append(&config_item);
	// without a console the log files are the only place startup/scan errors show
	// up, so the menu only offers the folder when file logging came up
	let logs_target = paths.logs_dir.is_dir().then(|| wide_os(paths.logs_dir.as_os_str()));
	if logs_target.is_some() {
		let _ = menu.append(&logs_item);
	}
	let _ = menu.append(&autostart_item);
	let _ = menu.append(&PredefinedMenuItem::separator());
	let _ = menu.append(&exit_item);
	AUTOSTART_ITEM.with(|slot| *slot.borrow_mut() = Some(autostart_item));

	let tray = (|| {
		let icon = load_icon().map_err(|e| format!("tray icon decode: {e}"))?;
		TrayIconBuilder::new()
			.with_menu(Box::new(menu))
			// default true would pop the menu on left-button-up, making a plain
			// left click (our open-UI gesture) unreachable
			.with_menu_on_left_click(false)
			.with_tooltip("Sumidoku")
			.with_icon(icon)
			.build()
			.map_err(|e| e.to_string())
	})();
	let tray = match tray {
		Ok(tray) => tray,
		Err(e) => {
			let _ = ready.send(Err(e));
			return;
		}
	};

	let menu_url = url.clone();
	MenuEvent::set_event_handler(Some(move |event: MenuEvent| match event.id.0.as_str() {
		"open" => open_in_browser(&menu_url),
		"logs" => {
			if let Some(target) = &logs_target {
				shell_open(target);
			}
		}
		"config" => shell_open(&config_target),
		"autostart" => {
			let enable = !autostart_enabled();
			if let Err(e) = set_autostart(enable, &autostart_dir) {
				tracing::warn!("startup registration unchanged: {e}");
			}
			// display what the registry holds, not what the click flipped to: an
			// out-of-band edit would otherwise leave the mark inverted all session
			AUTOSTART_ITEM.with(|slot| {
				if let Some(item) = slot.borrow().as_ref() {
					item.set_checked(autostart_enabled());
				}
			});
		}
		"exit" => {
			let _ = shutdown.send(true);
		}
		_ => {}
	}));
	TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
		if let TrayIconEvent::Click {
			button: MouseButton::Left,
			button_state: MouseButtonState::Up,
			..
		} = event
		{
			open_in_browser(&url);
		}
	}));

	let _ = ready.send(Ok(()));
	unsafe {
		while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
			TranslateMessage(&msg);
			DispatchMessageW(&msg);
		}
	}
	drop(tray);
}

fn load_icon() -> Result<Icon, String> {
	let image = image::load_from_memory_with_format(TRAY_ICON_PNG, image::ImageFormat::Png)
		.map_err(|e| e.to_string())?
		.into_rgba8();
	let (width, height) = image.dimensions();
	Icon::from_rgba(image.into_raw(), width, height).map_err(|e| e.to_string())
}

fn wide(s: &str) -> Vec<u16> {
	s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_os(s: &std::ffi::OsStr) -> Vec<u16> {
	use std::os::windows::ffi::OsStrExt;
	s.encode_wide().chain(std::iter::once(0)).collect()
}

fn shell_open(target: &[u16]) {
	let verb = wide("open");
	unsafe {
		let result = ShellExecuteW(
			std::ptr::null_mut(),
			verb.as_ptr(),
			target.as_ptr(),
			std::ptr::null(),
			std::ptr::null(),
			SW_SHOWNORMAL,
		);
		// the API's legacy return contract: values ≤ 32 are failure codes
		if result as isize <= 32 {
			tracing::warn!("ShellExecute open failed (code {})", result as isize);
		}
	}
}

fn autostart_enabled() -> bool {
	let subkey = wide(AUTOSTART_SUBKEY);
	let value = wide(AUTOSTART_VALUE);
	unsafe {
		RegGetValueW(
			HKEY_CURRENT_USER,
			subkey.as_ptr(),
			value.as_ptr(),
			RRF_RT_REG_SZ,
			std::ptr::null_mut(),
			std::ptr::null_mut(),
			std::ptr::null_mut(),
		) == ERROR_SUCCESS
	}
}

fn set_autostart(enable: bool, config_dir: &std::path::Path) -> Result<(), String> {
	// resolved before opening the key: a failure here must not leak the handle
	let command = enable.then(|| autostart_command(config_dir)).transpose()?;
	let subkey = wide(AUTOSTART_SUBKEY);
	let name = wide(AUTOSTART_VALUE);
	unsafe {
		let mut hkey: HKEY = std::ptr::null_mut();
		let err = RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_SET_VALUE, &mut hkey);
		if err != ERROR_SUCCESS {
			return Err(format!("open Run key: error {err}"));
		}
		let err = match &command {
			Some(data) => RegSetValueExW(hkey, name.as_ptr(), 0, REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32),
			None => RegDeleteValueW(hkey, name.as_ptr()),
		};
		RegCloseKey(hkey);
		if err == ERROR_SUCCESS {
			Ok(())
		} else {
			Err(format!("write Run value: error {err}"))
		}
	}
}

fn autostart_command(config_dir: &std::path::Path) -> Result<Vec<u16>, String> {
	let exe = std::env::current_exe().map_err(|e| format!("current exe: {e}"))?;
	// a logon start gets an arbitrary working directory, and the default config
	// dir off Windows is CWD-relative: pin it absolute
	let config_dir = std::path::absolute(config_dir).map_err(|e| format!("config dir: {e}"))?;
	Ok(wide(&crate::desktop::autostart_command(&exe, &config_dir)))
}
