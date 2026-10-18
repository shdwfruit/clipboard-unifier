//! RGBA <-> PNG for backends whose clipboard API speaks raw pixels (arboard).

use anyhow::{Context, Result};
use std::io::Cursor;

pub fn rgba_to_png(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>> {
    let img = image::RgbaImage::from_raw(width as u32, height as u32, rgba.to_vec())
        .context("clipboard image has an unexpected size")?;
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

pub fn png_to_rgba(png: &[u8]) -> Result<(usize, usize, Vec<u8>)> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png)?.to_rgba8();
    Ok((img.width() as usize, img.height() as usize, img.into_raw()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn png_round_trip() {
        let rgba: Vec<u8> = (0..4 * 3 * 2).map(|i| i as u8).collect();
        let png = super::rgba_to_png(3, 2, &rgba).unwrap();
        assert_eq!(super::png_to_rgba(&png).unwrap(), (3, 2, rgba));
    }
}
