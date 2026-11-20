use std::time::Instant;

#[derive(Debug, Clone)]
pub struct PlaybackClock {
    anchor_pos: f64,
    anchor_at: Instant,
    rate: f64,
    playing: bool,
    last_sample: Option<f64>,
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
            last_sample: None,
            display: 0.0,
            display_at: now,
            smoothing: true,
            snap_threshold: 0.5,
            duration: None,
        }
    }
}

impl PlaybackClock {
    pub fn reset(&mut self, position: f64, at: Instant, playing: bool, rate: f64) {
        self.anchor_pos = position;
        self.anchor_at = at;
        self.playing = playing;
        self.rate = if rate > 0.0 { rate } else { 1.0 };
        self.last_sample = Some(position);
        self.display = self.raw(Instant::now());
        self.display_at = Instant::now();
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
            if !playing {
                self.display = pos;
                self.display_at = now;
            }
        }
    }

    pub fn sample(&mut self, position: f64, at: Instant, forced: bool) {
        let stale = !forced
            && self.playing
            && self
                .last_sample
                .is_some_and(|last| (last - position).abs() < 1e-6);
        self.last_sample = Some(position);
        if stale {
            return;
        }
        self.anchor_pos = position;
        self.anchor_at = at;
        if forced {
            self.display = self.raw(Instant::now());
            self.display_at = Instant::now();
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn raw(&self, now: Instant) -> f64 {
        let elapsed = if self.playing {
            now.saturating_duration_since(self.anchor_at).as_secs_f64() * self.rate
        } else {
            0.0
        };
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
        let dt = now.saturating_duration_since(self.display_at).as_secs_f64();
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

    #[test]
    fn extrapolates_and_ignores_stale_samples() {
        let t0 = Instant::now();
        let mut c = PlaybackClock::default();
        c.reset(10.0, t0, true, 1.0);
        let t1 = t0 + Duration::from_millis(500);
        assert!((c.raw(t1) - 10.5).abs() < 1e-6);
        c.sample(10.0, t1, false);
        assert!((c.raw(t1) - 10.5).abs() < 1e-6);
        c.sample(10.6, t1, false);
        assert!((c.raw(t1) - 10.6).abs() < 1e-6);
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
}
