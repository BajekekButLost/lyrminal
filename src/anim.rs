use std::f64::consts::TAU;

#[derive(Debug, Clone, Copy)]
pub struct Spring {
    pub position: f64,
    pub velocity: f64,
    pub goal: f64,
    pub frequency: f64,
    pub damping: f64,
}

impl Spring {
    pub fn new(position: f64, frequency: f64, damping: f64) -> Self {
        Self {
            position,
            velocity: 0.0,
            goal: position,
            frequency,
            damping,
        }
    }

    pub fn set_goal(&mut self, goal: f64) {
        self.goal = goal;
    }

    pub fn snap(&mut self, value: f64) {
        self.position = value;
        self.goal = value;
        self.velocity = 0.0;
    }

    pub fn is_settled(&self) -> bool {
        self.velocity.abs() < 0.01 && (self.position - self.goal).abs() < 1.0 / 3840.0
    }

    pub fn step(&mut self, dt: f64) -> f64 {
        if dt <= 0.0 {
            return self.position;
        }
        if self.is_settled() {
            self.position = self.goal;
            self.velocity = 0.0;
            return self.position;
        }
        let w = self.frequency * TAU;
        let d = self.damping;
        let x0 = self.position - self.goal;
        let v0 = self.velocity;
        let (x, v) = if (d - 1.0).abs() < 1e-6 {
            let b = v0 + w * x0;
            let e = (-w * dt).exp();
            ((x0 + b * dt) * e, (v0 - w * b * dt) * e)
        } else if d < 1.0 {
            let a = d * w;
            let wd = w * (1.0 - d * d).sqrt();
            let b = (v0 + a * x0) / wd;
            let e = (-a * dt).exp();
            let (s, c) = (wd * dt).sin_cos();
            (
                e * (x0 * c + b * s),
                e * (v0 * c + (-a * b - wd * x0) * s),
            )
        } else {
            let r = (d * d - 1.0).sqrt();
            let r1 = -w * (d - r);
            let r2 = -w * (d + r);
            let c2 = (v0 - r1 * x0) / (r2 - r1);
            let c1 = x0 - c2;
            let e1 = (r1 * dt).exp();
            let e2 = (r2 * dt).exp();
            (c1 * e1 + c2 * e2, r1 * c1 * e1 + r2 * c2 * e2)
        };
        self.position = self.goal + x;
        self.velocity = v;
        self.position
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CubicBezier {
    cx: f64,
    bx: f64,
    ax: f64,
    cy: f64,
    by: f64,
    ay: f64,
}

impl CubicBezier {
    pub const fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        let cx = 3.0 * x1;
        let bx = 3.0 * (x2 - x1) - cx;
        let ax = 1.0 - cx - bx;
        let cy = 3.0 * y1;
        let by = 3.0 * (y2 - y1) - cy;
        let ay = 1.0 - cy - by;
        Self {
            cx,
            bx,
            ax,
            cy,
            by,
            ay,
        }
    }

    fn sample_x(&self, t: f64) -> f64 {
        ((self.ax * t + self.bx) * t + self.cx) * t
    }

    fn sample_y(&self, t: f64) -> f64 {
        ((self.ay * t + self.by) * t + self.cy) * t
    }

    fn sample_dx(&self, t: f64) -> f64 {
        (3.0 * self.ax * t + 2.0 * self.bx) * t + self.cx
    }

    fn solve_t(&self, x: f64) -> f64 {
        let mut t = x;
        for _ in 0..8 {
            let err = self.sample_x(t) - x;
            if err.abs() < 1e-6 {
                return t;
            }
            let d = self.sample_dx(t);
            if d.abs() < 1e-6 {
                break;
            }
            t -= err / d;
        }
        let (mut lo, mut hi) = (0.0, 1.0);
        t = x;
        for _ in 0..32 {
            let v = self.sample_x(t);
            if (v - x).abs() < 1e-6 {
                break;
            }
            if x > v {
                lo = t;
            } else {
                hi = t;
            }
            t = (lo + hi) * 0.5;
        }
        t
    }

    pub fn ease(&self, x: f64) -> f64 {
        if x <= 0.0 {
            return 0.0;
        }
        if x >= 1.0 {
            return 1.0;
        }
        self.sample_y(self.solve_t(x))
    }
}

pub const EASE: CubicBezier = CubicBezier::new(0.25, 0.1, 0.25, 1.0);

#[derive(Debug, Clone)]
pub struct Spline {
    xs: Vec<f64>,
    ys: Vec<f64>,
    ks: Vec<f64>,
}

impl Spline {
    pub fn new(points: &[(f64, f64)]) -> Self {
        let xs: Vec<f64> = points.iter().map(|p| p.0).collect();
        let ys: Vec<f64> = points.iter().map(|p| p.1).collect();
        let n = xs.len();
        let mut ks = vec![0.0; n];
        if n >= 3 {
            let mut a = vec![0.0; n];
            let mut b = vec![0.0; n];
            let mut c = vec![0.0; n];
            let mut d = vec![0.0; n];
            b[0] = 2.0 / (xs[1] - xs[0]);
            c[0] = 1.0 / (xs[1] - xs[0]);
            d[0] = 3.0 * (ys[1] - ys[0]) / (xs[1] - xs[0]).powi(2);
            for i in 1..n - 1 {
                let h0 = xs[i] - xs[i - 1];
                let h1 = xs[i + 1] - xs[i];
                a[i] = 1.0 / h0;
                b[i] = 2.0 * (1.0 / h0 + 1.0 / h1);
                c[i] = 1.0 / h1;
                d[i] = 3.0 * ((ys[i] - ys[i - 1]) / (h0 * h0) + (ys[i + 1] - ys[i]) / (h1 * h1));
            }
            let h = xs[n - 1] - xs[n - 2];
            a[n - 1] = 1.0 / h;
            b[n - 1] = 2.0 / h;
            d[n - 1] = 3.0 * (ys[n - 1] - ys[n - 2]) / (h * h);
            for i in 1..n {
                let m = a[i] / b[i - 1];
                b[i] -= m * c[i - 1];
                d[i] -= m * d[i - 1];
            }
            ks[n - 1] = d[n - 1] / b[n - 1];
            for i in (0..n - 1).rev() {
                ks[i] = (d[i] - c[i] * ks[i + 1]) / b[i];
            }
        } else if n == 2 {
            let k = (ys[1] - ys[0]) / (xs[1] - xs[0]);
            ks = vec![k, k];
        }
        Self { xs, ys, ks }
    }

