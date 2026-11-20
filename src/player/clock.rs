use std::time::Instant;

const SLACK: f64 = 0.02;

#[derive(Debug, Clone, Copy)]
struct Observation {
    value: f64,
    first_seen: Instant,
    not_before: Instant,
}

#[derive(Debug, Clone)]
pub struct PlaybackClock {
    anchor_pos: f64,
    anchor_at: Instant,
    rate: f64,
    playing: bool,
    observation: Option<Observation>,
    last_sample_at: Option<Instant>,
    freshness: i32,
    display: f64,
    display_at: Instant,
    pub smoothing: bool,
    pub snap_threshold: f64,
    pub duration: Option<f64>,
}

impl Default for PlaybackClock {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            anchor_pos: 0.0,
            anchor_at: now,
            rate: 1.0,
            playing: false,
            observation: None,
            last_sample_at: None,
            freshness: 0,
            display: 0.0,
            display_at: now,
            smoothing: true,
            snap_threshold: 0.5,
            duration: None,
        }
    }
}

fn secs(later: Instant, earlier: Instant) -> f64 {
    later.saturating_duration_since(earlier).as_secs_f64()
}

impl PlaybackClock {
    pub fn reset(&mut self, position: f64, at: Instant, playing: bool, rate: f64) {
        self.playing = playing;
        self.rate = if rate > 0.0 { rate } else { 1.0 };
        self.observation = None;
        self.last_sample_at = None;
        self.exact(position, at);
        self.display = self.raw(Instant::now());
        self.display_at = Instant::now();
    }

    fn exact(&mut self, position: f64, at: Instant) {
        self.anchor_pos = position;
        self.anchor_at = at;
        self.observation = Some(Observation {
            value: position,
            first_seen: at,
            not_before: at,
        });
        self.last_sample_at = Some(at);
    }

    pub fn set_playing(&mut self, playing: bool, rate: f64, now: Instant) {
        if playing != self.playing || (rate > 0.0 && (rate - self.rate).abs() > f64::EPSILON) {
            let pos = self.raw(now);
            self.anchor_pos = pos;
            self.anchor_at = now;
            self.playing = playing;
            if rate > 0.0 {
                self.rate = rate;
            }
            self.observation = None;
            if !playing {
                self.display = pos;
                self.display_at = now;
            }
        }
    }

