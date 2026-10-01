//! Reading printed scores (PDF, images) with Audiveris, an open source optical music
//! recognition program (AGPL, installed separately: https://github.com/Audiveris/audiveris).
//! It is run as a separate program; its MusicXML output is what we use.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// Audiveris constant that turns on the recognition of fingering digits
const FINGERINGS_SWITCH: &str = "org.audiveris.omr.sheet.ProcessingSwitches.fingerings=true";

/// Where Audiveris is: `configured`, the `AUDIVERIS` variable, the usual install folders,
/// or the PATH
pub fn find_audiveris(configured: Option<&Path>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    candidates.extend(configured.map(Path::to_path_buf));
    candidates.extend(std::env::var_os("AUDIVERIS").map(PathBuf::from));
    for var in ["ProgramFiles", "ProgramW6432", "LOCALAPPDATA"] {
        if let Some(base) = std::env::var_os(var) {
            let base = PathBuf::from(base);
            candidates.push(base.join("Audiveris").join("Audiveris.exe"));
            candidates.push(
                base.join("Programs")
                    .join("Audiveris")
                    .join("Audiveris.exe"),
            );
        }
    }
    candidates.push("/Applications/Audiveris.app/Contents/MacOS/Audiveris".into());
    candidates.push("/opt/audiveris/bin/Audiveris".into());
    candidates.push("/usr/bin/audiveris".into());
    if let Some(found) = candidates.into_iter().find(|p| p.is_file()) {
        return Some(found);
    }

    // On the PATH
    let names: &[&str] = if cfg!(windows) {
        &["Audiveris.exe", "audiveris.exe"]
    } else {
        &["Audiveris", "audiveris"]
    };
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
            .find(|p| p.is_file())
    })
}

/// Recognize `input` (PDF or image) with fingering detection on; returns the MusicXML of
/// every movement found, in order. `work_dir` receives Audiveris' files.
pub fn recognize(audiveris: &Path, input: &Path, work_dir: &Path) -> Result<Vec<String>, String> {
    std::fs::create_dir_all(work_dir).map_err(|e| e.to_string())?;

    let output = run(
        audiveris,
        &[
            "-batch".as_ref(),
            "-transcribe".as_ref(),
            "-export".as_ref(),
            "-constant".as_ref(),
            FINGERINGS_SWITCH.as_ref(),
            "-output".as_ref(),
            work_dir.as_os_str(),
            "--".as_ref(),
            input.as_os_str(),
        ],
    )?;

    // Our own reading of the digits, exported by Audiveris again
    let mut scores = Vec::new();
    match read_digits(audiveris, work_dir) {
        Ok(fingered) => scores = fingered,
        Err(err) => log::warn!("Reading fingering digits: {err}"),
    }
    if scores.is_empty() {
        scores = mxl_files(work_dir);
    }
    if scores.is_empty() {
        let log = String::from_utf8_lossy(&output.stderr);
        let tail: String = log.lines().rev().take(5).collect::<Vec<_>>().join(" | ");
        return Err(format!(
            "Audiveris found no music in {} ({}){}",
            input.display(),
            output.status,
            if tail.is_empty() {
                String::new()
            } else {
                format!(": {tail}")
            }
        ));
    }
    scores.iter().map(|p| super::read_file(p)).collect()
}

fn run(audiveris: &Path, args: &[&std::ffi::OsStr]) -> Result<std::process::Output, String> {
    let mut command = Command::new(audiveris);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window popping up
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        .output()
        .map_err(|e| format!("Could not run Audiveris ({}): {e}", audiveris.display()))
}

/// The .mxl files right in `dir`, in name order
fn mxl_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mxl")))
        .collect();
    out.sort();
    out
}

/// Find the fingering digits in the project Audiveris left in `work_dir`
/// (see [`super::omr_fingering`]) and export it again; the new MusicXML files
fn read_digits(audiveris: &Path, work_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let project = std::fs::read_dir(work_dir)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("omr")))
        .ok_or("no Audiveris project")?;
    let bytes = std::fs::read(&project).map_err(|e| e.to_string())?;
    let (patched, report) = super::omr_fingering::patch_omr(&bytes)?;
    log::info!(
        "Fingering digits: {} found, {} attached to notes",
        report.digits,
        report.attached
    );

    let dir = work_dir.join("fingered");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(project.file_name().unwrap());
    std::fs::write(&path, patched).map_err(|e| e.to_string())?;
    run(
        audiveris,
        &[
            "-batch".as_ref(),
            "-export".as_ref(),
            "--".as_ref(),
            path.as_os_str(),
        ],
    )?;
    let scores = mxl_files(&dir);
    if scores.is_empty() {
        return Err("Audiveris did not export the fingered project".into());
    }
    Ok(scores)
}
