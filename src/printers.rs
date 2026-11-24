//! Printer profiles: resolution, print-head width and (optionally) the printer's own
//! command language, so labels are drawn at the printer's real dot grid.
//!
//! Only the PeriPage PB41 has been tested on hardware. The other profiles come from
//! manufacturer specs; their direct (raw) modes are experimental.

use image::GrayImage;

/// A printer command language LabelCraft can send a finished bitmap in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RawLang {
    /// Zebra Programming Language (Zebra, and many printers' ZPL emulation).
    Zpl,
    /// TSC's TSPL/TSPL2 (most budget 4×6 printers: Munbyn, iDPRT, Polono…).
    Tspl,
}

impl RawLang {
    pub fn name(self) -> &'static str {
        match self {
            RawLang::Zpl => "ZPL",
            RawLang::Tspl => "TSPL",
        }
    }
    pub fn ext(self) -> &'static str {
        match self {
            RawLang::Zpl => "zpl",
            RawLang::Tspl => "tspl",
        }
    }
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "zpl" => Some(RawLang::Zpl),
            "tspl" => Some(RawLang::Tspl),
            _ => None,
        }
    }
}

pub struct Profile {
    pub id: &'static str,
    pub name: &'static str,
    /// Lower-case fragments of the OS printer name that identify this family.
    pub keywords: &'static [&'static str],
    pub dpi: f32,
    /// The family also comes in 300 dpi; a "300" in the printer's name selects it.
    pub has_300: bool,
    /// Widest printable line, mm.
    pub head_mm: f32,
    /// Language for direct printing, if the family is known to speak one.
    pub raw: Option<RawLang>,
    pub tested: bool,
}

pub const PROFILES: &[Profile] = &[
    Profile { id: "peripage", name: "PeriPage PB41", keywords: &["peripage", "pb41", "ppg"], dpi: 203.0, has_300: false, head_mm: 104.0, raw: None, tested: true },
    Profile { id: "zebra", name: "Zebra (GK420d, ZD411/421, ZD620, ZSB…)", keywords: &["zebra", "zdesigner", "gk420", "gx420", "zd4", "zd6", "zd2", "zsb", "2844"], dpi: 203.0, has_300: true, head_mm: 104.0, raw: Some(RawLang::Zpl), tested: false },
    Profile { id: "rollo", name: "Rollo (USB / Wireless)", keywords: &["rollo"], dpi: 203.0, has_300: false, head_mm: 104.0, raw: None, tested: false },
    Profile { id: "munbyn", name: "MUNBYN (ITPP941, RW401/403…)", keywords: &["munbyn", "itpp941", "itpp 941", "rw40"], dpi: 203.0, has_300: true, head_mm: 108.0, raw: Some(RawLang::Tspl), tested: false },
    Profile { id: "idprt", name: "iDPRT / Polono (SP410, SP420, PL420…)", keywords: &["idprt", "sp410", "sp420", "sp320", "id4", "polono", "pl420", "pl60"], dpi: 203.0, has_300: true, head_mm: 108.0, raw: Some(RawLang::Tspl), tested: false },
    Profile { id: "jadens", name: "Jadens", keywords: &["jadens"], dpi: 203.0, has_300: false, head_mm: 104.0, raw: None, tested: false },
    Profile { id: "nelko", name: "Nelko (PL70e…)", keywords: &["nelko", "pl70"], dpi: 203.0, has_300: false, head_mm: 104.0, raw: None, tested: false },
    Profile { id: "phomemo", name: "Phomemo (PM-241…)", keywords: &["phomemo", "pm-241", "pm241"], dpi: 203.0, has_300: false, head_mm: 104.0, raw: None, tested: false },
    Profile { id: "dymo", name: "DYMO LabelWriter 4XL / 5XL", keywords: &["dymo", "labelwriter"], dpi: 300.0, has_300: false, head_mm: 105.0, raw: None, tested: false },
    Profile { id: "brother", name: "Brother QL-1100 / QL-1110", keywords: &["ql-1100", "ql-1110", "ql1100", "ql1110"], dpi: 300.0, has_300: false, head_mm: 98.6, raw: None, tested: false },
];

