#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod barcodes;
mod ocr;
mod gui;
mod theme;
mod widgets;
mod pdfout;
mod pipeline;
mod print;

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
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("LabelCraft", options, Box::new(move |cc| Ok(Box::new(gui::App::new(cc, initial)))))
        .map_err(|e| anyhow::anyhow!("{e}"))
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
    let src = pipeline::Source::open(&PathBuf::from(input))?;
    let page = src.render_page(0)?;
    let o = pipeline::process(&page, &s)?;
    for n in &o.notes {
        println!("note: {n}");
    }
    for b in &o.barcodes {
        println!("{}", b.summary());
    }
    for l in &o.ocr {
        println!("text {:?} conf {:.2} → {:?}", l.text, l.confidence, l.status);
    }
    let out = PathBuf::from(out);
    std::fs::write(&out, pdfout::label_png(&o.bitmap, s.dpi)?)?;
    std::fs::write(out.with_extension("pdf"), pdfout::label_pdf(&o.bitmap, s.width_mm, s.height_mm))?;
    if let Some(i) = args.iter().position(|a| a == "--print") {
        let printer = args.get(i + 1).cloned().or_else(print::default_printer).unwrap_or_default();
        print::print(&printer, &o.bitmap, &s, "", 1)?;
        println!("sent to {printer}");
    }
    Ok(())
}
