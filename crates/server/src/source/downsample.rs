//! 页图降采样变体：reader 按显示宽度取缩小版 PNG，原图字节在无参、不放大或
//! 解码失败时逐字节直通，下载/OPDS 不经过这里
use image::ImageReader;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// 请求宽度向上量化到 256 的整数倍：变体数量有界，且窗口微调/不同 DPR 共享同一份缓存
pub const WIDTH_BUCKET: u32 = 256;

/// 取桶饱和到 u32::MAX：超大请求宽度恒不缩（不放大），等价直通
pub fn bucket_width(width: u32) -> u32 {
	width.checked_next_multiple_of(WIDTH_BUCKET).unwrap_or(u32::MAX)
}

pub enum VariantHit {
	Png(Vec<u8>),
	PassThrough,
}

fn variant_dir(blobs_dir: &Path, chapter_id: i64, bucket: u32) -> PathBuf {
	blobs_dir
		.join(chapter_id.to_string())
		.join("variants")
		.join(bucket.to_string())
}

/// 变体生命周期挂靠页图 blob 目录：load_pages 整章重写与启动清扫把变体一并清掉，
/// 所以这里不做独立失效逻辑
pub async fn read_variant(blobs_dir: &Path, chapter_id: i64, index: i64, bucket: u32) -> Option<VariantHit> {
	let dir = variant_dir(blobs_dir, chapter_id, bucket);
	if let Ok(bytes) = tokio::fs::read(dir.join(format!("{index}.png"))).await {
		return Some(VariantHit::Png(bytes));
	}
	// .pass 记录「该桶不缩」（窄图或解码失败），挡住每次访问重复解码大图
	if tokio::fs::metadata(dir.join(format!("{index}.pass"))).await.is_ok() {
		return Some(VariantHit::PassThrough);
	}
	None
}

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// tmp+rename 落盘；并发的同页同桶写入各用各的 tmp 名，最后一个 rename 胜出
pub async fn write_variant(
	blobs_dir: &Path,
	chapter_id: i64,
	index: i64,
	bucket: u32,
	png: Option<&[u8]>,
) -> std::io::Result<()> {
	let dir = variant_dir(blobs_dir, chapter_id, bucket);
	tokio::fs::create_dir_all(&dir).await?;
	let (name, data): (String, &[u8]) = match png {
		Some(bytes) => (format!("{index}.png"), bytes),
		None => (format!("{index}.pass"), &[]),
	};
	let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
	let tmp = dir.join(format!(".{name}.{seq}.tmp"));
	tokio::fs::write(&tmp, data).await?;
	tokio::fs::rename(&tmp, dir.join(&name)).await
}

/// 缩到目标宽的 PNG 字节；原图不宽于目标（不放大）或解码/编码失败返回 None
pub fn downsample_png(bytes: &[u8], target_width: u32) -> Option<Vec<u8>> {
	let image = ImageReader::new(Cursor::new(bytes))
		.with_guessed_format()
		.ok()?
		.decode()
		.ok()?;
	let (w, h) = (image.width(), image.height());
	if w <= target_width {
		return None;
	}
	let target_height = (h as u64 * target_width as u64 / w as u64).max(1) as u32;
	// Lanczos3：漫画线稿与网点缩小后糊得最少，代价只在变体首请求付一次
	let resized = image.resize_exact(target_width, target_height, image::imageops::FilterType::Lanczos3);
	let mut out = Vec::new();
	resized
		.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
		.ok()?;
	Some(out)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn solid_png(w: u32, h: u32) -> Vec<u8> {
		let image = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
		let mut out = Vec::new();
		image::DynamicImage::ImageRgba8(image)
			.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
			.expect("encode");
		out
	}

	#[test]
	fn bucket_rounds_up_to_256() {
		assert_eq!(bucket_width(1), 256);
		assert_eq!(bucket_width(1500), 1536);
		assert_eq!(bucket_width(1536), 1536);
		assert_eq!(bucket_width(u32::MAX), u32::MAX);
	}

	#[test]
	fn downsample_scales_wide_images() {
		let png = solid_png(800, 600);
		let out = downsample_png(&png, 256).expect("resized");
		let image = ImageReader::new(Cursor::new(out))
			.with_guessed_format()
			.expect("format")
			.decode()
			.expect("decode");
		assert_eq!((image.width(), image.height()), (256, 192));
	}

	#[test]
	fn downsample_passes_through_narrow_images() {
		let png = solid_png(200, 300);
		assert!(downsample_png(&png, 200).is_none());
		assert!(downsample_png(&png, 512).is_none());
	}

	#[test]
	fn downsample_rejects_undecodable_bytes() {
		assert!(downsample_png(b"not an image", 256).is_none());
	}
}
