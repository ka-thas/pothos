//! Lossless JPEG rotation by rewriting only the EXIF Orientation tag.
//! Pixel data is never re-encoded. Browsers, macOS and iOS all honor the tag.

/// Next orientation after a 90 degree turn. Handles mirrored values too.
pub fn next_orientation(cur: u16, cw: bool) -> u16 {
    match (cur, cw) {
        (1, true) => 6, (6, true) => 3, (3, true) => 8, (8, true) => 1,
        (2, true) => 7, (7, true) => 4, (4, true) => 5, (5, true) => 2,
        (1, false) => 8, (8, false) => 3, (3, false) => 6, (6, false) => 1,
        (2, false) => 5, (5, false) => 4, (4, false) => 7, (7, false) => 2,
        (_, true) => 6,
        (_, false) => 8,
    }
}

struct Tiff<'a> {
    d: &'a [u8],
    le: bool,
}

impl<'a> Tiff<'a> {
    fn new(d: &'a [u8]) -> Option<Self> {
        if d.len() < 8 {
            return None;
        }
        let le = match &d[0..4] {
            b"II*\0" => true,
            b"MM\0*" => false,
            _ => return None,
        };
        Some(Tiff { d, le })
    }
    fn u16(&self, o: usize) -> Option<u16> {
        let b = self.d.get(o..o + 2)?;
        Some(if self.le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) })
    }
    fn u32(&self, o: usize) -> Option<u32> {
        let b = self.d.get(o..o + 4)?;
        let a = [b[0], b[1], b[2], b[3]];
        Some(if self.le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) })
    }
    fn ifd0(&self) -> Option<usize> {
        Some(self.u32(4)? as usize)
    }
    /// Returns (entry offset, value) of the Orientation tag in IFD0.
    fn orientation(&self) -> Option<(usize, u16)> {
        let ifd = self.ifd0()?;
        let n = self.u16(ifd)? as usize;
        for i in 0..n {
            let e = ifd + 2 + i * 12;
            if self.u16(e)? == 0x0112 {
                return Some((e, self.u16(e + 8)?));
            }
        }
        None
    }
}

fn p16(le: bool, v: u16) -> [u8; 2] {
    if le { v.to_le_bytes() } else { v.to_be_bytes() }
}
fn p32(le: bool, v: u32) -> [u8; 4] {
    if le { v.to_le_bytes() } else { v.to_be_bytes() }
}

fn minimal_tiff(v: u16) -> Vec<u8> {
    let mut t = b"MM\0*\0\0\0\x08".to_vec();
    t.extend_from_slice(&[0, 1]); // 1 entry
    t.extend_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1]); // tag, SHORT, count 1
    t.extend_from_slice(&v.to_be_bytes());
    t.extend_from_slice(&[0, 0]);
    t.extend_from_slice(&[0, 0, 0, 0]); // no next IFD
    t
}

/// Sets orientation inside an existing TIFF block. If the tag is missing,
/// a copy of IFD0 with the extra entry is appended and the header repointed,
/// so no existing offsets move.
fn set_orientation(mut t: Vec<u8>, v: u16) -> Result<Vec<u8>, String> {
    let tf = Tiff::new(&t).ok_or("bad TIFF header")?;
    let le = tf.le;
    if let Some((e, _)) = tf.orientation() {
        t[e + 2..e + 4].copy_from_slice(&p16(le, 3));
        t[e + 4..e + 8].copy_from_slice(&p32(le, 1));
        t[e + 8..e + 10].copy_from_slice(&p16(le, v));
        t[e + 10..e + 12].copy_from_slice(&[0, 0]);
        return Ok(t);
    }
    let ifd = tf.ifd0().ok_or("no IFD0")?;
    let n = tf.u16(ifd).ok_or("bad IFD0")? as usize;
    let entries_end = ifd + 2 + n * 12;
    let next = tf.u32(entries_end).ok_or("bad IFD0")?;
    let mut entries: Vec<Vec<u8>> = (0..n)
        .map(|i| t[ifd + 2 + i * 12..ifd + 14 + i * 12].to_vec())
        .collect();
    let mut ne = Vec::with_capacity(12);
    ne.extend_from_slice(&p16(le, 0x0112));
    ne.extend_from_slice(&p16(le, 3));
    ne.extend_from_slice(&p32(le, 1));
    ne.extend_from_slice(&p16(le, v));
    ne.extend_from_slice(&[0, 0]);
    entries.push(ne);
    let tag = |e: &Vec<u8>| if le { u16::from_le_bytes([e[0], e[1]]) } else { u16::from_be_bytes([e[0], e[1]]) };
    entries.sort_by_key(|e| tag(e));

    if t.len() % 2 == 1 {
        t.push(0);
    }
    let new_off = t.len() as u32;
    t.extend_from_slice(&p16(le, entries.len() as u16));
    for e in &entries {
        t.extend_from_slice(e);
    }
    t.extend_from_slice(&p32(le, next));
    t[4..8].copy_from_slice(&p32(le, new_off));
    Ok(t)
}

