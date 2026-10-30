//! Downsampled page-image variants: the reader fetches a shrunk PNG at display width; original bytes pass through
//! verbatim when no parameter is given, when the image would be upscaled, or when decoding fails; downloads never come through here
use image::ImageReader;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Request widths quantize up to a multiple of 256: the variant count stays bounded, and window nudges/different DPRs share one cache entry
pub const WIDTH_BUCKET: u32 = 256;

/// Bucketing saturates at u32::MAX: an oversized request width never resizes (no upscaling), equivalent to pass-through
pub fn bucket_width(width: u32) -> u32 {
	width.checked_next_multiple_of(WIDTH_BUCKET).unwrap_or(u32::MAX)
}

pub enum VariantHit {
	Png(Vec<u8>),
	PassThrough,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct VariantManifest { identity: String, digest: String }

fn bytes_digest(bytes: &[u8]) -> String {
	use sha2::Digest;
	sha2::Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

pub async fn variant_entry_is_intact(blobs_dir: &Path, chapter_id: i64, index: i64, bucket: u32) -> bool {
	let dir = variant_dir(blobs_dir, chapter_id, bucket);
	let Ok(manifest) = tokio::fs::read(dir.join(format!("{index}.identity"))).await else { return false; };
	let Ok(manifest) = serde_json::from_slice::<VariantManifest>(&manifest) else { return false; };
	if let Ok(bytes) = tokio::fs::read(dir.join(format!("{index}.png"))).await {
		return bytes_digest(&bytes) == manifest.digest;
	}
	tokio::fs::metadata(dir.join(format!("{index}.pass"))).await.is_ok() && bytes_digest(&[]) == manifest.digest
}

fn variant_dir(blobs_dir: &Path, chapter_id: i64, bucket: u32) -> PathBuf {
	blobs_dir
		.join(chapter_id.to_string())
		.join("variants")
		.join(bucket.to_string())
}

pub fn variant_paths(blobs_dir: &Path, chapter_id: i64, index: i64, bucket: u32) -> [PathBuf; 3] {
    let dir = variant_dir(blobs_dir, chapter_id, bucket);
    [
        dir.join(format!("{index}.png")),
        dir.join(format!("{index}.pass")),
        dir.join(format!("{index}.identity")),
    ]
}

/// The variant lifecycle hangs on the page-blob directory: load_pages' chapter rewrite and the startup sweep remove
/// variants along with it, so there is no independent invalidation logic here
pub async fn read_variant(blobs_dir: &Path, chapter_id: i64, index: i64, bucket: u32, identity: &str) -> Option<VariantHit> {
	let dir = variant_dir(blobs_dir, chapter_id, bucket);
	let manifest: VariantManifest = serde_json::from_slice(&tokio::fs::read(dir.join(format!("{index}.identity"))).await.ok()?).ok()?;
	if manifest.identity != identity { return None; }
	if let Ok(bytes) = tokio::fs::read(dir.join(format!("{index}.png"))).await {
		return (bytes_digest(&bytes) == manifest.digest).then_some(VariantHit::Png(bytes));
	}
	// A .pass records "this bucket does not resize" (narrow image or decode failure), stopping every visit from re-decoding the big image
	if tokio::fs::metadata(dir.join(format!("{index}.pass"))).await.is_ok() && bytes_digest(&[]) == manifest.digest {
		return Some(VariantHit::PassThrough);
	}
	None
}

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Writes via tmp+rename; concurrent writers of the same page and bucket each use their own tmp name, and the last rename wins
pub async fn write_variant(
	blobs_dir: &Path,
	chapter_id: i64,
	index: i64,
	bucket: u32,
	png: Option<&[u8]>,
	identity: &str,
) -> std::io::Result<()> {
	let dir = variant_dir(blobs_dir, chapter_id, bucket);
	tokio::fs::create_dir_all(&dir).await?;
	let (name, data): (String, &[u8]) = match png {
		Some(bytes) => (format!("{index}.png"), bytes),
		None => (format!("{index}.pass"), &[]),
	};
	let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
	let tmp = dir.join(format!(".{name}.{seq}.tmp"));
	if let Err(error) = tokio::fs::write(&tmp, data).await {
		let _ = tokio::fs::remove_file(&tmp).await;
		return Err(error);
	}
	if let Err(error) = tokio::fs::rename(&tmp, dir.join(&name)).await {
		let _ = tokio::fs::remove_file(&tmp).await;
		return Err(error);
	}
	let other = dir.join(if png.is_some() { format!("{index}.pass") } else { format!("{index}.png") });
	let _ = tokio::fs::remove_file(other).await;
	let identity_tmp = dir.join(format!(".{index}.identity.{seq}.tmp"));
	let manifest = serde_json::to_vec(&VariantManifest { identity: identity.to_string(), digest: bytes_digest(data) })
		.map_err(std::io::Error::other)?;
	if let Err(error) = tokio::fs::write(&identity_tmp, manifest).await {
		let _ = tokio::fs::remove_file(&identity_tmp).await;
		return Err(error);
	}
	if let Err(error) = tokio::fs::rename(&identity_tmp, dir.join(format!("{index}.identity"))).await {
		let _ = tokio::fs::remove_file(&identity_tmp).await;
		return Err(error);
	}
	Ok(())
}

/// PNG bytes resized to the target width; returns None when the source image is no wider than the target (no upscaling) or decode/encode fails
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
	// Lanczos3: blurs manga line art and halftones the least when shrinking, and the cost is paid only once per variant's first request
	let resized = image.resize_exact(target_width, target_height, image::imageops::FilterType::Lanczos3);
	let mut out = Vec::new();
	resized
		.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
		.ok()?;
	Some(out)
}

/// JPEG bytes resized so the longest edge fits the target: covers are photographic, so JPEG's size beats
/// PNG's losslessness here; alpha flattens onto RGB (JPEG has no alpha channel). Longest-edge semantics
/// mirror kmrs's thumbnail resize (a landscape cover is not penalized for its width)
pub fn downsample_jpeg(bytes: &[u8], target_edge: u32) -> Option<Vec<u8>> {
	let image = ImageReader::new(Cursor::new(bytes))
		.with_guessed_format()
		.ok()?
		.decode()
		.ok()?;
	let (w, h) = (image.width(), image.height());
	let longest = w.max(h);
	if longest <= target_edge {
		return None;
	}
	let scale = f64::from(target_edge) / f64::from(longest);
	let target_w = (f64::from(w) * scale).max(1.0) as u32;
	let target_h = (f64::from(h) * scale).max(1.0) as u32;
	// Lanczos3: blurs manga line art and halftones the least when shrinking, and the cost is paid only once per variant's first request
	let resized = image.resize_exact(target_w, target_h, image::imageops::FilterType::Lanczos3);
	let mut out = Vec::new();
	resized
		.to_rgb8()
		.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Jpeg)
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

