// SPDX-License-Identifier: EPL-2.0

//! The fonts, for all that robots print: egui's (Ubuntu Light, Hack) with a newer Noto
//! Emoji than egui ships (`assets/fonts`), and then the system's own fonts for the scripts
//! those lack (Chinese, Japanese, Korean, Arabic, Hebrew, Indic, Thai, more symbols...).
//! Those are big (a CJK font is 10 to 20 MB), so they are read only once text that needs
//! them is shown ([`cover`]), on a thread of their own; `ROBOCODES_GUI_FONTS` (a path
//! list of font files) adds fonts before them.
//!
//! egui draws glyph outlines, so emoji are monochrome: the colour emoji fonts that are
//! bitmaps (Noto Color Emoji, Apple Color Emoji) can't be used.

use eframe::egui::{self, FontData, FontFamily, FontId};
use eframe::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const EMOJI: &[u8] = include_bytes!("../assets/fonts/NotoEmoji-Regular.ttf");
/// egui's name for its emoji font: the newer one takes its place in the families.
const EMOJI_NAME: &str = "NotoEmoji-Regular";

/// egui's fonts with the newer emoji font.
pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    // egui's scale for its emoji (a little smaller than the text) suits this one too.
    let tweak = fonts
        .font_data
        .get(EMOJI_NAME)
        .map(|d| d.tweak.clone())
        .unwrap_or_default();
    fonts.font_data.insert(
        EMOJI_NAME.into(),
        Arc::new(FontData::from_static(EMOJI).tweak(tweak)),
    );
    ctx.set_fonts(fonts);
}

/// The system's fonts are being read, or have been.
static MORE: AtomicBool = AtomicBool::new(false);

/// Makes sure `text`, shown in widget `id`, can be drawn: when it has a character no font
/// has, the system's fonts for more scripts are read (once; they apply from a later
/// frame). Text that only grew since the last frame (console output) has just its new
/// part looked at, and unchanged text nothing.
pub fn cover(ui: &egui::Ui, id: egui::Id, text: &str) {
    if MORE.load(Ordering::Relaxed) || text.is_ascii() {
        return;
    }
    let id = id.with("fonts-checked");
    let now = Checked::of(text);
    let before = ui.data(|d| d.get_temp::<Checked>(id));
    ui.data_mut(|d| d.insert_temp(id, now));
    let new = match before {
        Some(b) if b == now => return,
        Some(b) if b.len < now.len => text.get(b.len..).unwrap_or(text),
        // Changed otherwise (cleared, its start cut off, replaced): all of it.
        _ => text,
    };
    let font = FontId::monospace(12.0);
    let missing = ui.ctx().fonts_mut(|f| {
        new.chars()
            .filter(|&c| !c.is_ascii() && !invisible(c))
            .any(|c| !f.has_glyph(&font, c))
    });
    if missing && !MORE.swap(true, Ordering::Relaxed) {
        let ctx = ui.ctx().clone();
        std::thread::spawn(move || load_more(&ctx));
    }
}

/// What [`cover`] looked at last: the text's length and a hash of its end.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Checked {
    len: usize,
    end: u64,
}

impl Checked {
    fn of(text: &str) -> Self {
        use std::hash::{Hash, Hasher};
        let bytes = text.as_bytes();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes[bytes.len().saturating_sub(64)..].hash(&mut hasher);
        Self {
            len: bytes.len(),
            end: hasher.finish(),
        }
    }
}

/// Characters that are drawn as nothing (or join others), which fonts needn't have.
fn invisible(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}' | '\u{2060}'..='\u{2064}'
        | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}' | '\u{E0000}'..='\u{E007F}' | '\u{E0100}'..='\u{E01EF}')
}

/// Reads the system's fonts for more scripts and adds them after egui's, in both families.
fn load_more(ctx: &egui::Context) {
    let mut files = crate::settings::env_paths("ROBOCODES_GUI_FONTS");
    files.extend(pick(GROUPS, &index(&font_dirs())));
    for path in files {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let name = format!("system: {}", path.display());
        let families = [FontFamily::Proportional, FontFamily::Monospace]
            .map(|family| InsertFontFamily {
                family,
                priority: FontPriority::Lowest,
            })
            .into();
        ctx.add_font(FontInsert::new(
            &name,
            FontData::from_owned(bytes),
            families,
        ));
    }
    ctx.request_repaint();
}

