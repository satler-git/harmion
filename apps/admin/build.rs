use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-changed=ui/src");
    println!("cargo:rerun-if-changed=ui/index.html");
    println!("cargo:rerun-if-changed=ui/package.json");
    println!("cargo:rerun-if-changed=ui/package-lock.json");
    println!("cargo:rerun-if-changed=ui/vite.config.ts");
    println!("cargo:rerun-if-changed=ui/tsconfig.json");
    println!("cargo:rerun-if-env-changed=NPM");
    println!("cargo:rerun-if-env-changed=HARMION_ADMIN_UI_DIST");
    let ui = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("ui");
    let dist = match env::var_os("HARMION_ADMIN_UI_DIST") {
        Some(path) => {
            let path = PathBuf::from(path);
            assert!(
                path.join("index.html").is_file(),
                "HARMION_ADMIN_UI_DIST must point at a built UI directory (index.html missing)"
            );
            println!("cargo:rerun-if-changed={}", path.display());
            path
        }
        None => {
            assert!(
                ui.join("node_modules").is_dir(),
                "Run npm ci --prefix apps/admin/ui before building harmion-admin, or set HARMION_ADMIN_UI_DIST"
            );
            let result = Command::new(env::var_os("NPM").unwrap_or_else(|| "npm".into()))
                .args(["run", "build"])
                .current_dir(&ui)
                .status()
                .expect("npm is required to build harmion-admin (set NPM to its path)");
            assert!(result.success(), "admin UI build failed");
            ui.join("dist")
        }
    };
    let mut entries = Vec::new();
    collect(&dist, &dist, &mut entries);
    let mut output = String::from(
        "pub fn asset(path: &str) -> Option<(&'static str, &'static [u8])> { match path {\n",
    );
    for (route, path) in entries {
        let mime = match path.extension().and_then(|s| s.to_str()) {
            Some("html") => "text/html; charset=utf-8",
            Some("js") => "text/javascript; charset=utf-8",
            Some("css") => "text/css; charset=utf-8",
            _ => panic!("unexpected frontend asset"),
        };
        output.push_str(&format!(
            "{route:?} => Some(({mime:?}, include_bytes!({path:?}))),\n"
        ));
    }
    output.push_str("_ => None, } }\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("assets.rs"),
        output,
    )
    .unwrap();
}

fn collect(root: &Path, directory: &Path, output: &mut Vec<(String, PathBuf)>) {
    let mut entries: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .map(Result::unwrap)
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, output);
        } else {
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .replace('\\', "/");
            let route = if relative == "index.html" {
                "/".into()
            } else {
                format!("/{relative}")
            };
            output.push((route, path));
        }
    }
}