	#[tokio::test]
	async fn variants_require_matching_identity_and_payload_digest() {
		let tmp = tempfile::tempdir().expect("tempdir");
		write_variant(tmp.path(), 7, 0, 256, Some(b"product-resize"), "product:artifact-a")
			.await.expect("write variant");
		assert!(matches!(read_variant(tmp.path(), 7, 0, 256, "product:artifact-a").await, Some(VariantHit::Png(bytes)) if bytes == b"product-resize"));
		assert!(read_variant(tmp.path(), 7, 0, 256, "source:url-page").await.is_none());
		std::fs::write(variant_dir(tmp.path(), 7, 256).join("0.png"), b"corrupted resize").expect("corrupt variant");
		assert!(read_variant(tmp.path(), 7, 0, 256, "product:artifact-a").await.is_none());
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

	#[test]
	fn downsample_jpeg_fits_the_longest_edge() {
		// landscape: the width is the longest edge and binds the fit
		let out = downsample_jpeg(&solid_png(800, 600), 400).expect("resized");
		let image = ImageReader::new(Cursor::new(out))
			.with_guessed_format()
			.expect("format")
			.decode()
			.expect("decode");
		assert_eq!((image.width(), image.height()), (400, 300));
		// portrait: the height binds instead
		let out = downsample_jpeg(&solid_png(600, 800), 400).expect("resized");
		let image = ImageReader::new(Cursor::new(out))
			.with_guessed_format()
			.expect("format")
			.decode()
			.expect("decode");
		assert_eq!((image.width(), image.height()), (300, 400));
		let out = downsample_jpeg(&solid_png(300, 800), 400).expect("narrow portrait resized");
		let image = ImageReader::new(Cursor::new(out))
			.with_guessed_format()
			.expect("format")
			.decode()
			.expect("decode");
		assert_eq!((image.width(), image.height()), (150, 400));
		// already small enough: pass-through
		assert!(downsample_jpeg(&solid_png(400, 300), 400).is_none());
	}
}
