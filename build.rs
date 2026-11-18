//! Compresses the remote-control page's assets into the binary.
//!
//! The page must work with no network and under a `default-src 'self'`
//! content-security policy, so xterm.js is vendored rather than fetched from
//! a CDN. Serving it raw would put half a megabyte of JavaScript on every
//! page load over what may be a phone's cellular link, so each asset is
//! gzipped here and served with `Content-Encoding: gzip`. Doing it at build
//! time rather than per request keeps the server free of a compressor and
//! makes the cost show up once, in `cargo build`, instead of on every view.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;

const ASSETS: [&str; 11] = [
    "index.html",
    "app.js",
    "dashboard.js",
    "shell.js",
    "layout.js",
    "studio.js",
    "core.js",
    "app.css",
    "vendor/xterm.js",
    "vendor/xterm.css",
    "vendor/addon-fit.js",
];

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let web = Path::new("web");

    println!("cargo:rerun-if-changed=web");
    for asset in ASSETS {
        let source = web.join(asset);
        println!("cargo:rerun-if-changed={}", source.display());

        let bytes = fs::read(&source)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", source.display()));
        let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
        encoder
            .write_all(&bytes)
            .expect("gzip encoder accepts the asset");
        let compressed = encoder.finish().expect("gzip encoder finishes");

        // Flattened so the include! sites need no directory structure.
        let name = asset.replace('/', "-");
        fs::write(out_dir.join(format!("{name}.gz")), compressed)
            .unwrap_or_else(|error| panic!("failed to write {name}.gz: {error}"));
    }

    write_custom_files(&out_dir);
}

/// Lists every file under `web/custom/examples` - the official mods,
/// packages, themes and new-mod templates - as `include_str!`s, so adding
/// one is dropping it in the folder rather than editing a table.
fn write_custom_files(out_dir: &Path) {
    let root = Path::new("web/custom");
    let mut files = Vec::new();
    collect(&root.join("examples"), root, &mut files);
    files.sort();
    let mut source = String::from("pub(crate) const CUSTOM_FILES: &[(&str, &str)] = &[\n");
    for relative in files {
        let absolute = fs::canonicalize(root.join(&relative))
            .unwrap_or_else(|error| panic!("failed to resolve {relative}: {error}"));
        source.push_str(&format!(
            "    ({relative:?}, include_str!({:?})),\n",
            absolute.display().to_string()
        ));
    }
    source.push_str("];\n");
    fs::write(out_dir.join("custom_files.rs"), source).expect("write custom_files.rs");
}

fn collect(dir: &Path, root: &Path, files: &mut Vec<String>) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, root, files);
        } else if path.file_name().is_some_and(|name| name != ".DS_Store") {
            let relative = path
                .strip_prefix(root)
                .expect("under the root")
                .to_string_lossy()
                .replace('\\', "/");
            files.push(relative);
        }
    }
}