/// The font files to use: per group (fonts for the same scripts, best first) the first one
/// that is there, by file name in any case.
fn pick(groups: &[&[&str]], files: &HashMap<String, PathBuf>) -> Vec<PathBuf> {
    groups
        .iter()
        .filter_map(|group| {
            group
                .iter()
                .find_map(|name| files.get(&name.to_lowercase()).cloned())
        })
        .collect()
}

/// The font files in `dirs` and below, by their file name in lower case (the first one
/// found of a name). Bounded, so that a huge font folder stays quick.
fn index(dirs: &[PathBuf]) -> HashMap<String, PathBuf> {
    const DEPTH: usize = 6;
    const ENTRIES: usize = 50_000;
    let mut files = HashMap::new();
    let mut seen = 0;
    let mut stack: Vec<(PathBuf, usize)> = dirs.iter().rev().map(|d| (d.clone(), 0)).collect();
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut subdirs = Vec::new();
        for entry in entries.flatten() {
            seen += 1;
            if seen > ENTRIES {
                return files;
            }
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                if depth < DEPTH {
                    subdirs.push((path, depth + 1));
                }
            } else if is_font(&path) {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                files.entry(name).or_insert(path);
            }
        }
        subdirs.sort();
        stack.extend(subdirs.into_iter().rev());
    }
    files
}

fn is_font(path: &Path) -> bool {
    path.extension().is_some_and(|e| {
        ["ttf", "otf", "ttc"]
            .iter()
            .any(|f| e.eq_ignore_ascii_case(f))
    })
}

/// Where the system keeps its fonts, and the user's own.
fn font_dirs() -> Vec<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let mut dirs = Vec::new();
    if cfg!(windows) {
        let windows = var("WINDIR")
            .or_else(|| var("SystemRoot"))
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        dirs.push(windows.join("Fonts"));
        dirs.extend(var("LOCALAPPDATA").map(|d| d.join(r"Microsoft\Windows\Fonts")));
    } else if cfg!(target_os = "macos") {
        dirs.extend(["/System/Library/Fonts", "/Library/Fonts"].map(PathBuf::from));
        dirs.extend(var("HOME").map(|h| h.join("Library/Fonts")));
    } else {
        let home = var("HOME");
        dirs.extend(
            var("XDG_DATA_HOME")
                .or_else(|| home.as_ref().map(|h| h.join(".local/share")))
                .map(|d| d.join("fonts")),
        );
        dirs.extend(home.map(|h| h.join(".fonts")));
        let data = std::env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        dirs.extend(
            data.split(':')
                .filter(|d| !d.is_empty())
                .map(|d| Path::new(d).join("fonts")),
        );
    }
    dirs
}

/// The fonts looked for, by file name, in groups for the same scripts (best first).
#[cfg(windows)]
const GROUPS: &[&[&str]] = &[
    &["seguisym.ttf"],
    // Arabic, Hebrew, Armenian, Georgian and more.
    &["segoeui.ttf"],
    &["msyh.ttc", "simsun.ttc"],
    &["YuGothR.ttc", "msgothic.ttc"],
    &["malgun.ttf", "gulim.ttc"],
    &["Nirmala.ttc", "Nirmala.ttf", "mangal.ttf"],
    // Thai, Lao, Khmer.
    &["LeelawUI.ttf", "tahoma.ttf"],
    &["ebrima.ttf"],
    // Emoji that Noto Emoji lacks.
    &["seguiemj.ttf"],
];

#[cfg(target_os = "macos")]
const GROUPS: &[&[&str]] = &[
    &["Apple Symbols.ttf"],
    &["PingFang.ttc", "Hiragino Sans GB.ttc", "STHeiti Light.ttc"],
    &["AppleSDGothicNeo.ttc"],
    &["GeezaPro.ttc", "SFArabic.ttf"],
    &["ArialHB.ttc", "SFHebrew.ttf"],
    &["Kohinoor.ttc", "KohinoorDevanagari.ttc", "DevanagariMT.ttc"],
    &["Thonburi.ttc"],
    &["Arial Unicode.ttf"],
];

