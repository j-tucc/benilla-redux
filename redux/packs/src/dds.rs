//! The DX10 BC7_UNORM DDS files a local texture pack holds: header facts and the exact
//! payload a full chain to 1x1 implies. No image crate: the chain uploads as stored.

use std::io::Read;
use std::path::Path;

pub(crate) const HEADER: usize = 148;
const DXGI_BC7_UNORM: u32 = 98;

/// A pack file's level-0 size and level count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Bc7Shape {
    pub width: u32,
    pub height: u32,
    pub mips: u32,
}

/// Levels of a full chain from `width × height` to 1x1.
pub(crate) fn full_mips(width: u32, height: u32) -> u32 {
    32 - width.max(height).leading_zeros()
}

/// Bytes of a full BC7 chain at `width × height`; each level is `max(1, edge >> i)` in whole
/// 4x4 blocks of 16 bytes.
pub(crate) fn bc7_chain_bytes(width: u32, height: u32) -> usize {
    (0..full_mips(width, height))
        .map(|i| {
            let (w, h) = ((width >> i).max(1), (height >> i).max(1));
            (w.div_ceil(4) * h.div_ceil(4)) as usize * 16
        })
        .sum()
}

/// Parse a DX10 BC7_UNORM 2D full-chain header of whole blocks; `payload` is the file length less
/// the header, checked against the chain the header implies.
pub(crate) fn parse(head: &[u8], payload: usize) -> Result<Bc7Shape, String> {
    if head.len() < HEADER || &head[..4] != b"DDS " {
        return Err("not a DDS".into());
    }
    let u32_at = |at: usize| u32::from_le_bytes(head[at..at + 4].try_into().unwrap());
    if &head[84..88] != b"DX10" {
        return Err("no DX10 header".into());
    }
    let (height, width, mips) = (u32_at(12), u32_at(16), u32_at(28).max(1));
    let (dxgi, dimension, array) = (u32_at(128), u32_at(132), u32_at(140));
    if dxgi != DXGI_BC7_UNORM {
        return Err(format!("DXGI {dxgi}, want BC7_UNORM"));
    }
    if dimension != 3 || array > 1 {
        return Err(format!("dimension {dimension}, array {array}"));
    }
    if width == 0 || height == 0 || !width.is_multiple_of(4) || !height.is_multiple_of(4) {
        return Err(format!("{width}x{height} is not whole blocks"));
    }
    if mips != full_mips(width, height) {
        return Err(format!("{mips} mips at {width}x{height}"));
    }
    let want = bc7_chain_bytes(width, height);
    if payload != want {
        return Err(format!("{payload} payload bytes, want {want}"));
    }
    Ok(Bc7Shape {
        width,
        height,
        mips,
    })
}

/// The header and payload length of the file at `path`, without reading the payload.
pub(crate) fn read_head(path: &Path) -> Result<(Vec<u8>, usize), String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let len = file.metadata().map_err(|e| e.to_string())?.len() as usize;
    let mut head = vec![0u8; HEADER.min(len)];
    file.read_exact(&mut head).map_err(|e| e.to_string())?;
    Ok((head, len.saturating_sub(HEADER)))
}

/// Every file under `dir`, as (path relative to `dir` with `/`, lowercased; full path).
pub(crate) fn files_under(dir: &Path) -> Result<Vec<(String, std::path::PathBuf)>, String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries = std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(dir) {
                let rel = rel
                    .to_string_lossy()
                    .replace('\\', "/")
                    .to_ascii_lowercase();
                out.push((rel, path));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) fn test_header(width: u32, height: u32, mips: u32, dxgi: u32) -> Vec<u8> {
    let mut h = vec![0u8; HEADER];
    h[..4].copy_from_slice(b"DDS ");
    h[12..16].copy_from_slice(&height.to_le_bytes());
    h[16..20].copy_from_slice(&width.to_le_bytes());
    h[28..32].copy_from_slice(&mips.to_le_bytes());
    h[84..88].copy_from_slice(b"DX10");
    h[128..132].copy_from_slice(&dxgi.to_le_bytes());
    h[132..136].copy_from_slice(&3u32.to_le_bytes());
    h[140..144].copy_from_slice(&1u32.to_le_bytes());
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_bc7_chains_parse_square_and_tall() {
        for (w, h, mips) in [(1024, 1024, 11), (2048, 2048, 12), (256, 1024, 11)] {
            let shape = parse(&test_header(w, h, mips, 98), bc7_chain_bytes(w, h)).unwrap();
            assert_eq!(
                shape,
                Bc7Shape {
                    width: w,
                    height: h,
                    mips
                }
            );
        }
        // The remacri 1024² payload and its 256x1024 tree sheet.
        assert_eq!(bc7_chain_bytes(1024, 1024), 1_398_128);
        assert_eq!(bc7_chain_bytes(256, 1024), 349_584);
    }

    #[test]
    fn anything_else_is_rejected() {
        let n = bc7_chain_bytes(1024, 1024);
        assert!(parse(&test_header(1024, 1024, 11, 99), n).is_err(), "sRGB");
        assert!(parse(&test_header(1024, 1024, 10, 98), n).is_err(), "short");
        assert!(parse(&test_header(1024, 1024, 11, 98), n - 16).is_err());
        assert!(parse(&test_header(1022, 1024, 11, 98), n).is_err());
        let mut dx9 = test_header(1024, 1024, 11, 98);
        dx9[84..88].copy_from_slice(b"DXT5");
        assert!(parse(&dx9, n).is_err(), "legacy header");
    }
}
