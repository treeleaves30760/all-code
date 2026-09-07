//! The page's static files, compiled into the binary.
//!
//! Compressed at build time (see `build.rs`) and served with
//! `Content-Encoding: gzip`, so alc ships no compressor and a phone on a
//! slow link is not sent half a megabyte of uncompressed JavaScript.

macro_rules! asset {
    ($name:literal) => {
        include_bytes!(concat!(env!("OUT_DIR"), "/", $name, ".gz"))
    };
}

/// One servable file: its route, its media type and its gzipped bytes.
pub(crate) struct Asset {
    pub path: &'static str,
    pub content_type: &'static str,
    pub gzipped: &'static [u8],
}

const HTML: &str = "text/html; charset=utf-8";
const JS: &str = "text/javascript; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";

pub(crate) const ASSETS: [Asset; 6] = [
    Asset {
        path: "/",
        content_type: HTML,
        gzipped: asset!("index.html"),
    },
    Asset {
        path: "/assets/app.js",
        content_type: JS,
        gzipped: asset!("app.js"),
    },
    Asset {
        path: "/assets/app.css",
        content_type: CSS,
        gzipped: asset!("app.css"),
    },
    Asset {
        path: "/assets/xterm.js",
        content_type: JS,
        gzipped: asset!("vendor-xterm.js"),
    },
    Asset {
        path: "/assets/xterm.css",
        content_type: CSS,
        gzipped: asset!("vendor-xterm.css"),
    },
    Asset {
        path: "/assets/addon-fit.js",
        content_type: JS,
        gzipped: asset!("vendor-addon-fit.js"),
    },
];

pub(crate) fn find(path: &str) -> Option<&'static Asset> {
    ASSETS.iter().find(|asset| asset.path == path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_asset_is_present_and_non_empty() {
        for asset in &ASSETS {
            assert!(!asset.gzipped.is_empty(), "{} is empty", asset.path);
        }
    }

    #[test]
    fn the_index_is_served_at_the_root() {
        assert!(find("/").is_some());
        assert!(find("/assets/xterm.js").is_some());
    }

    #[test]
    fn an_unknown_path_is_not_an_asset() {
        // The static table is the whole allowlist; there is no filesystem
        // lookup behind it, so path traversal has nothing to reach.
        assert!(find("/assets/../../etc/passwd").is_none());
        assert!(find("/etc/passwd").is_none());
    }

    #[test]
    fn assets_are_gzip_streams() {
        for asset in &ASSETS {
            assert_eq!(
                &asset.gzipped[..2],
                &[0x1f, 0x8b],
                "{} is not gzip",
                asset.path
            );
        }
    }
}
