use std::sync::Arc;

use anyhow::{Context, Result, bail};
use image::{RgbaImage, imageops::FilterType};

use crate::anim::Rgb;
use crate::config::{AppearanceConfig, Background};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub dominant: Rgb,
    pub accent: Rgb,
}

#[derive(Debug, Clone)]
pub struct Artwork {
    pub url: String,
    pub image: Arc<RgbaImage>,
    pub palette: Palette,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub base: Rgb,
    pub paint: Background,
    pub bg_top: Rgb,
    pub bg_bottom: Rgb,
    pub text: Rgb,
    pub accent: Rgb,
    pub muted: Rgb,
}

impl Theme {
    pub fn plain(terminal_bg: Option<Rgb>) -> Self {
        let base = terminal_bg.unwrap_or(Rgb::BLACK);
        let light = base.luminance() > 0.5;
        let text = if light { Rgb::new(0.08, 0.08, 0.1) } else { Rgb::WHITE };
        Self {
            base,
            paint: Background::None,
            bg_top: base,
            bg_bottom: base,
            text,
            accent: text,
            muted: base.mix(text, 0.55),
        }
    }

    pub fn build(cfg: &AppearanceConfig, palette: Option<Palette>, terminal_bg: Option<Rgb>) -> Self {
        let plain = Self::plain(terminal_bg);
        let Some(p) = palette.filter(|_| cfg.album_theming) else {
            return plain;
        };
        let (h, s, _) = p.dominant.to_hsl();
        let (ah, as_, al) = p.accent.to_hsl();
        let bg_top = Rgb::from_hsl(ah, (as_ * 0.75).min(0.6), 0.17);
        let bg_bottom = Rgb::from_hsl(h, (s * 0.7).min(0.55), 0.08);
        let accent = Rgb::from_hsl(ah, as_.max(0.45).min(0.9), al.clamp(0.62, 0.78));
        let text = Rgb::WHITE.mix(accent, 0.06);
        match cfg.background {
            Background::None => Self {
                accent,
                text: plain.text.mix(accent, 0.06),
                ..plain
            },
            mode => {
                let solid = bg_top.mix(bg_bottom, 0.5);
                let (top, bottom) = if mode == Background::Solid { (solid, solid) } else { (bg_top, bg_bottom) };
                Self {
                    base: top.mix(bottom, 0.5),
                    paint: mode,
                    bg_top: top,
                    bg_bottom: bottom,
                    text,
                    accent,
                    muted: solid.mix(text, 0.55),
                }
            }
        }
    }

    pub fn bg_at(&self, y: u16, height: u16) -> Rgb {
        if height <= 1 {
            return self.bg_top;
        }
        let t = y as f32 / (height - 1) as f32;
        self.bg_top.mix(self.bg_bottom, t)
    }

    pub fn ink(&self, alpha: f32, bg: Rgb) -> Rgb {
        bg.mix(self.text, alpha)
    }
}

pub fn extract_palette(img: &RgbaImage) -> Palette {
    let small = image::imageops::resize(img, 48, 48, FilterType::Triangle);
    let pixels: Vec<[f32; 3]> = small
        .pixels()
        .filter(|p| p[3] > 128)
        .map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0])
        .collect();
    if pixels.is_empty() {
        return Palette {
            dominant: Rgb::new(0.2, 0.2, 0.25),
            accent: Rgb::new(0.6, 0.6, 0.7),
        };
    }
    let k = 6.min(pixels.len());
    let mut centers: Vec<[f32; 3]> = (0..k).map(|i| pixels[i * pixels.len() / k]).collect();
    let mut counts = vec![0usize; k];
    for _ in 0..10 {
        let mut sums = vec![[0f32; 3]; k];
        counts.iter_mut().for_each(|c| *c = 0);
        for p in &pixels {
            let (best, _) = centers
                .iter()
                .enumerate()
                .map(|(i, c)| (i, (c[0] - p[0]).powi(2) + (c[1] - p[1]).powi(2) + (c[2] - p[2]).powi(2)))
                .fold((0, f32::MAX), |a, b| if b.1 < a.1 { b } else { a });
            counts[best] += 1;
            for ch in 0..3 {
                sums[best][ch] += p[ch];
            }
        }
        for i in 0..k {
            if counts[i] > 0 {
                for ch in 0..3 {
                    centers[i][ch] = sums[i][ch] / counts[i] as f32;
                }
            }
        }
    }
    let total = pixels.len() as f32;
    let colors: Vec<(Rgb, f32)> = centers
        .iter()
        .zip(counts.iter())
        .map(|(c, n)| (Rgb::new(c[0], c[1], c[2]), *n as f32 / total))
        .collect();
    let dominant = colors
        .iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|c| c.0)
        .unwrap_or(Rgb::BLACK);
    let accent = colors
        .iter()
        .map(|(c, share)| {
            let (_, s, l) = c.to_hsl();
            let light_score = 1.0 - ((l - 0.55).abs() * 2.0).min(1.0);
            (c, s * 0.7 + light_score * 0.3 + share.sqrt() * 0.4)
        })
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|c| *c.0)
        .unwrap_or(dominant);
    Palette { dominant, accent }
}

pub async fn load_artwork(client: &reqwest::Client, url: &str) -> Result<Artwork> {
    let bytes: Vec<u8> = if let Some(path) = url.strip_prefix("file://") {
        let path = percent_decode(path);
        tokio::fs::read(&path).await.with_context(|| format!("reading {path}"))?
    } else if url.starts_with("http://") || url.starts_with("https://") {
        let resp = client.get(url).send().await?;
        if !resp.status().is_success() {
            bail!("artwork request failed: {}", resp.status());
        }
        resp.bytes().await?.to_vec()
    } else {
        bail!("unsupported artwork url");
    };
    let image = tokio::task::spawn_blocking(move || -> Result<RgbaImage> {
        let img = image::load_from_memory(&bytes)?;
        let img = img.resize(512, 512, FilterType::Triangle);
        Ok(img.to_rgba8())
    })
    .await??;
    let palette = extract_palette(&image);
    Ok(Artwork {
        url: url.to_string(),
        image: Arc::new(image),
        palette,
    })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_finds_vibrant_accent() {
        let mut img = RgbaImage::new(10, 10);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = if x < 7 {
                image::Rgba([20, 20, 30, 255])
            } else {
                image::Rgba([230, 40, 60, 255])
            };
        }
        let pal = extract_palette(&img);
        assert!(pal.accent.r > 0.6 && pal.accent.g < 0.4);
        assert!(pal.dominant.luminance() < 0.2);
    }

    #[test]
    fn decodes_file_urls() {
        assert_eq!(percent_decode("/tmp/a%20b.png"), "/tmp/a b.png");
    }
}
