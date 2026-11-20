use std::io::{self, Write};
use std::path::PathBuf;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use flate2::Compression;
use flate2::write::ZlibEncoder;

const CHUNK: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub col: u16,
    pub row: u16,
    pub cols: u16,
    pub rows: u16,
}

pub fn transmit_direct(out: &mut impl Write, id: u32, width: u32, height: u32, rgba: &[u8]) -> io::Result<()> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::fast());
    enc.write_all(rgba)?;
    let compressed = enc.finish()?;
    let payload = STANDARD.encode(compressed);
    let bytes = payload.as_bytes();
    let mut first = true;
    let mut offset = 0;
    while offset < bytes.len() || first {
        let end = (offset + CHUNK).min(bytes.len());
        let more = if end < bytes.len() { 1 } else { 0 };
        let chunk = std::str::from_utf8(&bytes[offset..end]).unwrap_or("");
        if first {
            write!(out, "\x1b_Ga=t,f=32,o=z,s={width},v={height},i={id},q=2,m={more};{chunk}\x1b\\")?;
            first = false;
        } else {
            write!(out, "\x1b_Gm={more},q=2;{chunk}\x1b\\")?;
        }
        offset = end;
        if offset >= bytes.len() {
            break;
        }
    }
    Ok(())
}

fn shm_dir() -> PathBuf {
    let shm = PathBuf::from("/dev/shm");
    if shm.is_dir() { shm } else { std::env::temp_dir() }
}

fn temp_path(seq: u64) -> PathBuf {
    shm_dir().join(format!("lyrminal-tty-graphics-protocol-{}-{seq}.rgba", std::process::id()))
}

pub fn forget_old_temp_files(seq: u64, keep: u64) {
    if seq > keep {
        let _ = std::fs::remove_file(temp_path(seq - keep));
    }
}

pub fn transmit_temp_file(out: &mut impl Write, id: u32, width: u32, height: u32, rgba: &[u8], seq: u64) -> io::Result<()> {
    let path = temp_path(seq);
    std::fs::write(&path, rgba)?;
    let encoded = STANDARD.encode(path.to_string_lossy().as_bytes());
    write!(out, "\x1b_Ga=t,t=t,f=32,s={width},v={height},i={id},q=2;{encoded}\x1b\\")
}

pub fn place(out: &mut impl Write, id: u32, placement_id: u32, p: Placement, z: i32) -> io::Result<()> {
    write!(
        out,
        "\x1b7\x1b[{};{}H\x1b_Ga=p,i={id},p={placement_id},c={},r={},C=1,q=2,z={z}\x1b\\\x1b8",
        p.row + 1,
        p.col + 1,
        p.cols.max(1),
        p.rows.max(1)
    )
}

pub fn delete_image(out: &mut impl Write, id: u32) -> io::Result<()> {
    write!(out, "\x1b_Ga=d,d=I,i={id},q=2\x1b\\")
}

pub fn delete_placements(out: &mut impl Write, id: u32) -> io::Result<()> {
    write!(out, "\x1b_Ga=d,d=i,i={id},q=2\x1b\\")
}

pub fn delete_all(out: &mut impl Write) -> io::Result<()> {
    write!(out, "\x1b_Ga=d,d=A,q=2\x1b\\")
}

pub fn cleanup_temp_files() {
    let prefix = format!("lyrminal-tty-graphics-protocol-{}-", std::process::id());
    if let Ok(entries) = std::fs::read_dir(shm_dir()) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with(&prefix) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_direct_transmission() {
        let mut out = Vec::new();
        let noise: Vec<u8> = (0..64 * 64 * 4).map(|i| ((i * 7919) % 251) as u8).collect();
        transmit_direct(&mut out, 7, 64, 64, &noise).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("\x1b_Ga=t,f=32,o=z,s=64,v=64,i=7,q=2,m="));
        assert!(s.ends_with("\x1b\\"));
        assert!(s.matches("\x1b_G").count() >= 1);
        assert!(s.contains("m=0"));
    }
}