#[cfg(not(any(windows, target_os = "macos")))]
const GROUPS: &[&[&str]] = &[
    &[
        "NotoSansCJK-Regular.ttc",
        "NotoSansCJKsc-Regular.otf",
        "NotoSansCJK-VF.ttc",
        "NotoSansCJK-VF.otf.ttc",
        "SourceHanSans-Regular.ttc",
        "SourceHanSansSC-Regular.otf",
        "NotoSansSC-Regular.otf",
        "NotoSansSC-Regular.ttf",
        "wqy-microhei.ttc",
        "wqy-zenhei.ttc",
        "DroidSansFallbackFull.ttf",
        "DroidSansFallback.ttf",
    ],
    &["NotoSansKR-Regular.otf", "NanumGothic.ttf", "UnDotum.ttf"],
    &["NotoSansArabic-Regular.ttf", "NotoNaskhArabic-Regular.ttf"],
    &["NotoSansHebrew-Regular.ttf"],
    &["NotoSansDevanagari-Regular.ttf", "Lohit-Devanagari.ttf"],
    &["NotoSansBengali-Regular.ttf", "Lohit-Bengali.ttf"],
    &["NotoSansTamil-Regular.ttf", "Lohit-Tamil.ttf"],
    &["NotoSansThai-Regular.ttf", "Garuda.otf", "Loma.ttf"],
    &["NotoSansArmenian-Regular.ttf"],
    &["NotoSansGeorgian-Regular.ttf"],
    &["NotoSansEthiopic-Regular.ttf"],
    &["NotoSansSymbols-Regular.ttf"],
    &["NotoSansSymbols2-Regular.ttf"],
    &["NotoSansMath-Regular.ttf"],
    &["DejaVuSans.ttf"],
    &["FreeSans.ttf", "FreeSans.otf"],
    &["Symbola.ttf", "Symbola_hint.ttf"],
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_first_font_there_per_group_in_any_case() {
        let dir = std::env::temp_dir().join(format!("robocode-s-gui-fonts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("truetype/noto")).unwrap();
        std::fs::create_dir_all(dir.join("opentype")).unwrap();
        for f in [
            "truetype/noto/NotoSansArabic-Regular.ttf",
            "truetype/noto/notes.txt",
            "opentype/wqy-zenhei.TTC",
            "opentype/DejaVuSans.ttf",
            "DejaVuSans.ttf",
        ] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        let files = index(std::slice::from_ref(&dir));
        assert_eq!(files.len(), 3, "{files:?}");
        let groups: &[&[&str]] = &[
            &["Missing.ttf"],
            &["DejaVuSans.ttf"],
            &[
                "NotoSansCJK-Regular.ttc",
                "wqy-microhei.ttc",
                "wqy-zenhei.ttc",
            ],
            &["NotoSansArabic-Regular.ttf", "NotoNaskhArabic-Regular.ttf"],
        ];
        assert_eq!(
            pick(groups, &files),
            [
                // The shallower one: a folder's own files come before its subfolders'.
                dir.join("DejaVuSans.ttf"),
                dir.join("opentype/wqy-zenhei.TTC"),
                dir.join("truetype/noto/NotoSansArabic-Regular.ttf"),
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn joiners_and_selectors_need_no_glyph() {
        for c in ['\u{200D}', '\u{FE0F}', '\u{E0067}', '\u{FEFF}'] {
            assert!(invisible(c), "{c:?}");
        }
        for c in ['é', '漢', '😀', '\u{1F3FD}'] {
            assert!(!invisible(c), "{c:?}");
        }
    }

    #[test]
    fn the_emoji_font_has_todays_emoji() {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
        out.textures_delta.clear();
        let font = FontId::monospace(12.0);
        // Unicode 6 to 14.
        for c in ['😀', '🚀', '🥲', '🫠'] {
            assert!(ctx.fonts_mut(|f| f.has_glyph(&font, c)), "{c}");
        }
    }
}
