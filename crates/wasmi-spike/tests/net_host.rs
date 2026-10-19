//! net 族实测：WAT guest 走 Aidoku 真实调用序
//! （init → set_url → send → data_len/read_data/get_status_code；
//! get_header → std.buffer_len/read_buffer 回读头部字符串），
//! 打本地裸 HTTP mock，两引擎同一套断言
use std::io::{Read, Write};
use std::net::TcpListener;
use wasmi_spike::wasmer_host::WasmerHost;
use wasmi_spike::wasmi_host::WasmiHost;
use wasmi_spike::{Host, Limits};

const NET_WAT: &str = r#"
(module
  (import "net" "init" (func $init (param i32) (result i32)))
  (import "net" "set_url" (func $set_url (param i32 i32 i32) (result i32)))
  (import "net" "send" (func $send (param i32) (result i32)))
  (import "net" "data_len" (func $data_len (param i32) (result i32)))
  (import "net" "read_data" (func $read_data (param i32 i32 i32) (result i32)))
  (import "net" "get_status_code" (func $get_status_code (param i32) (result i32)))
  (import "net" "get_header" (func $get_header (param i32 i32 i32) (result i32)))
  (import "std" "buffer_len" (func $buffer_len (param i32) (result i32)))
  (import "std" "read_buffer" (func $read_buffer (param i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "start"))
  (func $open (param $url_ptr i32) (param $url_len i32) (result i32)
    (local $rid i32)
    (local.set $rid (call $init (i32.const 0)))
    (if (i32.lt_s (local.get $rid) (i32.const 0))
      (then (return (local.get $rid))))
    (if (i32.ne (call $set_url (local.get $rid) (local.get $url_ptr) (local.get $url_len)) (i32.const 0))
      (then (return (i32.const -100))))
    (if (i32.ne (call $send (local.get $rid)) (i32.const 0))
      (then (return (i32.const -101))))
    (local.get $rid))
  (func (export "fetch_status") (param $url_ptr i32) (param $url_len i32) (result i32)
    (local $rid i32)
    (local.set $rid (call $open (local.get $url_ptr) (local.get $url_len)))
    (if (i32.lt_s (local.get $rid) (i32.const 0))
      (then (return (local.get $rid))))
    (call $get_status_code (local.get $rid)))
  (func (export "fetch_body") (param $url_ptr i32) (param $url_len i32) (param $out i32) (result i32)
    (local $rid i32) (local $len i32)
    (local.set $rid (call $open (local.get $url_ptr) (local.get $url_len)))
    (if (i32.lt_s (local.get $rid) (i32.const 0))
      (then (return (local.get $rid))))
    (local.set $len (call $data_len (local.get $rid)))
    (if (i32.lt_s (local.get $len) (i32.const 0))
      (then (return (local.get $len))))
    (if (i32.ne (call $read_data (local.get $rid) (local.get $out) (local.get $len)) (i32.const 0))
      (then (return (i32.const -102))))
    (local.get $len))
  (func (export "fetch_header") (param $url_ptr i32) (param $url_len i32) (param $hdr_ptr i32) (param $hdr_len i32) (param $out i32) (result i32)
    (local $rid i32) (local $s i32) (local $len i32)
    (local.set $rid (call $open (local.get $url_ptr) (local.get $url_len)))
    (if (i32.lt_s (local.get $rid) (i32.const 0))
      (then (return (local.get $rid))))
    (local.set $s (call $get_header (local.get $rid) (local.get $hdr_ptr) (local.get $hdr_len)))
    (if (i32.lt_s (local.get $s) (i32.const 0))
      (then (return (local.get $s))))
    (local.set $len (call $buffer_len (local.get $s)))
    (if (i32.lt_s (local.get $len) (i32.const 0))
      (then (return (i32.const -103))))
    (if (i32.ne (call $read_buffer (local.get $s) (local.get $out) (local.get $len)) (i32.const 0))
      (then (return (i32.const -104))))
    (local.get $len)))
"#;

const BODY: &str = "spike-net-ok";

fn mock_server() -> String {
	let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
	let port = listener.local_addr().expect("local addr").port();
	std::thread::spawn(move || {
		for stream in listener.incoming() {
			let Ok(mut stream) = stream else { break };
			let mut head = Vec::new();
			let mut chunk = [0u8; 4096];
			loop {
				match stream.read(&mut chunk) {
					Ok(0) | Err(_) => break,
					Ok(n) => {
						head.extend_from_slice(&chunk[..n]);
						if head.windows(4).any(|w| w == b"\r\n\r\n") {
							break;
						}
					}
				}
			}
			let response = format!(
				"HTTP/1.1 200 OK\r\ncontent-length: {}\r\nx-spike: wasmi-net\r\nconnection: close\r\n\r\n{}",
				BODY.len(),
				BODY
			);
			if stream.write_all(response.as_bytes()).is_err() {
				break;
			}
		}
	});
	format!("http://127.0.0.1:{port}/manga/1")
}

const URL_PTR: u32 = 1024;
const HDR_PTR: u32 = 2048;
const OUT_PTR: u32 = 4096;

fn net_flow(host: &mut impl Host, url: &str) {
	host.mem_write(URL_PTR, url.as_bytes()).expect("write url");
	let url_len = url.len() as i32;

	let status = host
		.call_raw("fetch_status", &[URL_PTR as i32, url_len])
		.expect("fetch_status");
	assert_eq!(status, 200);

	let len = host
		.call_raw("fetch_body", &[URL_PTR as i32, url_len, OUT_PTR as i32])
		.expect("fetch_body");
	assert_eq!(len, BODY.len() as i32);
	let body = host.mem_read(OUT_PTR, len as u32).expect("read body");
	assert_eq!(body, BODY.as_bytes());

	host.mem_write(HDR_PTR, b"x-spike").expect("write header name");
	let len = host
		.call_raw(
			"fetch_header",
			&[URL_PTR as i32, url_len, HDR_PTR as i32, 7, OUT_PTR as i32],
		)
		.expect("fetch_header");
	let value = host.mem_read(OUT_PTR, len as u32).expect("read header");
	assert_eq!(value, b"wasmi-net");
}

#[test]
fn net_family_round_trip_under_wasmi() {
	let bytes = wat::parse_str(NET_WAT).expect("valid wat");
	let mut host = WasmiHost::build(&bytes, Limits::default()).expect("build wasmi host");
	net_flow(&mut host, &mock_server());
}

#[test]
fn net_family_round_trip_under_wasmer_baseline() {
	let bytes = wat::parse_str(NET_WAT).expect("valid wat");
	let mut host = WasmerHost::build(&bytes, Limits::default()).expect("build wasmer host");
	net_flow(&mut host, &mock_server());
}
