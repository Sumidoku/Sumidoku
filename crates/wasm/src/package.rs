//! .aix 包（zip）读取：Aidoku 打包约定固定 `Payload/` 前缀
//! （aidoku-cli internal/build），清单 source.json、载荷 main.wasm 与目录源同名

use std::io::{Read, Seek};
use std::path::Path;

pub const MANIFEST_ENTRY: &str = "Payload/source.json";
pub const WASM_ENTRY: &str = "Payload/main.wasm";
pub const SETTINGS_ENTRY: &str = "Payload/settings.json";
/// Aidoku 包内嵌图标：解析后供已装源列表用（Aidoku 从解包目录读 icon.png）
pub const ICON_ENTRY: &str = "Payload/icon.png";

/// 单个 entry 解压上限：真实包 wasm 不足 1 MiB，64 MiB 已留足余量；防 zip bomb
pub const WASM_PAYLOAD_MAX: u64 = 64 * 1024 * 1024;
/// 清单 / settings 这类 json 元数据的上限
pub const JSON_ENTRY_MAX: u64 = 1024 * 1024;
/// 图标上限：真实图标几十 KiB，1 MiB 已留足余量
pub const ICON_ENTRY_MAX: u64 = 1024 * 1024;

/// 错误用裸 String：调用方（wasm 端 build、服务端扫描）各自包成自己的错误类型
pub fn read_zip_entry<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    max: u64,
) -> std::result::Result<Vec<u8>, String> {
    let entry = archive
        .by_name(name)
        .map_err(|_| format!("missing {name}"))?;
    if entry.size() > max {
        return Err(format!("{name} exceeds {max} bytes"));
    }
    let mut out = Vec::new();
    // central directory 的未压缩大小可能撒谎，take 兜底
    entry
        .take(max + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("read {name}: {e}"))?;
    if out.len() as u64 > max {
        return Err(format!("{name} exceeds {max} bytes"));
    }
    Ok(out)
}

/// 可选 entry：不存在返回 `Ok(None)`，存在则按上限读取
pub fn read_optional_zip_entry<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    max: u64,
) -> std::result::Result<Option<Vec<u8>>, String> {
    if archive.by_name(name).is_err() {
        return Ok(None);
    }
    read_zip_entry(archive, name, max).map(Some)
}

/// wasm_path 可能是裸 .wasm 或 .aix（zip）：按 PK 魔数区分，zip 从 Payload/main.wasm 取载荷
pub fn read_wasm_payload(path: &Path) -> crate::Result<Vec<u8>> {
    use crate::Error;
    let mut file = std::fs::File::open(path)
        .map_err(|e| Error::Instance(format!("read wasm {}: {e}", path.display())))?;
    let mut magic = [0u8; 4];
    let is_zip = file.read_exact(&mut magic).is_ok() && magic.starts_with(b"PK");
    file.seek(std::io::SeekFrom::Start(0))
        .map_err(|e| Error::Instance(format!("read wasm {}: {e}", path.display())))?;
    if is_zip {
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| Error::Instance(format!("invalid archive {}: {e}", path.display())))?;
        read_zip_entry(&mut archive, WASM_ENTRY, WASM_PAYLOAD_MAX).map_err(Error::Instance)
    } else {
        let mut out = Vec::new();
        file.read_to_end(&mut out)
            .map_err(|e| Error::Instance(format!("read wasm {}: {e}", path.display())))?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, data) in entries {
                writer.start_file(name, options).unwrap();
                std::io::Write::write_all(&mut writer, data).unwrap();
            }
            writer.finish().unwrap();
        }
        cursor.into_inner()
    }

    #[test]
    fn read_zip_entry_reads_and_caps() {
        let bytes = make_zip(&[("Payload/main.wasm", b"\0asm payload")]);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let out = read_zip_entry(&mut archive, WASM_ENTRY, WASM_PAYLOAD_MAX).unwrap();
        assert_eq!(out, b"\0asm payload");

        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(make_zip(&[]))).unwrap();
        let err = read_zip_entry(&mut archive, WASM_ENTRY, WASM_PAYLOAD_MAX).unwrap_err();
        assert!(err.contains("missing Payload/main.wasm"), "{err}");

        let bytes = make_zip(&[("Payload/main.wasm", b"0123456789")]);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let err = read_zip_entry(&mut archive, WASM_ENTRY, 4).unwrap_err();
        assert!(err.contains("exceeds"), "{err}");
    }

    #[test]
    fn read_optional_zip_entry_absent_is_none() {
        let bytes = make_zip(&[("Payload/main.wasm", b"x")]);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(
            read_optional_zip_entry(&mut archive, ICON_ENTRY, ICON_ENTRY_MAX).unwrap(),
            None
        );

        let bytes = make_zip(&[(ICON_ENTRY, b"icon")]);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(
            read_optional_zip_entry(&mut archive, ICON_ENTRY, ICON_ENTRY_MAX).unwrap(),
            Some(b"icon".to_vec())
        );

        let bytes = make_zip(&[(ICON_ENTRY, b"0123456789")]);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let err = read_optional_zip_entry(&mut archive, ICON_ENTRY, 4).unwrap_err();
        assert!(err.contains("exceeds"), "{err}");
    }

    #[test]
    fn read_wasm_payload_accepts_raw_and_aix() {
        let tmp = std::env::temp_dir().join(format!("aidoku-wasm-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let raw = tmp.join("raw.wasm");
        std::fs::write(&raw, b"\0asm raw module").unwrap();
        assert_eq!(read_wasm_payload(&raw).unwrap(), b"\0asm raw module");

        let aix = tmp.join("packed.aix");
        std::fs::write(&aix, make_zip(&[(WASM_ENTRY, b"\0asm packed")])).unwrap();
        assert_eq!(read_wasm_payload(&aix).unwrap(), b"\0asm packed");

        let broken = tmp.join("broken.aix");
        std::fs::write(&broken, b"PK\x03\x04 not really a zip").unwrap();
        let err = read_wasm_payload(&broken).unwrap_err();
        assert!(err.to_string().contains("invalid archive"), "{err}");

        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
