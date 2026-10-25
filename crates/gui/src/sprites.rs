// SPDX-License-Identifier: EPL-2.0

//! Classic Robocode's images (`web/src/lib/assets/classic`, see its README): the tank
//! parts, recoloured as classic does it (a port of `ColorUtil` and `ImageUtil.ColorFilter`
//! in `f32`, like the Java original and the website's `sprites.ts`), the bullet explosion
//! frames and the floor tiles.

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use std::collections::HashMap;

pub type Rgb = [u8; 3];

const BODY: &[u8] = include_bytes!("../../../web/src/lib/assets/classic/body.png");
const TURRET: &[u8] = include_bytes!("../../../web/src/lib/assets/classic/turret.png");
const RADAR: &[u8] = include_bytes!("../../../web/src/lib/assets/classic/radar.png");
const GROUND: [&[u8]; 5] = [
    include_bytes!("../../../web/src/lib/assets/classic/ground/blue_metal_0.png"),
    include_bytes!("../../../web/src/lib/assets/classic/ground/blue_metal_1.png"),
    include_bytes!("../../../web/src/lib/assets/classic/ground/blue_metal_2.png"),
    include_bytes!("../../../web/src/lib/assets/classic/ground/blue_metal_3.png"),
    include_bytes!("../../../web/src/lib/assets/classic/ground/blue_metal_4.png"),
];
macro_rules! explosion {
    ($($n:literal),*) => {
        [$(include_bytes!(concat!("../../../web/src/lib/assets/classic/explosion/explosion1-", $n, ".png")) as &[u8]),*]
    };
}
const EXPLOSION: [&[u8]; 17] = explosion!(
    "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16", "17"
);

/// `ColorUtil.fromRGBtoHSL`.
pub fn rgb_to_hsl([r, g, b]: Rgb) -> [f32; 3] {
    let (r, g, b) = (
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    );
    let min = r.min(g).min(b);
    let max = r.max(g).max(b);
    let delta = max - min;
    let l = (max + min) / 2.0;
    if delta == 0.0 {
        return [0.0, 0.0, l];
    }
    let s = if l < 0.5 {
        delta / (max + min)
    } else {
        delta / ((2.0 - max) - min)
    };
    let dr = (((max - r) / 6.0) + (delta / 2.0)) / delta;
    let dg = (((max - g) / 6.0) + (delta / 2.0)) / delta;
    let db = (((max - b) / 6.0) + (delta / 2.0)) / delta;
    // `Utils.isNear`, in double.
    let near = |a: f32, b: f32| (f64::from(a) - f64::from(b)).abs() < 0.00001;
    let mut h = if near(r, max) {
        db - dg
    } else if near(g, max) {
        ((1.0 / 3.0) + dr) - db
    } else {
        ((2.0 / 3.0) + dg) - dr
    };
    if h < 0.0 {
        h += 1.0;
    }
    if h > 1.0 {
        h -= 1.0;
    }
    [h, s, l]
}

/// `ColorUtil.fromHSLtoRGB`.
pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> Rgb {
    let m2 = if l <= 0.5 {
        l * (s + 1.0)
    } else {
        (l + s) - (l * s)
    };
    let m1 = (2.0 * l) - m2;
    let channel = |x: f32| (255.0 * hue_to_rgb(m1, m2, x)) as u8;
    [
        channel(h + (1.0 / 3.0)),
        channel(h),
        channel(h - (1.0 / 3.0)),
    ]
}

fn hue_to_rgb(m1: f32, m2: f32, mut h: f32) -> f32 {
    if h < 0.0 {
        h += 1.0;
    }
    if h > 1.0 {
        h -= 1.0;
    }
    if h * 6.0 < 1.0 {
        return m1 + ((m2 - m1) * h) * 6.0;
    }
    if h * 2.0 < 1.0 {
        return m2;
    }
    if h * 3.0 < 2.0 {
        return m1 + ((m2 - m1) * ((2.0 / 3.0) - h)) * 6.0;
    }
    m1
}

/// `ImageUtil.ColorFilter.filterRGB`: a coloured pixel takes the robot colour's hue and
/// saturation and a lightness between both; grey pixels (the tracks) stay as they are.
pub fn recolor_pixel(pixel: Rgb, color: Rgb) -> Rgb {
    let [ch, cs, cl] = rgb_to_hsl(color);
    let [_, s, l] = rgb_to_hsl(pixel);
    if s <= 0.0 {
        return pixel;
    }
    let lum = (((cl + l) / 2.0) + (cl / 7.0)).min(1.0);
    hsl_to_rgb(ch, cs, lum)
}

