//! Phase 0 proof: write `xmp:Rating` / `xmp:Label` into a sidecar, merging
//! into whatever Lightroom or darktable already left there.
//!
//!     zaru-mark IMG_4821.CR3 --rating 4 --label Green
//!     zaru-mark IMG_4821.CR3 --reject
//!     zaru-mark IMG_4821.CR3 --read

use std::path::PathBuf;
use std::process::ExitCode;

use zaru_xmp::{Marks, SidecarStyle, REJECTED};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut input: Option<PathBuf> = None;
    let mut rating: Option<i8> = None;
    let mut label: Option<Option<String>> = None;
    let mut style = SidecarStyle::ReplaceExtension;
    let mut read_only = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--rating" => match args.next().and_then(|v| v.parse::<i8>().ok()) {
                Some(v) if (REJECTED..=5).contains(&v) => rating = Some(v),
                _ => return fail("--rating takes a number from -1 to 5"),
            },
            "--reject" => rating = Some(REJECTED),
            "--label" => match args.next() {
                Some(v) if v.is_empty() => label = Some(None),
                Some(v) => label = Some(Some(v)),
                None => return fail("--label needs a colour name, or \"\" to clear"),
            },
            "--darktable" => style = SidecarStyle::AppendExtension,
            "--read" => read_only = true,
            "-h" | "--help" => {
                println!("usage: zaru-mark <file.CR3> [--rating N|--reject] [--label NAME] [--darktable] [--read]");
                return ExitCode::SUCCESS;
            }
            _ if input.is_none() => input = Some(PathBuf::from(arg)),
            other => return fail(&format!("unexpected argument {other:?}")),
        }
    }

    let Some(input) = input else {
        return fail("usage: zaru-mark <file.CR3> [--rating N|--reject] [--label NAME]");
    };
    let sidecar = zaru_xmp::sidecar_path(&input, style);

    if read_only {
        return match std::fs::read_to_string(&sidecar) {
            Ok(xml) => {
                let m = zaru_xmp::read(&xml);
                println!("{}", sidecar.display());
                println!("  rating  {}{}", m.rating, if m.is_rejected() { " (rejected)" } else { "" });
                println!("  label   {}", m.label.as_deref().unwrap_or("-"));
                ExitCode::SUCCESS
            }
            Err(e) => fail(&format!("{}: {e}", sidecar.display())),
        };
    }

    // Start from what is already on disk so an unspecified flag means "leave
    // it alone" rather than "clear it".
    let mut marks = std::fs::read_to_string(&sidecar)
        .map(|xml| zaru_xmp::read(&xml))
        .unwrap_or_else(|_| Marks::default());
    if let Some(r) = rating {
        marks.rating = r;
    }
    if let Some(l) = label {
        marks.label = l;
    }

    match zaru_xmp::apply(&input, &marks, style) {
        Ok(path) => {
            println!("{} rating={} label={}", path.display(), marks.rating,
                     marks.label.as_deref().unwrap_or("-"));
            ExitCode::SUCCESS
        }
        Err(e) => fail(&format!("{}: {e}", sidecar.display())),
    }
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("zaru-mark: {msg}");
    ExitCode::FAILURE
}
