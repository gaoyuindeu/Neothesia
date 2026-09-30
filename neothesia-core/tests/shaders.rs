//! Shaders are only compiled when their pipeline is created at runtime,
//! so parse and validate all of them here.

use std::path::Path;

fn collect(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "wgsl") {
            out.push(path);
        }
    }
}

#[test]
fn all_shaders_are_valid() {
    let mut shaders = Vec::new();
    collect(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut shaders,
    );
    assert!(!shaders.is_empty());

    let mut failed = Vec::new();
    for path in &shaders {
        let source = std::fs::read_to_string(path).unwrap();
        let module = match naga::front::wgsl::parse_str(&source) {
            Ok(module) => module,
            Err(err) => {
                failed.push(format!(
                    "{}:\n{}",
                    path.display(),
                    err.emit_to_string(&source)
                ));
                continue;
            }
        };

        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        );
        if let Err(err) = validator.validate(&module) {
            failed.push(format!(
                "{}:\n{}",
                path.display(),
                err.emit_to_string(&source)
            ));
        }
    }

    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}