pub fn by_id(id: &str) -> Option<&'static Profile> {
    PROFILES.iter().find(|p| p.id == id)
}

/// Guess the profile from the OS printer name.
pub fn detect(printer: &str) -> Option<&'static Profile> {
    let n = printer.to_lowercase();
    PROFILES.iter().find(|p| p.keywords.iter().any(|k| n.contains(k)))
}

/// What LabelCraft will use for this printer: a profile (or None = custom settings)
/// and its effective resolution.
pub struct Resolved {
    pub profile: Option<&'static Profile>,
    pub dpi: f32,
}

/// `choice` is a profile id, "auto" or "custom"; `custom_dpi` applies to custom.
pub fn resolve(choice: &str, printer: &str, custom_dpi: f32) -> Resolved {
    let profile = match choice {
        "custom" => None,
        "auto" => detect(printer),
        id => by_id(id),
    };
    let dpi = match profile {
        Some(p) if p.has_300 && printer.contains("300") => 300.0,
        Some(p) => p.dpi,
        None => custom_dpi,
    };
    Resolved { profile, dpi }
}

/// Bits of a 1-bit label (dark pixel = printed), packed MSB-first, rows padded to bytes.
fn packed_rows(bitmap: &GrayImage, printed_is_one: bool) -> (u32, Vec<u8>) {
    let (w, h) = bitmap.dimensions();
    let row = w.div_ceil(8);
    let mut out = vec![if printed_is_one { 0u8 } else { 0xFF }; (row * h) as usize];
    for (x, y, p) in bitmap.enumerate_pixels() {
        if p[0] < 128 {
            let i = (y * row + x / 8) as usize;
            let bit = 0x80 >> (x % 8);
            if printed_is_one {
                out[i] |= bit;
            } else {
                out[i] &= !bit;
            }
        }
    }
    (row, out)
}

/// Map LabelCraft's darkness (−20…+20) onto a 0…max scale centred on `mid`.
fn darkness_scale(darkness: i32, mid: i32, max: i32) -> i32 {
    (mid + (darkness as f32 / 20.0 * (max - mid) as f32).round() as i32).clamp(0, max)
}

/// A complete ZPL job that prints `bitmap` once per copy.
pub fn to_zpl(bitmap: &GrayImage, darkness: i32, copies: u32) -> Vec<u8> {
    let (w, h) = bitmap.dimensions();
    let (row, data) = packed_rows(bitmap, true);
    let mut hex = String::with_capacity(data.len() * 2);
    for b in &data {
        hex.push_str(&format!("{b:02X}"));
    }
    // ^MD is relative darkness (−30…30); ^PON = normal orientation; ^LH0,0 = label home.
    let md = (darkness as f32 * 1.5).round() as i32;
    format!(
        "^XA\n^PW{w}\n^LL{h}\n^LH0,0\n^PON\n^MD{md}\n^FO0,0^GFA,{n},{n},{row},{hex}^FS\n^PQ{copies}\n^XZ\n",
        n = data.len(),
        copies = copies.max(1)
    )
    .into_bytes()
}

