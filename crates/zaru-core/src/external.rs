//! Handing a batch of photos to another program.
//!
//! After a pass, the keepers usually go somewhere else — DxO PureRAW, Topaz,
//! Photoshop. The mechanism is the one Windows itself uses for "Open with":
//! start the program with the file paths as arguments. DxO publishes no command
//! line for PureRAW, but its executable is registered against `.CR3` and takes
//! several files at once, which was verified before this was written.
//!
//! Nothing here is specific to one program. Hard-coding a vendor would not have
//! been less work, only more brittle the day the user changes tools.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Windows refuses a command line past 32767 characters, and the failure is a
/// process that never starts rather than a message. The batch is split well
/// under that so the quoting and the executable's own path always fit.
const COMMAND_LINE_BUDGET: usize = 28_000;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Program {
    pub path: String,
    /// What to call it on a button. The file name without its extension is
    /// usually right and is never wrong enough to matter.
    pub name: String,
}

impl Program {
    pub fn at(path: &Path) -> Program {
        let file = file_name(path);
        let name = file.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(&file).to_string();
        Program { path: path.display().to_string(), name }
    }
}

/// The last component of a path, splitting on either separator.
///
/// `Path::file_name` only knows the separator of the platform it runs on, so a
/// Windows path handled anywhere else comes back whole — and then a folder like
/// `DxO PureRAW 6` makes every file inside it look like the application,
/// including the installer. Settings travel between machines and tests run on
/// one platform for code that ships on another, so the split is explicit.
fn file_name(path: &Path) -> String {
    let text = path.display().to_string();
    text.rsplit(['/', '\\']).next().unwrap_or(&text).to_string()
}

/// Folders worth looking in, and what a photo editor's executable looks like.
const ROOTS: &[&str] = &[
    r"C:\Program Files",
    r"C:\Program Files (x86)",
    "/Applications",
];

const WANTED: &[&str] = &["pureraw", "photolab", "topaz", "photoshop", "affinity photo"];

/// Programs that are probably installed, for the settings screen to offer.
///
/// This is a convenience, not the mechanism: the path is a setting the user can
/// type, and a program nobody guessed still works.
pub fn detect() -> Vec<Program> {
    let mut found: Vec<Program> = Vec::new();
    for root in ROOTS {
        let root = Path::new(root);
        if !root.is_dir() {
            continue;
        }
        // Two levels is enough for `DxO\DxO PureRAW 6\PureRAWv6.exe` and stops
        // this from walking an entire Program Files.
        walk(root, 3, &mut found);
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found.dedup_by(|a, b| a.path == b.path);
    found
}

fn walk(dir: &Path, depth: usize, found: &mut Vec<Program>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, depth - 1, found);
        } else if is_editor(&path) {
            found.push(Program::at(&path));
        }
    }
}

/// True for an executable that looks like the application rather than one of
/// its helpers.
///
/// The helpers are the trap: DxO ships `PureRAWv6_saver.exe` beside
/// `PureRAWv6.exe`, and the first search written for this picked the saver,
/// which starts and exits without a window. A test that looks like a refusal
/// and is really the wrong binary costs more than the filter does.
pub fn is_editor(path: &Path) -> bool {
    let name = file_name(path).to_lowercase();
    if name.is_empty() {
        return false;
    }
    if ["_saver", "_helper", "crashpad", "install", "uninstall", "update", "process", "runtime"]
        .iter()
        .any(|helper| name.contains(helper))
    {
        return false;
    }
    let stem = name.trim_end_matches(".exe").replace(['-', '_'], "");
    WANTED.iter().any(|wanted| stem.contains(&wanted.replace(' ', "")))
}

