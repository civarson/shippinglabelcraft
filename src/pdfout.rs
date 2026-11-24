//! Minimal single-page PDF writer for a 1-bit label bitmap at exact physical size.

use flate2::{Compression, write::ZlibEncoder};
use image::GrayImage;
use std::io::Write;

/// Build a PDF whose page is exactly `width_mm × height_mm` with the bitmap filling it.
pub fn label_pdf(bitmap: &GrayImage, width_mm: f32, height_mm: f32) -> Vec<u8> {
    let (w, h) = bitmap.dimensions();
    // Pack 1 bit per pixel, rows padded to bytes. 1 = white with /Decode default (DeviceGray).
    let row_bytes = w.div_ceil(8) as usize;
    let mut packed = vec![0u8; row_bytes * h as usize];
    for (x, y, p) in bitmap.enumerate_pixels() {
        if p[0] >= 128 {
            packed[y as usize * row_bytes + (x / 8) as usize] |= 0x80 >> (x % 8);
        }
    }
    let mut z = ZlibEncoder::new(Vec::new(), Compression::best());
    z.write_all(&packed).unwrap();
    let img = z.finish().unwrap();

    let pw = width_mm / 25.4 * 72.0;
    let ph = height_mm / 25.4 * 72.0;
    let content = format!("q {pw:.3} 0 0 {ph:.3} 0 0 cm /Im0 Do Q\n");

    let mut out: Vec<u8> = Vec::new();
    let mut offsets = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n");
    let mut obj = |out: &mut Vec<u8>, body: &[u8]| {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    obj(&mut out, b"<< /Type /Catalog /Pages 2 0 R >>");
    obj(&mut out, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    obj(
        &mut out,
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.3} {ph:.3}] \
             /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>"
        )
        .as_bytes(),
    );
    obj(
        &mut out,
        &[format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(), content.as_bytes(), b"\nendstream"].concat(),
    );
    obj(
        &mut out,
        &[
            format!(
                "<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray \
                 /BitsPerComponent 1 /Interpolate false /Filter /FlateDecode /Length {} >>\nstream\n",
                img.len()
            )
            .as_bytes(),
            &img,
            b"\nendstream",
        ]
        .concat(),
    );
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for o in &offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1).as_bytes(),
    );
    out
}

/// PNG with the physical DPI recorded (pHYs), so viewers/print dialogs size it right.
pub fn label_png(bitmap: &GrayImage, dpi: f32) -> anyhow::Result<Vec<u8>> {
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, bitmap.width(), bitmap.height());
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        let ppm = (dpi / 0.0254).round() as u32;
        enc.set_pixel_dims(Some(png::PixelDimensions { xppu: ppm, yppu: ppm, unit: png::Unit::Meter }));
        let mut w = enc.write_header()?;
        w.write_image_data(bitmap.as_raw())?;
    }
    Ok(buf)
}