fn decode(png: &[u8]) -> ColorImage {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .expect("bundled image")
        .to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    ColorImage::from_rgba_unmultiplied(size, img.as_raw())
}

fn recolored(base: &ColorImage, color: Rgb) -> ColorImage {
    let mut out = base.clone();
    let mut memo: HashMap<Rgb, Rgb> = HashMap::new();
    for p in &mut out.pixels {
        let [r, g, b, a] = p.to_srgba_unmultiplied();
        if a == 0 {
            continue;
        }
        let c = *memo
            .entry([r, g, b])
            .or_insert_with(|| recolor_pixel([r, g, b], color));
        *p = egui::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], a);
    }
    out
}

/// A tank part: body, gun or radar.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Part {
    Body,
    Gun,
    Radar,
}

/// Textures, recoloured on demand and cached per part and colour.
pub struct Sprites {
    base: HashMap<Part, ColorImage>,
    tanks: HashMap<(Part, Rgb), TextureHandle>,
    pub ground: Vec<TextureHandle>,
    pub explosion: Vec<TextureHandle>,
}

impl Sprites {
    pub fn new(ctx: &egui::Context) -> Self {
        let load =
            |name: &str, png: &[u8]| ctx.load_texture(name, decode(png), TextureOptions::LINEAR);
        Self {
            base: HashMap::from([
                (Part::Body, decode(BODY)),
                (Part::Gun, decode(TURRET)),
                (Part::Radar, decode(RADAR)),
            ]),
            tanks: HashMap::new(),
            ground: GROUND
                .iter()
                .enumerate()
                .map(|(i, png)| load(&format!("ground-{i}"), png))
                .collect(),
            explosion: EXPLOSION
                .iter()
                .enumerate()
                .map(|(i, png)| load(&format!("explosion-{i}"), png))
                .collect(),
        }
    }

    /// The part's texture in `color`, and its size in pixels.
    pub fn tank(
        &mut self,
        ctx: &egui::Context,
        part: Part,
        color: Rgb,
    ) -> (egui::TextureId, egui::Vec2) {
        let base = &self.base[&part];
        let size = egui::vec2(base.size[0] as f32, base.size[1] as f32);
        let texture = self.tanks.entry((part, color)).or_insert_with(|| {
            ctx.load_texture(
                format!("{part:?}-{color:?}"),
                recolored(base, color),
                TextureOptions::LINEAR,
            )
        });
        (texture.id(), size)
    }
}

/// A robot's colours when it set none: derived from its name, as on the website
/// (`tank-colors.ts`), so a robot looks the same everywhere.
pub fn default_colors(name: &str) -> [Rgb; 3] {
    let mut h: u32 = 2166136261;
    for c in name.encode_utf16() {
        h = (h ^ u32::from(c)).wrapping_mul(16777619);
    }
    let hue = h % 360;
    let hue2 = (hue + 120 + ((h >> 9) % 120)) % 360;
    [
        hsl(hue as f64, 0.55, 0.38),
        hsl(hue as f64, 0.35, 0.55),
        hsl(hue2 as f64, 0.7, 0.55),
    ]
}

pub fn hsl(hue: f64, s: f64, l: f64) -> Rgb {
    let a = s * l.min(1.0 - l);
    let channel = |n: f64| {
        let k = (n + hue / 30.0) % 12.0;
        (255.0 * (l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0))).round() as u8
    };
    [channel(0.0), channel(8.0), channel(4.0)]
}

/// An ARGB colour from the protocol as RGB.
pub fn rgb(argb: i32) -> Rgb {
    let v = argb as u32;
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    // The website's reference values, computed with classic's Java code (sprites.spec.ts).
    #[test]
    fn recolors_like_classic() {
        let cases: Vec<[u32; 3]> = serde_json::from_str(include_str!(
            "../../../web/src/lib/components/recolor.expected.json"
        ))
        .unwrap();
        assert!(cases.len() > 100);
        let split = |v: u32| [(v >> 16) as u8, (v >> 8) as u8, v as u8];
        for [color, pixel, expected] in cases {
            assert_eq!(
                recolor_pixel(split(pixel), split(color)),
                split(expected),
                "color {color:06x} pixel {pixel:06x}"
            );
        }
    }

    #[test]
    fn default_colors_are_the_websites() {
        // web/src/lib/components/tank-colors.spec.ts has the same values.
        assert_eq!(
            default_colors("sample.Fire"),
            [[44, 150, 84], [100, 180, 131], [199, 60, 221]]
        );
    }

    #[test]
    fn default_colors_are_stable_per_name() {
        assert_eq!(default_colors("sample.Fire"), default_colors("sample.Fire"));
        assert_ne!(
            default_colors("sample.Fire"),
            default_colors("sample.Walls")
        );
    }
}
