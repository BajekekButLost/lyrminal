use crate::anim::Rgb;

#[derive(Debug, Clone, Default)]
pub struct Mask {
    pub w: usize,
    pub h: usize,
    pub x: i32,
    pub y: i32,
    pub data: Vec<u8>,
}

impl Mask {
    pub fn new(x: i32, y: i32, w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            x,
            y,
            data: vec![0; w * h],
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn stamp_resampled(&mut self, src: &[u8], sw: usize, sh: usize, left: f32, top: f32, k: f32, mut shade: impl FnMut(i32) -> f32) {
        if sw == 0 || sh == 0 || k <= 0.0 {
            return;
        }
        let x0 = (left.floor() as i32).max(self.x);
        let x1 = ((left + sw as f32 * k).ceil() as i32).min(self.x + self.w as i32);
        let y0 = (top.floor() as i32).max(self.y);
        let y1 = ((top + sh as f32 * k).ceil() as i32).min(self.y + self.h as i32);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let inv = 1.0 / k;
        let at = |c: i32, r: i32| -> f32 {
            if c < 0 || r < 0 || c as usize >= sw || r as usize >= sh {
                0.0
            } else {
                src[r as usize * sw + c as usize] as f32
            }
        };
        let cols: Vec<(i32, f32, f32)> = (x0..x1)
            .map(|tx| {
                let u = (tx as f32 + 0.5 - left) * inv - 0.5;
                let uf = u.floor();
                (uf as i32, u - uf, shade(tx).clamp(0.0, 1.0))
            })
            .collect();
        for ty in y0..y1 {
            let v = (ty as f32 + 0.5 - top) * inv - 0.5;
            let vf = v.floor();
            let (r, fy) = (vf as i32, v - vf);
            if r < -1 || r >= sh as i32 {
                continue;
            }
            let row = (ty - self.y) as usize * self.w;
            for (i, &(c, fx, a)) in cols.iter().enumerate() {
                if a <= 0.0 || c < -1 || c >= sw as i32 {
                    continue;
                }
                let top_row = at(c, r) + (at(c + 1, r) - at(c, r)) * fx;
                let bottom_row = at(c, r + 1) + (at(c + 1, r + 1) - at(c, r + 1)) * fx;
                let s = (top_row + (bottom_row - top_row) * fy) / 255.0 * a;
                if s <= 0.0 {
                    continue;
                }
                let d = &mut self.data[row + (x0 - self.x) as usize + i];
                let cur = *d as f32 / 255.0;
                *d = ((s + cur * (1.0 - s)) * 255.0).round().min(255.0) as u8;
            }
        }
    }

    pub fn blurred(&self, sigma: f32) -> Mask {
        if sigma < 0.05 {
            return self.clone();
        }
        let r = box_radius(sigma);
        let pad = (sigma * 3.0).ceil() as usize + 1;
        let w = self.w + pad * 2;
        let h = self.h + pad * 2;
        let mut buf = vec![0f32; w * h];
        for y in 0..self.h {
            for x in 0..self.w {
                buf[(y + pad) * w + x + pad] = self.data[y * self.w + x] as f32;
            }
        }
        let mut tmp = vec![0f32; w * h];
        for _ in 0..3 {
            box_pass(&buf, &mut tmp, h, w, w, 1, r);
            box_pass(&tmp, &mut buf, w, h, 1, w, r);
        }
        Mask {
            w,
            h,
            x: self.x - pad as i32,
            y: self.y - pad as i32,
            data: buf.iter().map(|v| v.round().clamp(0.0, 255.0) as u8).collect(),
        }
    }
}

fn box_radius(sigma: f32) -> f32 {
    let target = sigma * sigma / 3.0;
    let mut p = 0.0f32;
    loop {
        let a = p * (p + 1.0) * (2.0 * p + 1.0) / 3.0;
        let b = (p + 1.0) * (p + 1.0);
        let c = 2.0 * p + 1.0;
        let f = (target * c - a) / (2.0 * (b - target));
        if (0.0..1.0).contains(&f) || p > 64.0 {
            return p + f.clamp(0.0, 1.0);
        }
        p += 1.0;
    }
}

fn box_pass(src: &[f32], dst: &mut [f32], lines: usize, len: usize, line_step: usize, step: usize, r: f32) {
    let p = r.floor() as usize;
    let f = r - p as f32;
    let div = 2.0 * r + 1.0;
    for l in 0..lines {
        let base = l * line_step;
        let at = |i: usize| src[base + i * step];
        let mut acc = 0.0;
        for i in 0..=p.min(len - 1) {
            acc += at(i);
        }
        for x in 0..len {
            let mut edge = 0.0;
            if x > p {
                edge += at(x - p - 1);
            }
            if x + p + 1 < len {
                edge += at(x + p + 1);
            }
            dst[base + x * step] = (acc + f * edge) / div;
            if x + p + 1 < len {
                acc += at(x + p + 1);
            }
            if x >= p {
                acc -= at(x - p);
            }
        }
    }
}

pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

impl Canvas {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            px: vec![0; w * h * 4],
        }
    }

    pub fn resize(&mut self, w: usize, h: usize) {
        if self.w != w || self.h != h {
            self.w = w;
            self.h = h;
            self.px = vec![0; w * h * 4];
        }
    }

    pub fn clear(&mut self) {
        self.px.fill(0);
    }

    pub fn fill_gradient(&mut self, top: Rgb, bottom: Rgb) {
        for y in 0..self.h {
            let t = if self.h > 1 { y as f32 / (self.h - 1) as f32 } else { 0.0 };
            let [r, g, b] = top.mix(bottom, t).to_u8();
            let row = &mut self.px[y * self.w * 4..(y + 1) * self.w * 4];
            for p in row.chunks_exact_mut(4) {
                p[0] = r;
                p[1] = g;
                p[2] = b;
                p[3] = 255;
            }
        }
    }

    #[inline]
    fn blend(&mut self, x: usize, y: usize, rgb: [f32; 3], a: f32) {
        if a <= 0.0 {
            return;
        }
        let a = a.min(1.0);
        let i = (y * self.w + x) * 4;
        let p = &mut self.px[i..i + 4];
        let inv = 1.0 - a;
        p[0] = (rgb[0] * 255.0 * a + p[0] as f32 * inv).round() as u8;
        p[1] = (rgb[1] * 255.0 * a + p[1] as f32 * inv).round() as u8;
        p[2] = (rgb[2] * 255.0 * a + p[2] as f32 * inv).round() as u8;
        p[3] = (255.0 * a + p[3] as f32 * inv).round() as u8;
    }

    pub fn draw_alpha(
        &mut self,
        data: &[u8],
        w: usize,
        h: usize,
        x0: i32,
        y0: i32,
        clip: (i32, i32, i32, i32),
        mut shade: impl FnMut(i32) -> (Rgb, f32),
    ) {
        let (cx0, cy0, cx1, cy1) = clip;
        let mut cols: Vec<([f32; 3], f32)> = Vec::with_capacity(w);
        for col in 0..w {
            let (c, a) = shade(x0 + col as i32);
            cols.push(([c.r, c.g, c.b], a));
        }
        for row in 0..h {
            let y = y0 + row as i32;
            if y < cy0.max(0) || y >= cy1.min(self.h as i32) {
                continue;
            }
            for col in 0..w {
                let x = x0 + col as i32;
                if x < cx0.max(0) || x >= cx1.min(self.w as i32) {
                    continue;
                }
                let m = data[row * w + col];
                if m == 0 {
                    continue;
                }
                let (rgb, a) = cols[col];
                self.blend(x as usize, y as usize, rgb, a * m as f32 / 255.0);
            }
        }
    }

    pub fn disc(&mut self, cx: f32, cy: f32, radius: f32, softness: f32, color: Rgb, alpha: f32, clip: (i32, i32, i32, i32)) {
        if radius <= 0.0 || alpha <= 0.0 {
            return;
        }
        let reach = radius + softness + 1.0;
        let (x0, x1) = ((cx - reach).floor() as i32, (cx + reach).ceil() as i32);
        let (y0, y1) = ((cy - reach).floor() as i32, (cy + reach).ceil() as i32);
        let rgb = [color.r, color.g, color.b];
        for y in y0.max(clip.1).max(0)..y1.min(clip.3).min(self.h as i32) {
            for x in x0.max(clip.0).max(0)..x1.min(clip.2).min(self.w as i32) {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let cover = if softness <= 0.5 {
                    (radius + 0.5 - d).clamp(0.0, 1.0)
                } else {
                    let t = ((d - radius) / softness).clamp(0.0, 1.0);
                    if d < radius { 1.0 } else { (1.0 - t) * (1.0 - t) }
                };
                self.blend(x as usize, y as usize, rgb, alpha * cover);
            }
        }
    }

    pub fn is_opaque(&self) -> bool {
        self.px.chunks_exact(4).all(|p| p[3] == 255)
    }

    pub fn straight_rgba(&self) -> Vec<u8> {
        let mut out = self.px.clone();
        for p in out.chunks_exact_mut(4) {
            let a = p[3];
            if a != 0 && a != 255 {
                let k = 255.0 / a as f32;
                p[0] = (p[0] as f32 * k).min(255.0) as u8;
                p[1] = (p[1] as f32 * k).min(255.0) as u8;
                p[2] = (p[2] as f32 * k).min(255.0) as u8;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_spreads_and_preserves_mass() {
        let mut m = Mask::new(0, 0, 9, 9);
        m.data[4 * 9 + 4] = 255;
        let b = m.blurred(3.0);
        let total: u32 = b.data.iter().map(|v| *v as u32).sum();
        assert!(total > 150 && total < 400);
        assert!(b.data.iter().filter(|v| **v > 0).count() > 9);
    }

    #[test]
    fn blur_widens_continuously() {
        let mut src = Mask::new(0, 0, 81, 81);
        for row in 0..81 {
            src.data[row * 81 + 40] = 255;
        }
        let mut last = 0.0;
        for step in 1..=40 {
            let sigma = step as f32 * 0.1;
            let b = src.blurred(sigma);
            let centre = 40.0 - b.x as f64;
            let (mut var, mut mass) = (0.0, 0.0);
            for (i, v) in b.data.iter().enumerate() {
                let x = (i % b.w) as f64 - centre;
                var += *v as f64 * x * x;
                mass += *v as f64;
            }
            let measured = (var / mass).sqrt();
            assert!(measured > last, "blur stopped growing at sigma {sigma}");
            if sigma >= 1.0 {
                assert!((measured - sigma as f64).abs() < 0.15 * sigma as f64, "sigma {sigma} measured {measured}");
            }
            last = measured;
        }
    }

    #[test]
    fn blends_over_transparent() {
        let mut c = Canvas::new(2, 1);
        c.draw_alpha(&[255, 128], 2, 1, 0, 0, (0, 0, 2, 1), |_| (Rgb::WHITE, 1.0));
        assert_eq!(&c.px[0..4], &[255, 255, 255, 255]);
        let s = c.straight_rgba();
        assert_eq!(s[4], 255);
        assert!(s[7] > 120 && s[7] < 136);
    }
}
