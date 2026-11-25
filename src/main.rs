#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod barcodes;
mod ocr;
mod gui;
mod theme;
mod widgets;
mod pdfout;
mod pipeline;
mod print;
mod printers;
mod extras;

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Headless mode, handy for testing and scripting:
    //   labelcraft --cli input.pdf output.png [--no-regen] [--print "Printer Name"]
    if args.first().map(String::as_str) == Some("--cli") {
        return cli(&args[1..]);
    }

    let initial = args.first().map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("LabelCraft")
            .with_inner_size([1180.0, 820.0])
            .with_min_inner_size([900.0, 620.0])
            .with_drag_and_drop(true)
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native("LabelCraft", options, Box::new(move |cc| Ok(Box::new(gui::App::new(cc, initial)))))
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// The window/taskbar/dock icon.
fn app_icon() -> eframe::egui::IconData {
    let img = image::load_from_memory(include_bytes!("../assets/icon-256.png")).map(|i| i.to_rgba8());
    match img {
        Ok(i) => eframe::egui::IconData { width: i.width(), height: i.height(), rgba: i.into_raw() },
        Err(_) => eframe::egui::IconData::default(),
    }
}

fn cli(args: &[String]) -> anyhow::Result<()> {
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let (Some(input), Some(out)) = (positional.first(), positional.get(1)) else {
        anyhow::bail!("usage: labelcraft --cli <input.pdf|png|jpg> <output.png> [--no-regen] [--no-vector] [--ocr] [--print <printer>]");
    };
    let mut s = pipeline::Settings::default();
    if args.iter().any(|a| a == "--no-regen") {
        s.regenerate_barcodes = false;
    }
    if args.iter().any(|a| a == "--no-vector") {
        s.vector_render = false;
    }
    if args.iter().any(|a| a == "--ocr") {
        s.ocr = true;
    }
    if let Some(i) = args.iter().position(|a| a == "--dpi") {
        s.dpi = args.get(i + 1).and_then(|v| v.parse().ok()).ok_or_else(|| anyhow::anyhow!("--dpi needs a number"))?;
    }
    let src = pipeline::Source::open(&PathBuf::from(input))?;
    let page = src.render_page(0)?;
    if let Some(i) = args.iter().position(|a| a == "--calibration") {
        let p = args.get(i + 1).ok_or_else(|| anyhow::anyhow!("--calibration needs a file"))?;
        let g = extras::calibration_page(s.width_mm, s.height_mm, s.dpi, (s.offset_x_mm, s.offset_y_mm));
        std::fs::write(p, pdfout::label_png(&g, s.dpi)?)?;
        println!("wrote calibration page to {p}");
    }
    if args.iter().any(|a| a == "--layout") {
        for i in 0..src.page_count {
            let pg = src.render_page(i)?;
            let l = pipeline::find_labels(&pg.gray, pg.is_vector);
            println!("page {}: labels {:?} slip {:?} (page {}x{})", i + 1, l.labels, l.slip, pg.gray.width(), pg.gray.height());
        }
    }
    let mut o = pipeline::process(&page, &s)?;
    for n in &o.notes {
        println!("note: {n}");
    }
    for b in &o.barcodes {
        println!("{}", b.summary());
    }
    for (i, l) in o.ocr.iter().enumerate() {
        println!("[{i}] text {:?} conf {:.2} {:?} → {:?}", l.text, l.confidence, l.kind, l.status);
    }
    // --edit N=new text: replace OCR line N by hand, as the GUI does.
    for (i, a) in args.iter().enumerate() {
        if a != "--edit" {
            continue;
        }
        let Some((n, text)) = args.get(i + 1).and_then(|v| v.split_once('=')) else { anyhow::bail!("--edit needs N=text") };
        let line = o.ocr.get(n.parse::<usize>()?).ok_or_else(|| anyhow::anyhow!("no line {n}"))?.clone();
        match ocr::edit_line(&mut o.bitmap, &line, text) {
            Ok(r) => println!("edit [{n}] {text:?}: reads back {:?}, squeeze {:.2}", r.read_back, r.squeeze),
            Err(e) => println!("edit [{n}] {text:?} refused: {e}"),
        }
    }
    if let Some(i) = args.iter().position(|a| a == "--share") {
        let p = args.get(i + 1).ok_or_else(|| anyhow::anyhow!("--share needs a file"))?;
        let text: Vec<_> = if o.ocr.is_empty() { ocr::text_regions(&o.bitmap) } else { o.ocr.iter().map(|l| l.region).collect() };
        let codes: Vec<_> = o.barcodes.iter().map(|b| b.region).collect();
        extras::redacted(&o.bitmap, &text, &codes).save(p)?;
    }
    let out = PathBuf::from(out);
    std::fs::write(&out, pdfout::label_png(&o.bitmap, s.dpi)?)?;
    std::fs::write(out.with_extension("pdf"), pdfout::label_pdf(&o.bitmap, s.width_mm, s.height_mm))?;
    // --zpl out.zpl / --tspl out.tspl: write a printer-language job instead of (or as well as) PNG/PDF.
    if let Some(i) = args.iter().position(|a| a == "--zpl") {
        let p = args.get(i + 1).ok_or_else(|| anyhow::anyhow!("--zpl needs a file"))?;
        std::fs::write(p, printers::to_zpl(&o.bitmap, s.darkness, 1))?;
    }
    if let Some(i) = args.iter().position(|a| a == "--tspl") {
        let p = args.get(i + 1).ok_or_else(|| anyhow::anyhow!("--tspl needs a file"))?;
        std::fs::write(p, printers::to_tspl(&o.bitmap, s.width_mm, s.height_mm, s.darkness, 1))?;
    }
    // --capture <printer> <file>: run the driver but save its output instead of printing.
    if let Some(i) = args.iter().position(|a| a == "--capture") {
        let (Some(printer), Some(file)) = (args.get(i + 1), args.get(i + 2)) else { anyhow::bail!("--capture needs a printer and a file") };
        let dpi = print::capture_driver_output(printer, &o.bitmap, &s, "", std::path::Path::new(file))?;
        println!("captured driver output to {file}{}", dpi.map_or(String::new(), |(x, y)| format!(" (driver at {x}×{y} dpi)")));
    }
    if let Some(i) = args.iter().position(|a| a == "--print") {
        let printer = args.get(i + 1).cloned().or_else(print::default_printer).unwrap_or_default();
        print::print(&printer, &o.bitmap, &s, "", 1)?;
        println!("sent to {printer}");
    }
    Ok(())
}
