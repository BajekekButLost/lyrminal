pub mod kitty;

use std::io::Write;
use std::time::{Duration, Instant};

use crate::anim::Rgb;

#[derive(Debug, Clone, Copy, Default)]
pub struct TermCaps {
    pub kitty_graphics: bool,
    pub cell_px: Option<(f32, f32)>,
    pub background: Option<Rgb>,
    pub remote: bool,
}

impl TermCaps {
    pub fn refresh_cell_size(&mut self) {
        self.cell_px = cell_size();
    }
}

pub fn cell_size() -> Option<(f32, f32)> {
    let ws = crossterm::terminal::window_size().ok()?;
    if ws.width == 0 || ws.height == 0 || ws.columns == 0 || ws.rows == 0 {
        return None;
    }
    Some((ws.width as f32 / ws.columns as f32, ws.height as f32 / ws.rows as f32))
}

#[cfg(unix)]
fn read_with_timeout(buf: &mut Vec<u8>, deadline: Instant, done: impl Fn(&[u8]) -> bool) {
    let fd = libc::STDIN_FILENO;
    let mut chunk = [0u8; 512];
    while Instant::now() < deadline && !done(buf) {
        let remaining = deadline.saturating_duration_since(Instant::now()).as_millis() as i32;
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut pfd, 1, remaining.max(1)) };
        if ready <= 0 {
            break;
        }
        let n = unsafe { libc::read(fd, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len()) };
        if n <= 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n as usize]);
    }
}

fn has_da1(buf: &[u8]) -> bool {
    let s = String::from_utf8_lossy(buf);
    if let Some(i) = s.find("\x1b[?") {
        return s[i..].contains('c');
    }
    false
}

fn parse_osc11(s: &str) -> Option<Rgb> {
    let i = s.find("]11;rgb:")?;
    let rest = &s[i + 8..];
    let end = rest.find(['\x1b', '\x07']).unwrap_or(rest.len());
    let parts: Vec<&str> = rest[..end].split('/').collect();
    if parts.len() != 3 {
        return None;
    }
    let comp = |h: &str| -> Option<f32> {
        let v = u32::from_str_radix(h, 16).ok()?;
        let max = (1u32 << (4 * h.len() as u32)) - 1;
        Some(v as f32 / max as f32)
    };
    Some(Rgb::new(comp(parts[0])?, comp(parts[1])?, comp(parts[2])?))
}

#[cfg(unix)]
pub fn probe() -> TermCaps {
    let remote = std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some();
    let multiplexed = std::env::var_os("TMUX").is_some() || std::env::var("TERM").is_ok_and(|t| t.starts_with("screen"));
    let mut caps = TermCaps {
        remote,
        cell_px: cell_size(),
        ..Default::default()
    };
    if crossterm::terminal::enable_raw_mode().is_err() {
        return caps;
    }
    let mut out = std::io::stdout();
    let query = "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b]11;?\x1b\\\x1b[c";
    let _ = out.write_all(query.as_bytes());
    let _ = out.flush();
    let mut buf = Vec::new();
    read_with_timeout(&mut buf, Instant::now() + Duration::from_millis(600), has_da1);
    let _ = crossterm::terminal::disable_raw_mode();
    let s = String::from_utf8_lossy(&buf);
    caps.kitty_graphics = !multiplexed && s.contains("_Gi=31;OK");
    caps.background = parse_osc11(&s);
    caps
}

#[cfg(not(unix))]
pub fn probe() -> TermCaps {
    TermCaps {
        cell_px: cell_size(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_background_reply() {
        let c = parse_osc11("\x1b]11;rgb:1a1a/1b1b/2626\x1b\\").unwrap();
        assert!((c.r - 0x1a1a as f32 / 65535.0).abs() < 1e-4);
        assert!(has_da1(b"\x1b_Gi=31;OK\x1b\\\x1b[?62;22c"));
    }
}
