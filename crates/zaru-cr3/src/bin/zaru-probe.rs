//! Phase 0 proof: locate and extract the embedded JPEG preview of a CR3.
//!
//!     zaru-probe IMG_4821.CR3 [-o out.jpg]

use std::path::PathBuf;
use std::process::ExitCode;

use zaru_cr3::{probe, read_preview, PreviewKind};

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;

    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("-o") | Some("--output") => match args.next() {
                Some(v) => output = Some(PathBuf::from(v)),
                None => return fail("-o needs a path"),
            },
            Some("-h") | Some("--help") => {
                println!("usage: zaru-probe <file.CR3> [-o out.jpg]");
                return ExitCode::SUCCESS;
            }
            _ if input.is_none() => input = Some(PathBuf::from(arg)),
            _ => return fail("unexpected extra argument"),
        }
    }

    let Some(input) = input else {
        return fail("usage: zaru-probe <file.CR3> [-o out.jpg]");
    };

    let info = match probe(&input) {
        Ok(i) => i,
        Err(e) => return fail(&format!("{}: {e}", input.display())),
    };

    let source = match info.preview.kind {
        PreviewKind::Track => "trak sample",
        PreviewKind::Prvw => "PRVW box",
    };
    let (degrees, mirrored) = info.rotation();

    println!("{}", input.display());
    println!(
        "  preview     {}x{}  ({source})",
        info.preview.width, info.preview.height
    );
    println!(
        "  byte range  offset={} len={}",
        info.preview.offset, info.preview.len
    );
    println!(
        "  orientation {} -> rotate {degrees}{}",
        info.orientation,
        if mirrored { ", mirrored" } else { "" }
    );
    println!("  sensor      {}x{}", info.sensor_width, info.sensor_height);
    match info.captured_ms {
        Some(ms) => println!("  disparo     {}.{:03} (ms do epoch)", ms / 1000, ms % 1000),
        None => println!("  disparo     ausente"),
    }

    if let Some(out) = output {
        let bytes = match read_preview(&input, &info) {
            Ok(b) => b,
            Err(e) => return fail(&format!("reading preview: {e}")),
        };
        if let Err(e) = std::fs::write(&out, &bytes) {
            return fail(&format!("{}: {e}", out.display()));
        }
        println!("  wrote       {} ({} bytes)", out.display(), bytes.len());
    }

    ExitCode::SUCCESS
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("zaru-probe: {msg}");
    ExitCode::FAILURE
}
