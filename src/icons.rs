use ratatui::style::Color;

// yazi-style Nerd Font icons. Extension/filename -> (glyph, color).
// Requires a Nerd Font; can be disabled with `[ui] icons = false`.

pub const DIR_CLOSED: &str = "\u{f07b}"; //
pub const DIR_OPEN: &str = "\u{f07c}"; //
pub const FILE_DEFAULT: &str = "\u{f15b}"; //

pub const DIR_COLOR: Color = Color::Rgb(0x7a, 0xa2, 0xf7);

pub fn file_icon(name: &str) -> (&'static str, Color) {
    // Special cases matched on the exact filename
    match name {
        ".gitignore" | ".gitattributes" | ".gitmodules" => {
            return ("\u{e702}", Color::Rgb(0xf1, 0x50, 0x2f))
        }
        "Dockerfile" => return ("\u{f308}", Color::Rgb(0x45, 0x8e, 0xe6)),
        "Makefile" => return ("\u{e779}", Color::Rgb(0x6d, 0x80, 0x86)),
        "LICENSE" => return ("\u{f2c2}", Color::Rgb(0xcb, 0xcb, 0x41)),
        _ => {}
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match ext {
        "rs" => ("\u{e7a8}", Color::Rgb(0xde, 0xa5, 0x84)),
        "toml" => ("\u{e6b2}", Color::Rgb(0x9c, 0x42, 0x21)),
        "md" | "markdown" => ("\u{f48a}", Color::Rgb(0x51, 0x9a, 0xba)),
        "json" => ("\u{e60b}", Color::Rgb(0xcb, 0xcb, 0x41)),
        "yml" | "yaml" => ("\u{e615}", Color::Rgb(0x6d, 0x80, 0x86)),
        "js" | "mjs" | "cjs" => ("\u{e74e}", Color::Rgb(0xcb, 0xcb, 0x41)),
        "ts" | "mts" => ("\u{e628}", Color::Rgb(0x51, 0x9a, 0xba)),
        "tsx" | "jsx" => ("\u{e7ba}", Color::Rgb(0x51, 0x9a, 0xba)),
        "sh" | "zsh" | "bash" | "fish" => ("\u{f489}", Color::Rgb(0x4d, 0xaa, 0x57)),
        "py" => ("\u{e73c}", Color::Rgb(0xff, 0xbc, 0x03)),
        "go" => ("\u{e626}", Color::Rgb(0x51, 0x9a, 0xba)),
        "lua" => ("\u{e620}", Color::Rgb(0x51, 0x9a, 0xba)),
        "vim" => ("\u{e62b}", Color::Rgb(0x01, 0x98, 0x33)),
        "el" => ("\u{e632}", Color::Rgb(0x83, 0x58, 0xff)),
        "rb" => ("\u{e21e}", Color::Rgb(0x70, 0x15, 0x16)),
        "c" | "h" => ("\u{e61e}", Color::Rgb(0x59, 0x9e, 0xff)),
        "cpp" | "cc" | "hpp" => ("\u{e61d}", Color::Rgb(0xf3, 0x47, 0xb8)),
        "java" => ("\u{e738}", Color::Rgb(0xcc, 0x3e, 0x44)),
        "kt" | "kts" => ("\u{e634}", Color::Rgb(0x7f, 0x52, 0xff)),
        "swift" => ("\u{e755}", Color::Rgb(0xe3, 0x79, 0x33)),
        "html" | "htm" => ("\u{e736}", Color::Rgb(0xe3, 0x4c, 0x26)),
        "css" | "scss" | "sass" => ("\u{e749}", Color::Rgb(0x56, 0x3d, 0x7c)),
        "nix" => ("\u{f313}", Color::Rgb(0x7e, 0xba, 0xe4)),
        "lock" => ("\u{f023}", Color::Rgb(0x6d, 0x80, 0x86)),
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" => {
            ("\u{f1c5}", Color::Rgb(0xa0, 0x74, 0xc4))
        }
        "pdf" => ("\u{f1c1}", Color::Rgb(0xb3, 0x0b, 0x00)),
        "zip" | "gz" | "tar" | "xz" | "zst" => ("\u{f410}", Color::Rgb(0xec, 0xa5, 0x17)),
        "org" => ("\u{e633}", Color::Rgb(0x77, 0xaa, 0x99)),
        "txt" => ("\u{f15c}", Color::Rgb(0x89, 0xe0, 0x51)),
        _ => (FILE_DEFAULT, Color::Rgb(0x6d, 0x80, 0x86)),
    }
}
