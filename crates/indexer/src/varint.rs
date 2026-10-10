//! LEB128 variable-length integers. Small numbers (deltas, tfs) take 1 byte.

pub fn write(buf: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        buf.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

/// Read one varint from `buf` starting at `*pos`, advancing `*pos`.
pub fn read(buf: &[u8], pos: &mut usize) -> u64 {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let b = buf[*pos];
        *pos += 1;
        v |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return v;
        }
        shift += 7;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let vals = [0u64, 1, 127, 128, 300, 16_383, 16_384, u32::MAX as u64, u64::MAX];
        let mut buf = Vec::new();
        for v in vals {
            write(&mut buf, v);
        }
        let mut pos = 0;
        for v in vals {
            assert_eq!(read(&buf, &mut pos), v);
        }
        assert_eq!(pos, buf.len());
    }
}