/// Splits the files into as many runs as the command line needs.
///
/// One call would be simpler, and for a hundred keepers one call is what comes
/// back. A whole shoot is what breaks it: eighteen hundred paths is a hundred
/// thousand characters and the process would fail to start at all, silently.
pub fn batches(program: &Path, files: &[PathBuf]) -> Vec<Vec<PathBuf>> {
    // Every argument is quoted and separated by a space.
    let cost = |path: &Path| path.display().to_string().chars().count() + 3;
    let overhead = cost(program);

    let mut runs: Vec<Vec<PathBuf>> = Vec::new();
    let mut current: Vec<PathBuf> = Vec::new();
    let mut used = overhead;

    for file in files {
        let size = cost(file);
        // A single path longer than the whole budget cannot be sent at all, but
        // it also cannot exist: the filesystem caps a path far below this.
        if !current.is_empty() && used + size > COMMAND_LINE_BUDGET {
            runs.push(std::mem::take(&mut current));
            used = overhead;
        }
        used += size;
        current.push(file.clone());
    }
    if !current.is_empty() {
        runs.push(current);
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(count: usize, width: usize) -> Vec<PathBuf> {
        (0..count)
            .map(|i| PathBuf::from(format!("C:\\{}\\IMG_{i:05}.CR3", "x".repeat(width))))
            .collect()
    }

    #[test]
    fn a_normal_batch_goes_in_one_run() {
        let program = Path::new(r"C:\Program Files\DxO\DxO PureRAW 6\PureRAWv6.exe");
        let runs = batches(program, &paths(300, 20));
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].len(), 300);
    }

    #[test]
    fn a_whole_shoot_is_split_rather_than_failing_to_start() {
        // Eighteen hundred paths is well past what Windows will accept, and the
        // failure mode is a process that never starts.
        let program = Path::new(r"C:\Program Files\DxO\DxO PureRAW 6\PureRAWv6.exe");
        let files = paths(1800, 30);
        let runs = batches(program, &files);

        assert!(runs.len() > 1, "1800 caminhos deveriam precisar de mais de uma execução");
        assert_eq!(runs.iter().map(Vec::len).sum::<usize>(), files.len(), "nada se perde");

        let overhead = program.display().to_string().chars().count() + 3;
        for run in &runs {
            let length: usize =
                overhead + run.iter().map(|p| p.display().to_string().chars().count() + 3).sum::<usize>();
            assert!(length <= COMMAND_LINE_BUDGET, "uma execução passou do limite: {length}");
        }
        // And the order is preserved across the split.
        let flat: Vec<&PathBuf> = runs.iter().flatten().collect();
        assert_eq!(flat.first(), Some(&&files[0]));
        assert_eq!(flat.last(), Some(&&files[files.len() - 1]));
    }

    #[test]
    fn nothing_to_send_is_no_runs_at_all() {
        assert!(batches(Path::new("x.exe"), &[]).is_empty());
    }

    #[test]
    fn a_helper_beside_the_application_is_not_the_application() {
        // The saver starts and exits without a window; picking it looks exactly
        // like the program refusing the arguments.
        assert!(is_editor(Path::new(r"C:\Program Files\DxO\DxO PureRAW 6\PureRAWv6.exe")));
        assert!(!is_editor(Path::new(r"C:\Program Files\DxO\DxO PureRAW 6\PureRAWv6_saver.exe")));
        assert!(!is_editor(Path::new(r"C:\Program Files\DxO\DxO PureRAW 6\crashpad_handler.exe")));
        assert!(!is_editor(Path::new(
            r"C:\Program Files\DxO\DxO PureRAW 6\QtWebEngineProcess.exe"
        )));
        assert!(!is_editor(Path::new(
            r"C:\Program Files\DxO\DxO PureRAW 6\WindowsAppRuntimeInstall-x64.exe"
        )));
    }

    #[test]
    fn something_unrelated_is_not_offered() {
        assert!(!is_editor(Path::new(r"C:\Program Files\Notepad++\notepad++.exe")));
        assert!(!is_editor(Path::new(r"C:\Windows\explorer.exe")));
    }

    #[test]
    fn a_program_is_named_after_its_file() {
        let p = Program::at(Path::new(r"C:\Program Files\DxO\DxO PureRAW 6\PureRAWv6.exe"));
        assert_eq!(p.name, "PureRAWv6");
        assert!(p.path.ends_with("PureRAWv6.exe"));
    }
}
