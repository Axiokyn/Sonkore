//! Compiles the Slint interface (crates/app/ui), with the translations
//! (i18n/<lang>.po, gettext) bundled in the binary.

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let i18n = root.join("i18n");
    // Slint bundles <dir>/<lang>/LC_MESSAGES/<package name>.po.
    let domain = env!("CARGO_PKG_NAME");
    // slint-build watches this folder (rerun-if-changed): a file is written
    // only when its text changed, or every build would run this script again
    // and compile the whole interface (13 s for any change to the app).
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("translations");
    println!("cargo:rerun-if-changed={}", i18n.display());
    for entry in fs::read_dir(&i18n).expect("i18n").flatten() {
        let path = entry.path();
        let Some(lang) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".po")) else {
            continue;
        };
        println!("cargo:rerun-if-changed={}", path.display());
        let dir = out.join(lang).join("LC_MESSAGES");
        fs::create_dir_all(&dir).unwrap();
        let text = fs::read(&path).unwrap();
        let copy = dir.join(format!("{domain}.po"));
        if fs::read(&copy).ok().as_deref() != Some(&text[..]) {
            fs::write(&copy, &text).unwrap();
        }
    }
    let config = slint_build::CompilerConfiguration::new().with_bundled_translations(&out);
    slint_build::compile_with_config(root.join("crates/app/ui/main.slint"), config).expect("Slint UI");
}
