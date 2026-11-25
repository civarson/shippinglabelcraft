//! Printing: Windows uses GDI directly (exact dots, no driver scaling);
//! macOS/Linux hand an exact-size PDF to CUPS via `lp`.

use anyhow::Result;
use image::GrayImage;

use crate::pipeline::Settings;

pub fn list_printers() -> Vec<String> {
    imp::list_printers()
}

pub fn default_printer() -> Option<String> {
    imp::default_printer()
}

/// Print the 1-bit label on `printer`. Returns the driver's resolution (x, y) when known.
pub fn print(printer: &str, bitmap: &GrayImage, s: &Settings, extra_cups_options: &str, copies: u32) -> Result<Option<(f32, f32)>> {
    let mut dpi = None;
    for _ in 0..copies.clamp(1, 20) {
        dpi = imp::print(printer, bitmap, s, extra_cups_options, None)?;
    }
    Ok(dpi)
}

/// Run one print through the printer's driver but write what the driver would send to the
/// printer into `out` instead. Nothing is printed. Used to learn a printer's own commands.
pub fn capture_driver_output(printer: &str, bitmap: &GrayImage, s: &Settings, extra_cups_options: &str, out: &std::path::Path) -> Result<Option<(f32, f32)>> {
    imp::print(printer, bitmap, s, extra_cups_options, Some(out))
}

