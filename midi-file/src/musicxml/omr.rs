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

    let mut command = Command::new(audiveris);
    command
        .arg("-batch")
        .arg("-transcribe")
        .arg("-export")
        .arg("-constant")
        .arg(FINGERINGS_SWITCH)
        .arg("-output")
        .arg(work_dir)
        .arg("--")
        .arg(input);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window popping up
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command
        .output()
        .map_err(|e| format!("Could not run Audiveris ({}): {e}", audiveris.display()))?;

    let mut scores: Vec<PathBuf> = Vec::new();
    collect_mxl(work_dir, &mut scores);
    scores.sort();
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

fn collect_mxl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_mxl(&path, out);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("mxl"))
        {
            out.push(path);
        }
    }
}
