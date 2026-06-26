//! Embed a CLI-sized logo from `docs/banner.svg` at compile time.

use image::codecs::png::PngEncoder;
use image::{ColorType, ImageEncoder, imageops, load_from_memory};
use std::env;
use std::fs;
use std::path::PathBuf;

/// Embedded PNG resolution (display size is set separately in `banner.rs`).
const CLI_LOGO_PX: u32 = 256;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let svg_path = manifest_dir.join("../../docs/banner.svg");
    let svg = fs::read_to_string(&svg_path).unwrap_or_else(|e| {
        panic!("reading {}: {e}", svg_path.display());
    });

    let marker = "href=\"data:image/png;base64,";
    let start = svg.find(marker).unwrap_or_else(|| panic!("embedded logo not found in banner.svg"))
        + marker.len();
    let end = start + svg[start..].find('"').expect("unterminated base64 in banner.svg");
    let png = decode_base64(&svg[start..end]).expect("invalid logo base64 in banner.svg");

    let img = load_from_memory(&png).expect("decoding embedded logo png");
    let rgba =
        imageops::resize(&img.to_rgba8(), CLI_LOGO_PX, CLI_LOGO_PX, imageops::FilterType::Triangle);

    let mut out_png = Vec::new();
    PngEncoder::new(&mut out_png)
        .write_image(rgba.as_raw(), CLI_LOGO_PX, CLI_LOGO_PX, ColorType::Rgba8.into())
        .expect("encoding cli logo png");

    let logo_path = out_dir.join("logo.png");
    fs::write(&logo_path, &out_png).expect("writing OUT_DIR/logo.png");
    println!("cargo:rerun-if-changed={}", svg_path.display());
}

fn decode_base64(input: &str) -> Result<Vec<u8>, &'static str> {
    const TABLE: [i8; 128] = {
        let mut t = [-1i8; 128];
        let mut i = 0u8;
        while i < 26 {
            t[(b'A' + i) as usize] = i as i8;
            t[(b'a' + i) as usize] = (i + 26) as i8;
            i += 1;
        }
        let mut i = 0u8;
        while i < 10 {
            t[(b'0' + i) as usize] = (i + 52) as i8;
            i += 1;
        }
        t[b'+' as usize] = 62;
        t[b'/' as usize] = 63;
        t
    };

    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u32;

    for &byte in input.as_bytes() {
        if byte == b'=' {
            break;
        }
        if byte >= 128 {
            return Err("non-ascii base64");
        }
        let val = TABLE[byte as usize];
        if val < 0 {
            continue;
        }
        buf = (buf << 6) | val as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }

    Ok(out)
}
