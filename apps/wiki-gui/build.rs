use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let mut generated = String::new();
    for (name, variable) in [("SANS", "WIKI_SANS_FONT"), ("MONO", "WIKI_MONO_FONT")] {
        println!("cargo:rerun-if-env-changed={variable}");
        let path = env::var(variable)
            .ok()
            .map(PathBuf::from)
            .filter(|path| path.is_file());
        match path {
            Some(path) => generated.push_str(&format!(
                "pub const {name}: &[u8] = include_bytes!({path:?});\n"
            )),
            None => generated.push_str(&format!("pub const {name}: &[u8] = &[];\n")),
        }
    }
    let out = Path::new(&env::var("OUT_DIR").expect("OUT_DIR")).join("fonts.rs");
    fs::write(out, generated).expect("write fonts.rs");
}