/// Send a ready-made printer-language job (ZPL/TSPL) straight to the printer,
/// bypassing the driver's rendering.
pub fn print_raw(printer: &str, job: &[u8]) -> Result<()> {
    imp::print_raw(printer, job)
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    use anyhow::{Context, anyhow};
    use std::process::Command;

    pub fn list_printers() -> Vec<String> {
        Command::new("lpstat")
            .arg("-e")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
            .unwrap_or_default()
    }

    pub fn default_printer() -> Option<String> {
        let o = Command::new("lpstat").arg("-d").output().ok()?;
        let s = String::from_utf8_lossy(&o.stdout);
        s.split(':').nth(1).map(|n| n.trim().to_string()).filter(|n| !n.is_empty())
    }

    pub fn print_raw(printer: &str, job: &[u8]) -> Result<()> {
        let path = std::env::temp_dir().join(format!("shippinglabelcraft-{}.prn", std::process::id()));
        std::fs::write(&path, job).context("writing temporary print job")?;
        let out = Command::new("lp")
            .args(["-d", printer, "-t", "Shipping label", "-o", "raw"])
            .arg(&path)
            .output()
            .context("running lp (is CUPS available?)")?;
        let _ = std::fs::remove_file(&path);
        if !out.status.success() {
            return Err(anyhow!("lp failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(())
    }

    pub fn print(printer: &str, bitmap: &GrayImage, s: &Settings, extra: &str, capture: Option<&std::path::Path>) -> Result<Option<(f32, f32)>> {
        let pdf = crate::pdfout::label_pdf(bitmap, s.width_mm, s.height_mm);
        let path = std::env::temp_dir().join(format!("shippinglabelcraft-{}.pdf", std::process::id()));
        std::fs::write(&path, pdf).context("writing temporary PDF")?;
        let media = format!("media=Custom.{}x{}mm", s.width_mm.round(), s.height_mm.round());
        if let Some(out_path) = capture {
            // Run the queue's own filter chain (its PPD) and keep the printer-ready bytes.
            let mut cmd = Command::new("cupsfilter");
            cmd.args(["-P", &format!("/etc/cups/ppd/{printer}.ppd"), "-o", &media, "-o", "print-scaling=none"]);
            for opt in extra.split_whitespace() {
                cmd.args(["-o", opt]);
            }
            let out = cmd.arg(&path).output().context("running cupsfilter")?;
            let _ = std::fs::remove_file(&path);
            if !out.status.success() || out.stdout.is_empty() {
                return Err(anyhow!("cupsfilter failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
            }
            std::fs::write(out_path, &out.stdout).context("writing capture")?;
            return Ok(None);
        }
        let mut cmd = Command::new("lp");
        cmd.args(["-d", printer, "-t", "Shipping label", "-o", &media, "-o", "print-scaling=none"])
            .args(["-o", "orientation-requested=3"]);
        for opt in extra.split_whitespace() {
            cmd.args(["-o", opt]);
        }
        cmd.arg(&path);
        let out = cmd.output().context("running lp (is CUPS available?)")?;
        let _ = std::fs::remove_file(&path);
        if !out.status.success() {
            return Err(anyhow!("lp failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(None)
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use anyhow::bail;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::Graphics::Printing::*;
    use windows_sys::Win32::Storage::Xps::{DOCINFOW, EndDoc, EndPage, StartDocW, StartPage};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn from_wide(p: *const u16) -> String {
        if p.is_null() {
            return String::new();
        }
        let mut n = 0;
        unsafe {
            while *p.add(n) != 0 {
                n += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
        }
    }

    pub fn list_printers() -> Vec<String> {
        unsafe {
            let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
            let (mut needed, mut count) = (0u32, 0u32);
            EnumPrintersW(flags, null(), 4, null_mut(), 0, &mut needed, &mut count);
            if needed == 0 {
                return Vec::new();
            }
            // u64 backing keeps the PRINTER_INFO_4W array suitably aligned.
            let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
            if EnumPrintersW(flags, null(), 4, buf.as_mut_ptr() as *mut u8, needed, &mut needed, &mut count) == 0 {
                return Vec::new();
            }
            let infos = std::slice::from_raw_parts(buf.as_ptr() as *const PRINTER_INFO_4W, count as usize);
            infos.iter().map(|i| from_wide(i.pPrinterName)).collect()
        }
    }

    pub fn default_printer() -> Option<String> {
        unsafe {
            let mut n = 0u32;
            GetDefaultPrinterW(null_mut(), &mut n);
            if n == 0 {
                return None;
            }
            let mut buf = vec![0u16; n as usize];
            if GetDefaultPrinterW(buf.as_mut_ptr(), &mut n) == 0 {
                return None;
            }
            Some(from_wide(buf.as_ptr()))
        }
    }

    pub fn print_raw(printer: &str, job: &[u8]) -> Result<()> {
        unsafe {
            let name = wide(printer);
            let mut h = PRINTER_HANDLE::default();
            if OpenPrinterW(name.as_ptr(), &mut h, null()) == 0 {
                bail!("couldn't open printer {printer}");
            }
            let mut doc_name = wide("Shipping label");
            let mut datatype = wide("RAW");
            let info = DOC_INFO_1W { pDocName: doc_name.as_mut_ptr(), pOutputFile: null_mut(), pDatatype: datatype.as_mut_ptr() };
            if StartDocPrinterW(h, 1, &info) == 0 {
                ClosePrinter(h);
                bail!("the printer refused a raw job");
            }
            StartPagePrinter(h);
            let mut written = 0u32;
            let ok = WritePrinter(h, job.as_ptr() as *const _, job.len() as u32, &mut written);
            EndPagePrinter(h);
            EndDocPrinter(h);
            ClosePrinter(h);
            if ok == 0 || written as usize != job.len() {
                bail!("only {written} of {} bytes reached the printer", job.len());
            }
            Ok(())
        }
    }

    /// The printer's DEVMODE, modified to ask for our label size in portrait.
    /// Returned as u64s so the DEVMODEW inside is properly aligned.
    unsafe fn label_devmode(printer: &[u16], s: &Settings) -> Option<Vec<u64>> {
        unsafe {
            let mut h = PRINTER_HANDLE::default();
            if OpenPrinterW(printer.as_ptr(), &mut h, null()) == 0 {
                return None;
            }
            let size = DocumentPropertiesW(null_mut(), h, printer.as_ptr(), null_mut(), null(), 0);
            if size <= 0 {
                ClosePrinter(h);
                return None;
            }
            let words = (size as usize).div_ceil(8);
            let mut cur = vec![0u64; words];
            let dm = cur.as_mut_ptr() as *mut DEVMODEW;
            if DocumentPropertiesW(null_mut(), h, printer.as_ptr(), dm, null(), DM_OUT_BUFFER) < 0 {
                ClosePrinter(h);
                return None;
            }
            let d = &mut *dm;
            d.dmFields |= DM_PAPERSIZE | DM_PAPERLENGTH | DM_PAPERWIDTH | DM_ORIENTATION;
            d.Anonymous1.Anonymous1.dmOrientation = DMORIENT_PORTRAIT as i16;
            d.Anonymous1.Anonymous1.dmPaperSize = 0; // custom size from width/length (0.1 mm units)
            d.Anonymous1.Anonymous1.dmPaperWidth = (s.width_mm * 10.0).round() as i16;
            d.Anonymous1.Anonymous1.dmPaperLength = (s.height_mm * 10.0).round() as i16;
            // Let the driver merge and validate the request.
            let mut merged = vec![0u64; words];
            let ok = DocumentPropertiesW(
                null_mut(),
                h,
                printer.as_ptr(),
                merged.as_mut_ptr() as *mut DEVMODEW,
                dm,
                DM_IN_BUFFER | DM_OUT_BUFFER,
            );
            ClosePrinter(h);
            Some(if ok >= 0 { merged } else { cur })
        }
    }

    pub fn print(printer: &str, bitmap: &GrayImage, s: &Settings, _extra: &str, capture: Option<&std::path::Path>) -> Result<Option<(f32, f32)>> {
        let capture_w = capture.map(|p| wide(&p.to_string_lossy()));
        let name = wide(printer);
        unsafe {
            let devmode = label_devmode(&name, s);
            let dm_ptr = devmode.as_ref().map_or(null(), |b| b.as_ptr() as *const DEVMODEW);
            let hdc = CreateDCW(null(), name.as_ptr(), null(), dm_ptr);
            if hdc.is_null() {
                bail!("couldn't open printer \"{printer}\"");
            }
            let result = (|| -> Result<(f32, f32)> {
                let title = wide("Shipping label");
                let mut di: DOCINFOW = std::mem::zeroed();
                di.cbSize = std::mem::size_of::<DOCINFOW>() as i32;
                di.lpszDocName = title.as_ptr();
                // With an output file the driver's bytes go there instead of to the printer.
                if let Some(c) = &capture_w {
                    di.lpszOutput = c.as_ptr();
                }
                if StartDocW(hdc, &di) <= 0 {
                    bail!("StartDoc failed");
                }
                if StartPage(hdc) <= 0 {
                    EndDoc(hdc);
                    bail!("StartPage failed");
                }
                let caps = |i: GET_DEVICE_CAPS_INDEX| GetDeviceCaps(hdc, i as i32);
                let (dpi_x, dpi_y) = (caps(LOGPIXELSX) as f32, caps(LOGPIXELSY) as f32);
                let (off_x, off_y) = (caps(PHYSICALOFFSETX), caps(PHYSICALOFFSETY));
                // Label size in device pixels, anchored to the paper's top-left corner
                // (GDI's origin is the printable area, hence the physical offset).
                let dw = (s.width_mm / 25.4 * dpi_x).round() as i32;
                let dh = (s.height_mm / 25.4 * dpi_y).round() as i32;

                // 1-bit DIB, bottom-up rows padded to 4 bytes. Palette: 0 = black, 1 = white.
                let (w, h) = bitmap.dimensions();
                let stride = (w as usize).div_ceil(32) * 4;
                let mut bits = vec![0u8; stride * h as usize];
                for (x, y, p) in bitmap.enumerate_pixels() {
                    if p[0] >= 128 {
                        let row = (h - 1 - y) as usize;
                        bits[row * stride + (x / 8) as usize] |= 0x80 >> (x % 8);
                    }
                }
                #[repr(C)]
                struct Bmi2 {
                    header: BITMAPINFOHEADER,
                    colors: [RGBQUAD; 2],
                }
                let mut bmi: Bmi2 = std::mem::zeroed();
                bmi.header.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                bmi.header.biWidth = w as i32;
                bmi.header.biHeight = h as i32;
                bmi.header.biPlanes = 1;
                bmi.header.biBitCount = 1;
                bmi.header.biCompression = BI_RGB;
                bmi.header.biXPelsPerMeter = (s.dpi / 0.0254) as i32;
                bmi.header.biYPelsPerMeter = (s.dpi / 0.0254) as i32;
                bmi.colors[1] = RGBQUAD { rgbBlue: 255, rgbGreen: 255, rgbRed: 255, rgbReserved: 0 };

                SetStretchBltMode(hdc, COLORONCOLOR);
                let lines = StretchDIBits(
                    hdc,
                    -off_x,
                    -off_y,
                    dw,
                    dh,
                    0,
                    0,
                    w as i32,
                    h as i32,
                    bits.as_ptr() as *const _,
                    &bmi as *const Bmi2 as *const BITMAPINFO,
                    DIB_RGB_COLORS,
                    SRCCOPY,
                );
                if lines == 0 {
                    EndPage(hdc);
                    EndDoc(hdc);
                    bail!("the printer driver rejected the image");
                }
                if EndPage(hdc) <= 0 {
                    EndDoc(hdc);
                    bail!("EndPage failed");
                }
                if EndDoc(hdc) <= 0 {
                    bail!("EndDoc failed");
                }
                Ok((dpi_x, dpi_y))
            })();
            DeleteDC(hdc);
            result.map(Some)
        }
    }
}