    pub fn at(&self, x: f64) -> f64 {
        let n = self.xs.len();
        if n == 0 {
            return 0.0;
        }
        if x <= self.xs[0] {
            return self.ys[0];
        }
        if x >= self.xs[n - 1] {
            return self.ys[n - 1];
        }
        let mut i = 1;
        while i < n - 1 && self.xs[i] < x {
            i += 1;
        }
        let (x0, x1) = (self.xs[i - 1], self.xs[i]);
        let (y0, y1) = (self.ys[i - 1], self.ys[i]);
        let h = x1 - x0;
        let t = (x - x0) / h;
        let a = self.ks[i - 1] * h - (y1 - y0);
        let b = -self.ks[i] * h + (y1 - y0);
        (1.0 - t) * y0 + t * y1 + t * (1.0 - t) * (a * (1.0 - t) + b * t)
    }
}

pub fn clamp01(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

pub fn ease_sin_out(t: f64) -> f64 {
    (clamp01(t) * std::f64::consts::FRAC_PI_2).sin()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Rgb {
    pub const WHITE: Rgb = Rgb::new(1.0, 1.0, 1.0);
    pub const BLACK: Rgb = Rgb::new(0.0, 0.0, 0.0);

    pub const fn new(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }

    pub fn to_u8(self) -> [u8; 3] {
        [
            (self.r.clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.g.clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.b.clamp(0.0, 1.0) * 255.0).round() as u8,
        ]
    }

    pub fn mix(self, other: Rgb, t: f32) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        Rgb::new(
            self.r + (other.r - self.r) * t,
            self.g + (other.g - self.g) * t,
            self.b + (other.b - self.b) * t,
        )
    }

    pub fn luminance(self) -> f32 {
        0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b
    }

    pub fn to_hsl(self) -> (f32, f32, f32) {
        let max = self.r.max(self.g).max(self.b);
        let min = self.r.min(self.g).min(self.b);
        let l = (max + min) / 2.0;
        if (max - min).abs() < 1e-6 {
            return (0.0, 0.0, l);
        }
        let d = max - min;
        let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
        let h = if max == self.r {
            (self.g - self.b) / d + if self.g < self.b { 6.0 } else { 0.0 }
        } else if max == self.g {
            (self.b - self.r) / d + 2.0
        } else {
            (self.r - self.g) / d + 4.0
        };
        (h / 6.0, s, l)
    }

    pub fn from_hsl(h: f32, s: f32, l: f32) -> Rgb {
        if s <= 0.0 {
            return Rgb::new(l, l, l);
        }
        let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
        let p = 2.0 * l - q;
        let f = |mut t: f32| {
            if t < 0.0 {
                t += 1.0;
            }
            if t > 1.0 {
                t -= 1.0;
            }
            if t < 1.0 / 6.0 {
                p + (q - p) * 6.0 * t
            } else if t < 0.5 {
                q
            } else if t < 2.0 / 3.0 {
                p + (q - p) * (2.0 / 3.0 - t) * 6.0
            } else {
                p
            }
        };
        Rgb::new(f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0))
    }

    pub fn to_color(self) -> ratatui::style::Color {
        let [r, g, b] = self.to_u8();
        ratatui::style::Color::Rgb(r, g, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_converges() {
        let mut s = Spring::new(0.0, 1.0, 0.5);
        s.set_goal(1.0);
        for _ in 0..600 {
            s.step(1.0 / 60.0);
        }
        assert!((s.position - 1.0).abs() < 1e-3);
        let mut c = Spring::new(0.0, 1.0, 1.0);
        c.set_goal(1.0);
        for _ in 0..600 {
            c.step(1.0 / 60.0);
        }
        assert!((c.position - 1.0).abs() < 1e-3);
        let mut o = Spring::new(0.0, 1.0, 1.5);
        o.set_goal(1.0);
        for _ in 0..1200 {
            o.step(1.0 / 60.0);
        }
        assert!((o.position - 1.0).abs() < 1e-3);
    }

    #[test]
    fn spline_passes_through_points() {
        let s = Spline::new(&[(0.0, 0.95), (0.7, 1.05), (1.0, 1.0)]);
        assert!((s.at(0.0) - 0.95).abs() < 1e-9);
        assert!((s.at(0.7) - 1.05).abs() < 1e-9);
        assert!((s.at(1.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn bezier_endpoints() {
        let b = CubicBezier::new(0.4, 0.1, 0.0, 1.0);
        assert_eq!(b.ease(0.0), 0.0);
        assert_eq!(b.ease(1.0), 1.0);
        assert!(b.ease(0.5) > 0.5);
    }
}
