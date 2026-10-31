//! Cover downscaling: local covers and thumbnails resize to a bounded JPEG so the library grid
//! never ships the original bytes; page images are served as-is and scaled by the client
use image::ImageReader;
use std::io::Cursor;

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
	// Lanczos3: blurs manga line art and halftones the least when shrinking, and the cost is paid only once per cover's first request
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
