//! The startup banner printed by the `sheathe` CLI.

use base64::{Engine, engine::general_purpose::STANDARD};
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor};
use crossterm::{execute, style::Attribute, style::SetAttribute};
use std::io::{IsTerminal, Write, stderr};

const LOGO_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/logo.png"));

const NAVY: (u8, u8, u8) = (26, 36, 64);
const SLATE: (u8, u8, u8) = (71, 85, 105);
/// Inline-image display size in terminal pixels (embedded asset is 256×256).
const LOGO_DISPLAY_PX: u32 = 120;
/// Wordmark used when the inline PNG logo cannot be shown.
const WORDMARK: &str = "🗡️ sheathe ";
/// Kitty graphics-protocol base64 chunk size.
const KITTY_CHUNK: usize = 4096;

enum ImageProtocol {
    /// iTerm2 OSC 1337 `File=` (iTerm, Konsole).
    Iterm2,
    /// Kitty graphics protocol (Ghostty, kitty, WezTerm, rio).
    Kitty,
}

/// Print the logo and version to stderr. The tagline lives in clap's `about` text.
pub(crate) fn print() {
    let color = color_enabled();
    let truecolor = color && supports_truecolor();
    let logo = color && render_logo();

    if color {
        let mut out = stderr();
        let _ = execute!(out, Print("  "));
        if !logo {
            let _ = execute!(
                out,
                SetAttribute(Attribute::Bold),
                SetForegroundColor(rgb(NAVY, truecolor)),
                Print(WORDMARK),
                SetAttribute(Attribute::Reset),
            );
        } else {
            let _ = execute!(out, SetForegroundColor(rgb(NAVY, truecolor)), Print("sheathe "),);
        }
        let _ = execute!(
            out,
            SetForegroundColor(rgb(SLATE, truecolor)),
            Print(env!("CARGO_PKG_VERSION")),
            ResetColor,
            Print('\n'),
        );
    } else {
        eprintln!("  {WORDMARK}{}", env!("CARGO_PKG_VERSION"));
    }
}

fn color_enabled() -> bool {
    std::env::var("NO_COLOR").is_err() && stderr().is_terminal()
}

fn supports_truecolor() -> bool {
    if std::env::var("COLORTERM")
        .map(|c| {
            let c = c.to_ascii_lowercase();
            c.contains("truecolor") || c.contains("24bit")
        })
        .unwrap_or(false)
    {
        return true;
    }

    term_program_is("iTerm.app")
        || term_program_is("WezTerm")
        || term_program_is("ghostty")
        || term_program_is("WarpTerminal")
        || std::env::var("TERM").is_ok_and(|t| t.contains("kitty") || t.contains("ghostty"))
}

fn term_program_is(name: &str) -> bool {
    std::env::var("TERM_PROGRAM").is_ok_and(|t| t == name)
}

fn image_protocol() -> Option<ImageProtocol> {
    if kitty_graphics() {
        return Some(ImageProtocol::Kitty);
    }
    if iterm2_inline_images() {
        return Some(ImageProtocol::Iterm2);
    }
    None
}

fn kitty_graphics() -> bool {
    term_program_is("ghostty")
        || term_program_is("WezTerm")
        || term_program_is("rio")
        || term_program_is("kitty")
        || std::env::var("KITTY_WINDOW_ID").is_ok()
        || std::env::var("TERM").is_ok_and(|t| {
            let t = t.to_ascii_lowercase();
            t.contains("kitty") || t.contains("ghostty")
        })
}

/// iTerm2 OSC 1337 only. Warp's `File=` path often renders a Failed image block;
/// Ghostty does not implement OSC 1337.
fn iterm2_inline_images() -> bool {
    term_program_is("iTerm.app")
        || std::env::var("LC_TERMINAL").is_ok_and(|t| t == "iTerm2")
        || std::env::var("KONSOLE_VERSION").is_ok()
}

/// Inline PNG via Kitty or iTerm2. No half-block fallback — text wordmark otherwise.
fn render_logo() -> bool {
    match image_protocol() {
        Some(ImageProtocol::Kitty) => render_logo_kitty(),
        Some(ImageProtocol::Iterm2) => render_logo_iterm2(),
        None => false,
    }
}

fn render_logo_iterm2() -> bool {
    let b64 = STANDARD.encode(LOGO_BYTES);
    let name = STANDARD.encode("logo.png");
    writeln!(
        stderr(),
        "  \x1b]1337;File=name={name};inline=1;preserveAspectRatio=1;width={LOGO_DISPLAY_PX}px;height={LOGO_DISPLAY_PX}px;size={}:{b64}\x07",
        LOGO_BYTES.len(),
    )
    .is_ok()
}

fn render_logo_kitty() -> bool {
    let b64 = STANDARD.encode(LOGO_BYTES);
    // Approximate 120px at a typical 8×16 cell.
    let cols = (LOGO_DISPLAY_PX / 8).max(1);
    let rows = (LOGO_DISPLAY_PX / 16).max(1);
    let mut out = stderr();
    if write!(out, "  ").is_err() {
        return false;
    }
    let total = b64.len();
    let mut offset = 0;
    while offset < total {
        let end = (offset + KITTY_CHUNK).min(total);
        let more = u8::from(end < total);
        let chunk = &b64[offset..end];
        let wrote = if offset == 0 {
            write!(out, "\x1b_Ga=T,f=100,c={cols},r={rows},m={more};{chunk}\x1b\\")
        } else {
            write!(out, "\x1b_Gm={more};{chunk}\x1b\\")
        };
        if wrote.is_err() {
            return false;
        }
        offset = end;
    }
    writeln!(out).is_ok()
}

fn rgb((r, g, b): (u8, u8, u8), truecolor: bool) -> Color {
    if truecolor { Color::Rgb { r, g, b } } else { Color::AnsiValue(to_xterm256(r, g, b)) }
}

fn to_xterm256(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [i32; 6] = [0, 95, 135, 175, 215, 255];
    let nearest = |v: i32| -> usize {
        let mut best = 0;
        let mut best_dist = i32::MAX;
        for (i, &level) in LEVELS.iter().enumerate() {
            let dist = (v - level).abs();
            if dist < best_dist {
                best_dist = dist;
                best = i;
            }
        }
        best
    };

    let (r, g, b) = (r as i32, g as i32, b as i32);
    let (ri, gi, bi) = (nearest(r), nearest(g), nearest(b));
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let cube_idx = 16 + 36 * ri + 6 * gi + bi;

    let gray_i = (((r + g + b) / 3 - 8) as f32 / 10.0).round().clamp(0.0, 23.0) as i32;
    let gray_v = 8 + 10 * gray_i;
    let gray_idx = 232 + gray_i as usize;

    let dist = |c: (i32, i32, i32)| (c.0 - r).pow(2) + (c.1 - g).pow(2) + (c.2 - b).pow(2);
    if dist((gray_v, gray_v, gray_v)) < dist(cube) { gray_idx as u8 } else { cube_idx as u8 }
}