    pub fn sample(&mut self, position: f64, at: Instant, forced: bool) {
        if forced || !self.playing {
            self.exact(position, at);
            if forced {
                self.display = self.raw(Instant::now());
                self.display_at = Instant::now();
            }
            return;
        }
        let previous = self.last_sample_at.unwrap_or(at);
        self.last_sample_at = Some(at);
        let gap = secs(at, previous);
        if let Some(prev) = self.observation
            && gap > 0.02
            && gap < 0.35
        {
            let advanced = position - prev.value;
            if advanced.abs() < 1e-6 {
                self.freshness = (self.freshness - 1).max(-4);
            } else if (advanced - gap * self.rate).abs() < 0.04 {
                self.freshness = (self.freshness + 1).min(4);
            }
        }
        if self.freshness >= 2 {
            self.observation = Some(Observation {
                value: position,
                first_seen: at,
                not_before: at,
            });
            if (self.raw(at) - position).abs() > SLACK {
                self.anchor_pos = position;
                self.anchor_at = at;
            }
            return;
        }
        let obs = match self.observation {
            Some(o) if (o.value - position).abs() < 1e-6 => o,
            _ => Observation {
                value: position,
                first_seen: at,
                not_before: previous.min(at),
            },
        };
        self.observation = Some(obs);
        let lo = obs.value + secs(at, obs.first_seen) * self.rate;
        let hi = obs.value + secs(at, obs.not_before) * self.rate;
        let predicted = self.raw(at);
        if predicted < lo - SLACK {
            self.anchor_pos = lo;
            self.anchor_at = at;
        } else if predicted > hi + SLACK {
            self.anchor_pos = hi;
            self.anchor_at = at;
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn raw(&self, now: Instant) -> f64 {
        let elapsed = if self.playing { secs(now, self.anchor_at) * self.rate } else { 0.0 };
        let p = (self.anchor_pos + elapsed).max(0.0);
        match self.duration {
            Some(d) if d > 0.0 => p.min(d),
            _ => p,
        }
    }

    pub fn now(&mut self, now: Instant) -> f64 {
        let raw = self.raw(now);
        if !self.smoothing || !self.playing {
            self.display = raw;
            self.display_at = now;
            return raw;
        }
        let dt = secs(now, self.display_at);
        let predicted = self.display + dt * self.rate;
        let err = raw - predicted;
        self.display = if err.abs() > self.snap_threshold {
            raw
        } else {
            predicted + err * (1.0 - (-dt / 0.3).exp())
        };
        self.display_at = now;
        self.display
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn coarse_player_does_not_pull_clock_back() {
        let t0 = Instant::now();
        let mut c = PlaybackClock::default();
        c.reset(10.0, t0, true, 1.0);
        for step in 1..=20u64 {
            let at = t0 + ms(step * 250);
            let true_pos = 10.0 + (step * 250) as f64 / 1000.0;
            let reported = 10.0 + (true_pos - 10.0).floor();
            c.sample(reported, at, false);
            assert!((c.raw(at) - true_pos).abs() < 0.03, "step {step}: {} vs {true_pos}", c.raw(at));
        }
    }

    #[test]
    fn coarse_player_corrects_a_late_start() {
        let t0 = Instant::now();
        let mut c = PlaybackClock::default();
        let true_start = 10.7;
        c.reset(10.0, t0, true, 1.0);
        for step in 1..=16u64 {
            let at = t0 + ms(step * 100);
            let true_pos = true_start + (step * 100) as f64 / 1000.0;
            let reported = 10.0 + (true_pos - 10.0).floor();
            c.sample(reported, at, false);
        }
        let at = t0 + ms(1600);
        let true_pos = true_start + 1.6;
        assert!((c.raw(at) - true_pos).abs() < 0.12, "{} vs {true_pos}", c.raw(at));
    }

    #[test]
    fn fresh_player_tracks_exactly() {
        let t0 = Instant::now();
        let mut c = PlaybackClock::default();
        c.reset(5.0, t0, true, 1.0);
        let at = t0 + ms(1000);
        c.sample(6.4, at, false);
        assert!((c.raw(at) - 6.4).abs() < 1e-6);
        let at2 = t0 + ms(2000);
        c.sample(7.4, at2, false);
        assert!((c.raw(at2) - 7.4).abs() < 0.03);
    }

    #[test]
    fn fresh_player_is_followed_after_slow_polls() {
        let t0 = Instant::now();
        let mut c = PlaybackClock::default();
        c.reset(100.0, t0, true, 1.0);
        let speed = 1.005;
        for step in 1..=30u64 {
            let at = t0 + ms(step * 100);
            c.sample(100.0 + (step * 100) as f64 / 1000.0 * speed, at, false);
        }
        for step in 1..=10u64 {
            let at = t0 + ms(3000 + step * 1000);
            let truth = 100.0 + (3000 + step * 1000) as f64 / 1000.0 * speed;
            c.sample(truth, at, false);
            assert!((c.raw(at) - truth).abs() <= SLACK + 1e-9, "{} vs {truth}", c.raw(at));
        }
    }

    #[test]
    fn pauses_hold_position() {
        let t0 = Instant::now();
        let mut c = PlaybackClock::default();
        c.reset(5.0, t0, true, 1.0);
        let t1 = t0 + Duration::from_secs(1);
        c.set_playing(false, 1.0, t1);
        assert!((c.raw(t1 + Duration::from_secs(10)) - 6.0).abs() < 1e-6);
    }

    #[test]
    fn forced_samples_reanchor() {
        let t0 = Instant::now();
        let mut c = PlaybackClock::default();
        c.reset(5.0, t0, true, 1.0);
        let at = t0 + ms(500);
        c.sample(42.0, at, true);
        assert!((c.raw(at) - 42.0).abs() < 1e-6);
    }
}
