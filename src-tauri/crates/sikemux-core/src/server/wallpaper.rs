//! A theme worked out from a wallpaper's pixels. Each client reads its own
//! screen's wallpaper and samples it, since the wallpaper is the viewer's;
//! the core picks the palette from the samples, as it works a theme out of
//! Ghostty's colours.

use base64::Engine;
use serde::Deserialize;

/// Pixels sampled at most, spread evenly over the image.
const SAMPLES: usize = 24_000;
/// How many times the pixels are halved into boxes of like colours.
const CUTS: u32 = 5;
const MAX_PIXEL_BYTES: usize = 4 * 1024 * 1024;
/// The hues of red, yellow, green, cyan, blue and magenta.
const ANSI_HUES: [f64; 6] = [29.0, 100.0, 142.0, 195.0, 262.0, 328.0];

#[derive(Deserialize)]
pub(crate) struct PixelsParams {
    pub name: String,
    /// The wallpaper's pixels as RGBA bytes, in base64.
    pub pixels: String,
}

#[derive(Clone, Copy)]
struct Lch {
    l: f64,
    c: f64,
    h: f64,
}

#[derive(Clone, Copy)]
struct Cluster {
    lch: Lch,
    count: usize,
}

fn to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn from_linear(c: f64) -> f64 {
    if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn rgb_to_lch(r: f64, g: f64, b: f64) -> Lch {
    let (lr, lg, lb) = (
        to_linear(r / 255.0),
        to_linear(g / 255.0),
        to_linear(b / 255.0),
    );
    let l = (0.412_221_470_8 * lr + 0.536_332_536_3 * lg + 0.051_445_992_9 * lb).cbrt();
    let m = (0.211_903_498_2 * lr + 0.680_699_545_1 * lg + 0.107_396_956_6 * lb).cbrt();
    let s = (0.088_302_461_9 * lr + 0.281_718_837_6 * lg + 0.629_978_700_5 * lb).cbrt();
    let lightness = 0.210_454_255_3 * l + 0.793_617_785 * m - 0.004_072_046_8 * s;
    let a = 1.977_998_495_1 * l - 2.428_592_205 * m + 0.450_593_709_9 * s;
    let bb = 0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766 * s;
    Lch {
        l: lightness,
        c: a.hypot(bb),
        h: (bb.atan2(a).to_degrees() + 360.0) % 360.0,
    }
}

fn lch_to_linear(Lch { l, c, h }: Lch) -> [f64; 3] {
    let (a, b) = (c * h.to_radians().cos(), c * h.to_radians().sin());
    let l_ = (l + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
    let m_ = (l - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
    let s_ = (l - 0.089_484_177_5 * a - 1.291_485_548 * b).powi(3);
    [
        4.076_741_662_1 * l_ - 3.307_711_591_3 * m_ + 0.230_969_929_2 * s_,
        -1.268_438_004_6 * l_ + 2.609_757_401_1 * m_ - 0.341_319_396_5 * s_,
        -0.004_196_086_3 * l_ - 0.703_418_614_7 * m_ + 1.707_614_701 * s_,
    ]
}

/// `#rrggbb` for an OKLCH colour, giving up chroma until it fits in sRGB.
fn hex(color: Lch) -> String {
    let mut c = color.c;
    let mut rgb = lch_to_linear(Lch { c, ..color });
    while c > 0.0 && rgb.iter().any(|v| *v < -0.0001 || *v > 1.0001) {
        c = (c - 0.005).max(0.0);
        rgb = lch_to_linear(Lch { c, ..color });
    }
    let channel = |v: f64| (from_linear(v.clamp(0.0, 1.0)) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(rgb[0]),
        channel(rgb[1]),
        channel(rgb[2])
    )
}

fn hue_distance(a: f64, b: f64) -> f64 {
    ((((a - b) % 360.0) + 540.0) % 360.0 - 180.0).abs()
}

/// Splits the pixels along their widest channel until there are `depth`
/// levels of boxes, as Aether's median cut does.
fn median_cut(mut pixels: Vec<[u8; 3]>, depth: u32) -> Vec<Vec<[u8; 3]>> {
    if depth == 0 || pixels.len() < 2 {
        return vec![pixels];
    }
    let span = |channel: usize| {
        let values = pixels.iter().map(|pixel| pixel[channel]);
        values.clone().max().unwrap_or(0) - values.min().unwrap_or(0)
    };
    let spans = [span(0), span(1), span(2)];
    let widest = spans.iter().copied().max().unwrap_or(0);
    let channel = spans.iter().position(|span| *span == widest).unwrap_or(0);
    pixels.sort_by_key(|pixel| pixel[channel]);
    let upper = pixels.split_off(pixels.len() / 2);
    let mut boxes = median_cut(pixels, depth - 1);
    boxes.extend(median_cut(upper, depth - 1));
    boxes
}

fn clusters(rgba: &[u8]) -> Result<Vec<Cluster>, String> {
    let total = rgba.len() / 4;
    let stride = (total / SAMPLES).max(1);
    let pixels: Vec<[u8; 3]> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .step_by(stride)
        .filter(|pixel| pixel[3] >= 128)
        .map(|pixel| [pixel[0], pixel[1], pixel[2]])
        .collect();
    if pixels.is_empty() {
        return Err("the wallpaper has no visible pixels".into());
    }
    Ok(median_cut(pixels, CUTS)
        .into_iter()
        .filter(|cut| !cut.is_empty())
        .map(|cut| {
            let mean = |channel: usize| {
                cut.iter()
                    .map(|pixel| f64::from(pixel[channel]))
                    .sum::<f64>()
                    / cut.len() as f64
            };
            Cluster {
                lch: rgb_to_lch(mean(0), mean(1), mean(2)),
                count: cut.len(),
            }
        })
        .collect())
}

/// The first of `clusters` that `better` puts above all the rest.
fn first_best(clusters: &[Cluster], key: impl Fn(&Cluster) -> f64) -> Option<Cluster> {
    clusters
        .iter()
        .copied()
        .fold(None, |best: Option<Cluster>, next| match best {
            Some(best) if key(&best) >= key(&next) => Some(best),
            _ => Some(next),
        })
}

/// A terminal palette picked from a wallpaper's RGBA pixels: background,
/// foreground, cursor, cursor text, selection and the 16 ANSI colours, each
/// `#rrggbb`, with an accent.
pub(crate) fn colours(rgba: &[u8]) -> Result<(Vec<String>, String), String> {
    let found = clusters(rgba)?;
    let population: usize = found.iter().map(|cluster| cluster.count).sum();
    let brightness = found
        .iter()
        .map(|cluster| cluster.lch.l * cluster.count as f64)
        .sum::<f64>()
        / population as f64;
    let dark = brightness < 0.62;
    let vivid: Vec<Cluster> = found
        .iter()
        .copied()
        .filter(|cluster| cluster.lch.c >= 0.04 && cluster.lch.l > 0.2 && cluster.lch.l < 0.95)
        .collect();
    let lead = first_best(&vivid, |cluster| cluster.count as f64 * cluster.lch.c);
    let sizeable: Vec<Cluster> = found
        .iter()
        .copied()
        .filter(|cluster| cluster.count as f64 >= population as f64 * 0.05)
        .collect();
    let pool = if sizeable.is_empty() {
        &found
    } else {
        &sizeable
    };
    let ground = first_best(
        pool,
        |cluster| if dark { -cluster.lch.l } else { cluster.lch.l },
    )
    .ok_or("the wallpaper has no visible pixels")?;
    let tint_h = if ground.lch.c >= 0.015 {
        ground.lch.h
    } else {
        lead.map_or(270.0, |lead| lead.lch.h)
    };
    let tint_c = ground.lch.c.min(0.035);
    let typical = {
        let mut chromas: Vec<f64> = vivid.iter().map(|cluster| cluster.lch.c).collect();
        chromas.sort_by(f64::total_cmp);
        chromas.get(chromas.len() / 2).copied().unwrap_or(0.1)
    };
    let chroma = typical.clamp(0.08, 0.17);

    let neutral = |l: f64, c: f64| hex(Lch { l, c, h: tint_h });
    let background = if dark {
        neutral(ground.lch.l.clamp(0.14, 0.22), tint_c)
    } else {
        neutral(ground.lch.l.clamp(0.95, 0.985), tint_c.min(0.015))
    };
    let foreground = if dark {
        neutral(0.91, tint_c.min(0.02))
    } else {
        neutral(0.3, tint_c)
    };
    let accent_hue = lead.map_or(tint_h, |lead| lead.lch.h);
    let accent = hex(Lch {
        l: if dark { 0.76 } else { 0.54 },
        c: lead.map_or(0.08, |lead| lead.lch.c.clamp(0.1, 0.2)),
        h: accent_hue,
    });
    let hues: Vec<(f64, f64)> = ANSI_HUES
        .iter()
        .map(|target| {
            let near: Vec<Cluster> = vivid
                .iter()
                .copied()
                .filter(|cluster| hue_distance(cluster.lch.h, *target) <= 32.0)
                .collect();
            match first_best(&near, |cluster| cluster.count as f64) {
                Some(found) => (found.lch.h, found.lch.c.clamp(0.09, 0.2)),
                None => (*target, chroma),
            }
        })
        .collect();
    let normal = hues.iter().map(|(h, c)| {
        hex(Lch {
            l: if dark { 0.72 } else { 0.54 },
            c: *c,
            h: *h,
        })
    });
    let bright = hues.iter().map(|(h, c)| {
        hex(Lch {
            l: if dark { 0.8 } else { 0.47 },
            c: (c * 1.1).min(0.22),
            h: *h,
        })
    });
    let selection = hex(Lch {
        l: if dark { 0.34 } else { 0.87 },
        c: chroma.min(0.08),
        h: accent_hue,
    });
    let mut palette = vec![
        background.clone(),
        foreground,
        accent.clone(),
        background,
        selection,
        neutral(if dark { 0.3 } else { 0.32 }, tint_c),
    ];
    palette.extend(normal);
    palette.push(neutral(if dark { 0.82 } else { 0.86 }, tint_c));
    palette.push(neutral(if dark { 0.52 } else { 0.58 }, tint_c));
    palette.extend(bright);
    palette.push(neutral(if dark { 0.96 } else { 0.95 }, tint_c));
    Ok((palette, accent))
}

/// The RGBA bytes a client sent, checked before anything reads them.
pub(crate) fn pixels(params: &PixelsParams) -> Result<Vec<u8>, String> {
    if params.pixels.len() > MAX_PIXEL_BYTES / 3 * 4 + 4 {
        return Err("the wallpaper's pixels are more than the core reads".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&params.pixels)
        .map_err(|_| "the wallpaper's pixels are not base64".to_owned())?;
    if bytes.len() % 4 != 0 {
        return Err("the wallpaper's pixels are not RGBA".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paint(size: usize, pick: impl Fn(usize, usize) -> [u8; 3]) -> Vec<u8> {
        let mut pixels = Vec::with_capacity(size * size * 4);
        for y in 0..size {
            for x in 0..size {
                pixels.extend(pick(x, y));
                pixels.push(255);
            }
        }
        pixels
    }

    fn channels(colour: &str) -> [u8; 3] {
        let at = |from: usize| u8::from_str_radix(&colour[from..from + 2], 16).unwrap();
        [at(1), at(3), at(5)]
    }

    fn lightness(colour: &str) -> f64 {
        channels(colour)
            .iter()
            .map(|channel| f64::from(*channel))
            .sum::<f64>()
            / 765.0
    }

    #[test]
    fn takes_a_dark_ground_and_its_loudest_colour_from_a_night_wallpaper() {
        let (colours, accent) = colours(&paint(64, |x, y| {
            if x > 40 && y > 40 {
                [255, 60, 170]
            } else {
                [8, 10, 20]
            }
        }))
        .unwrap();
        assert!(lightness(&colours[0]) < 0.25);
        assert!(lightness(&colours[1]) > 0.8);
        let [r, g, b] = channels(&accent);
        assert!(r > g);
        assert!(b > g);
    }

    #[test]
    fn turns_a_pale_wallpaper_into_a_light_ground_with_dark_text() {
        let (colours, _) = colours(&paint(64, |x, _| {
            if x < 8 {
                [40, 120, 200]
            } else {
                [238, 232, 220]
            }
        }))
        .unwrap();
        assert!(lightness(&colours[0]) > 0.85);
        assert!(lightness(&colours[1]) < 0.4);
    }

    #[test]
    fn still_builds_a_full_palette_from_a_wallpaper_with_no_colour_in_it() {
        let (colours, _) = colours(&paint(32, |x, _| {
            let grey = (x * 4) as u8;
            [grey, grey, grey]
        }))
        .unwrap();
        assert_eq!(colours.len(), 21);
        assert!(colours.iter().all(|colour| colour.len() == 7
            && colour.starts_with('#')
            && colour[1..].chars().all(|char| char.is_ascii_hexdigit())));
    }
}
