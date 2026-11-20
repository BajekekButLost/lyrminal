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

    pub fn stamp(&mut self, src: &[u8], sw: usize, sh: usize, sx: i32, sy: i32) {
        for row in 0..sh {
            let ty = sy + row as i32 - self.y;
            if ty < 0 || ty as usize >= self.h {
                continue;
            }
            for col in 0..sw {
                let tx = sx + col as i32 - self.x;
                if tx < 0 || tx as usize >= self.w {
                    continue;
                }
                let v = src[row * sw + col];
                if v == 0 {
                    continue;
                }
                let d = &mut self.data[ty as usize * self.w + tx as usize];
                *d = (*d as u16 + v as u16 - (*d as u16 * v as u16) / 255).min(255) as u8;
            }
        }
    }

    pub fn blurred(&self, radius: f32) -> Mask {
        let r = radius.round() as usize;
        if r == 0 {
            return self.clone();
        }
        let pad = r * 3;
        let w = self.w + pad * 2;
        let h = self.h + pad * 2;
        let mut buf = vec![0f32; w * h];
        for y in 0..self.h {
            for x in 0..self.w {
                buf[(y + pad) * w + x + pad] = self.data[y * self.w + x] as f32;
            }
        }
        let pass = (r as f32 / 1.7).ceil().max(1.0) as usize;
        let mut tmp = vec![0f32; w * h];
        for _ in 0..3 {
            box_h(&buf, &mut tmp, w, h, pass);
            box_v(&tmp, &mut buf, w, h, pass);
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

fn box_h(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize) {
    let div = (2 * r + 1) as f32;
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        let mut acc = 0.0;
        for i in 0..=r.min(w - 1) {
            acc += row[i];
        }
        for x in 0..w {
            dst[y * w + x] = acc / div;
            if x + r + 1 < w {
                acc += row[x + r + 1];
            }
            if x >= r {
                acc -= row[x - r];
            }
        }
    }
}

fn box_v(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize) {
    let div = (2 * r + 1) as f32;
    for x in 0..w {
        let mut acc = 0.0;
        for i in 0..=r.min(h - 1) {
            acc += src[i * w + x];
        }
        for y in 0..h {
            dst[y * w + x] = acc / div;
            if y + r + 1 < h {
                acc += src[(y + r + 1) * w + x];
            }
            if y >= r {
                acc -= src[(y - r) * w + x];
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

    pub fn draw_rgba(&mut self, data: &[u8], w: usize, h: usize, x0: i32, y0: i32, alpha: f32, clip: (i32, i32, i32, i32)) {
        let (cx0, cy0, cx1, cy1) = clip;
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
                let i = (row * w + col) * 4;
                let a = data[i + 3] as f32 / 255.0 * alpha;
                let rgb = [data[i] as f32 / 255.0, data[i + 1] as f32 / 255.0, data[i + 2] as f32 / 255.0];
                self.blend(x as usize, y as usize, rgb, a);
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
    fn blends_over_transparent() {
        let mut c = Canvas::new(2, 1);
        c.draw_alpha(&[255, 128], 2, 1, 0, 0, (0, 0, 2, 1), |_| (Rgb::WHITE, 1.0));
        assert_eq!(&c.px[0..4], &[255, 255, 255, 255]);
        let s = c.straight_rgba();
        assert_eq!(s[4], 255);
        assert!(s[7] > 120 && s[7] < 136);
    }
}