/// A complete TSPL job that prints `bitmap` once per copy.
pub fn to_tspl(bitmap: &GrayImage, width_mm: f32, height_mm: f32, darkness: i32, copies: u32) -> Vec<u8> {
    let (_, h) = bitmap.dimensions();
    // TSPL bitmaps: a 0 bit prints, a 1 bit stays white.
    let (row, data) = packed_rows(bitmap, false);
    let density = darkness_scale(darkness, 8, 15);
    let mut out = format!(
        "SIZE {width_mm:.1} mm,{height_mm:.1} mm\r\nGAP 2 mm,0 mm\r\nDIRECTION 1,0\r\nREFERENCE 0,0\r\nOFFSET 0 mm\r\nDENSITY {density}\r\nCLS\r\nBITMAP 0,0,{row},{h},0,"
    )
    .into_bytes();
    out.extend_from_slice(&data);
    out.extend_from_slice(format!("\r\nPRINT 1,{}\r\n", copies.max(1)).as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    fn sample() -> GrayImage {
        // 21×5 with a diagonal and a solid block, so padding and bit order both matter.
        let mut g = GrayImage::from_pixel(21, 5, Luma([255]));
        for i in 0..5 {
            g.put_pixel(i * 4, i, Luma([0]));
        }
        for x in 15..21 {
            g.put_pixel(x, 2, Luma([0]));
        }
        g
    }

    fn unpack(row: u32, data: &[u8], w: u32, h: u32, printed_is_one: bool) -> GrayImage {
        GrayImage::from_fn(w, h, |x, y| {
            let bit = data[(y * row + x / 8) as usize] & (0x80 >> (x % 8)) != 0;
            Luma([if bit == printed_is_one { 0 } else { 255 }])
        })
    }

    #[test]
    fn zpl_round_trips() {
        let g = sample();
        let job = String::from_utf8(to_zpl(&g, 0, 2)).unwrap();
        assert!(job.starts_with("^XA") && job.trim_end().ends_with("^XZ"));
        assert!(job.contains("^PW21") && job.contains("^LL5") && job.contains("^PQ2"));
        let gf = job.split("^GFA,").nth(1).unwrap();
        let parts: Vec<&str> = gf.splitn(4, ',').collect();
        let (total, row): (usize, u32) = (parts[0].parse().unwrap(), parts[2].parse().unwrap());
        let hex = parts[3].split("^FS").next().unwrap();
        assert_eq!(hex.len(), total * 2);
        let bytes: Vec<u8> = (0..total).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect();
        assert_eq!(unpack(row, &bytes, 21, 5, true), g);
    }

    #[test]
    fn tspl_round_trips() {
        let g = sample();
        let job = to_tspl(&g, 100.0, 150.0, 0, 3);
        let head = b"BITMAP 0,0,";
        let at = job.windows(head.len()).position(|w| w == head).unwrap() + head.len();
        let rest = &job[at..];
        let comma_positions: Vec<usize> = rest.iter().enumerate().filter(|(_, c)| **c == b',').map(|(i, _)| i).take(3).collect();
        let row: u32 = std::str::from_utf8(&rest[..comma_positions[0]]).unwrap().parse().unwrap();
        let h: u32 = std::str::from_utf8(&rest[comma_positions[0] + 1..comma_positions[1]]).unwrap().parse().unwrap();
        let data = &rest[comma_positions[2] + 1..comma_positions[2] + 1 + (row * h) as usize];
        assert_eq!(unpack(row, data, 21, h, false), g);
        assert!(String::from_utf8_lossy(&job).contains("SIZE 100.0 mm,150.0 mm"));
        assert!(String::from_utf8_lossy(&job).trim_end().ends_with("PRINT 1,3"));
    }

    #[test]
    fn detects_common_driver_names() {
        assert_eq!(detect("ZDesigner GK420d").map(|p| p.id), Some("zebra"));
        assert_eq!(detect("PeriPage PB41").map(|p| p.id), Some("peripage"));
        assert_eq!(detect("Rollo Printer").map(|p| p.id), Some("rollo"));
        assert_eq!(detect("MUNBYN ITPP941").map(|p| p.id), Some("munbyn"));
        assert_eq!(detect("DYMO LabelWriter 5XL").map(|p| p.id), Some("dymo"));
        assert_eq!(resolve("auto", "ZDesigner ZD421-300dpi ZPL", 203.0).dpi, 300.0);
        assert_eq!(resolve("auto", "Office LaserJet", 203.0).profile.map(|p| p.id), None);
    }
}
