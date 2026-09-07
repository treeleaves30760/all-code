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

const ASSETS: [&str; 6] = [
    "index.html",
    "app.js",
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
}