#[allow(dead_code)]
pub fn read_orientation(jpeg: &[u8]) -> u16 {
    find_exif(jpeg)
        .ok()
        .and_then(|(seg, _)| seg)
        .and_then(|(s, e)| Tiff::new(&jpeg[s + 10..e]).and_then(|t| t.orientation()).map(|(_, v)| v))
        .unwrap_or(1)
}

/// Returns (EXIF APP1 range, insert position for a new APP1).
fn find_exif(j: &[u8]) -> Result<(Option<(usize, usize)>, usize), String> {
    if j.len() < 4 || j[0] != 0xFF || j[1] != 0xD8 {
        return Err("not a JPEG".into());
    }
    let mut pos = 2;
    let mut insert_at = 2;
    while pos + 4 <= j.len() {
        if j[pos] != 0xFF {
            return Err("corrupt JPEG marker".into());
        }
        let m = j[pos + 1];
        if m == 0xFF {
            pos += 1;
            continue;
        }
        if m == 0xDA || m == 0xD9 {
            break;
        }
        if (0xD0..=0xD7).contains(&m) || m == 0x01 {
            pos += 2;
            continue;
        }
        let len = u16::from_be_bytes([j[pos + 2], j[pos + 3]]) as usize;
        let end = pos + 2 + len;
        if end > j.len() || len < 2 {
            return Err("truncated JPEG".into());
        }
        if m == 0xE1 && len >= 8 && &j[pos + 4..pos + 10] == b"Exif\0\0" {
            return Ok((Some((pos, end)), insert_at));
        }
        if m == 0xE0 && pos == 2 {
            insert_at = end; // keep JFIF APP0 first
        }
        pos = end;
    }
    Ok((None, insert_at))
}

pub fn rotate(jpeg: &[u8], cw: bool) -> Result<Vec<u8>, String> {
    let (seg, insert_at) = find_exif(jpeg)?;
    let tiff = match seg {
        Some((s, e)) => {
            let old = jpeg[s + 10..e].to_vec();
            let cur = Tiff::new(&old).and_then(|t| t.orientation()).map(|(_, v)| v).unwrap_or(1);
            match set_orientation(old, next_orientation(cur, cw)) {
                Ok(t) => t,
                Err(_) => minimal_tiff(next_orientation(cur, cw)), // unreadable EXIF: replace
            }
        }
        None => minimal_tiff(next_orientation(1, cw)),
    };
    let seg_len = 2 + 6 + tiff.len();
    if seg_len > 0xFFFF {
        return Err("EXIF block too large".into());
    }
    let mut app1 = vec![0xFF, 0xE1];
    app1.extend_from_slice(&(seg_len as u16).to_be_bytes());
    app1.extend_from_slice(b"Exif\0\0");
    app1.extend_from_slice(&tiff);

    let (a, b) = match seg {
        Some((s, e)) => (s, e),
        None => (insert_at, insert_at),
    };
    let mut out = Vec::with_capacity(jpeg.len() + app1.len());
    out.extend_from_slice(&jpeg[..a]);
    out.extend_from_slice(&app1);
    out.extend_from_slice(&jpeg[b..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_jpeg() -> Vec<u8> {
        // SOI, APP0 JFIF, then a fake SOS + EOI (enough for segment parsing)
        let mut j = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 16];
        j.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        j.extend_from_slice(&[0xFF, 0xDA, 0, 2, 0xAB, 0xCD, 0xFF, 0xD9]);
        j
    }

    #[test]
    fn full_cycles() {
        let mut j = tiny_jpeg();
        let mut seen = vec![];
        for _ in 0..4 {
            j = rotate(&j, true).unwrap();
            seen.push(read_orientation(&j));
        }
        assert_eq!(seen, vec![6, 3, 8, 1]);
        for _ in 0..4 {
            j = rotate(&j, false).unwrap();
        }
        assert_eq!(read_orientation(&j), 1);
        assert!(j.ends_with(&[0xFF, 0xDA, 0, 2, 0xAB, 0xCD, 0xFF, 0xD9]));
        assert_eq!(&j[2..4], &[0xFF, 0xE0]); // JFIF still first
    }

    #[test]
    fn adds_tag_to_existing_exif() {
        // Little-endian EXIF with one unrelated tag (0x010F Make, inline "ab")
        let mut t = b"II*\0\x08\0\0\0".to_vec();
        t.extend_from_slice(&[1, 0, 0x0F, 0x01, 2, 0, 3, 0, 0, 0, b'a', b'b', 0, 0, 0, 0, 0, 0]);
        let t2 = set_orientation(t, 6).unwrap();
        let tf = Tiff::new(&t2).unwrap();
        assert_eq!(tf.orientation().unwrap().1, 6);
        let ifd = tf.ifd0().unwrap();
        assert_eq!(tf.u16(ifd).unwrap(), 2);
        assert_eq!(tf.u16(ifd + 2).unwrap(), 0x010F); // sorted, Make kept
    }

    #[test]
    fn mirrored_cycle() {
        let mut o = 2;
        for _ in 0..4 {
            o = next_orientation(o, true);
        }
        assert_eq!(o, 2);
        assert_eq!(next_orientation(next_orientation(5, true), false), 5);
    }
}
