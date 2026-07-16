/// The canvas model: dimensions, palette, and how pixels map to KV entries.
///
/// Each painted pixel is one replicated key: `px:{x:03}:{y:03}` → palette
/// index as a decimal string. Unpainted pixels are implicit (index 0, white),
/// so an empty cluster is a blank canvas and snapshots only carry paint.
use raft_kv::RaftKv;

pub const WIDTH: u16 = 256;
pub const HEIGHT: u16 = 256;

/// The classic 2017 r/place palette. Index 0 (white) is the background.
pub const PALETTE: [&str; 16] = [
    "#FFFFFF", "#E4E4E4", "#888888", "#222222", "#FFA7D1", "#E50000", "#E59500", "#A06A42",
    "#E5D900", "#94E044", "#02BE01", "#00D3DD", "#0083C7", "#0000EA", "#CF6EE4", "#820080",
];

pub const KEY_PREFIX: &str = "px:";

pub fn pixel_key(x: u16, y: u16) -> String {
    format!("px:{x:03}:{y:03}")
}

/// Parse "px:{x}:{y}" back into coordinates, rejecting out-of-range values.
pub fn parse_pixel_key(key: &str) -> Option<(u16, u16)> {
    let rest = key.strip_prefix(KEY_PREFIX)?;
    let (x, y) = rest.split_once(':')?;
    let (x, y) = (x.parse().ok()?, y.parse().ok()?);
    (x < WIDTH && y < HEIGHT).then_some((x, y))
}

pub fn parse_color(value: &str) -> Option<u8> {
    let c: u8 = value.parse().ok()?;
    ((c as usize) < PALETTE.len()).then_some(c)
}

/// Materialize the full board from this node's applied state:
/// one byte per pixel (palette index), row-major.
pub async fn snapshot(node: &RaftKv) -> Vec<u8> {
    let mut board = vec![0u8; WIDTH as usize * HEIGHT as usize];
    for (key, value) in node.scan_prefix_local(KEY_PREFIX).await {
        if let (Some((x, y)), Some(color)) = (parse_pixel_key(&key), parse_color(&value)) {
            board[y as usize * WIDTH as usize + x as usize] = color;
        }
    }
    board
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_key_roundtrip() {
        for (x, y) in [(0, 0), (255, 255), (7, 128)] {
            assert_eq!(parse_pixel_key(&pixel_key(x, y)), Some((x, y)));
        }
    }

    #[test]
    fn parse_rejects_out_of_range() {
        assert_eq!(parse_pixel_key("px:256:000"), None);
        assert_eq!(parse_pixel_key("px:000:999"), None);
        assert_eq!(parse_pixel_key("not-a-pixel"), None);
        assert_eq!(parse_color("16"), None);
        assert_eq!(parse_color("white"), None);
    }

    #[test]
    fn keys_sort_within_prefix() {
        // Fixed-width coordinates keep every pixel inside the scan prefix
        // in lexicographic order — required for scan_prefix to see them all.
        assert!(pixel_key(2, 0) < pixel_key(10, 0));
        assert!(pixel_key(0, 2) < pixel_key(0, 10));
    }
}
