//! MD5 (RFC 1321), for the FLAC audio signature.

pub struct Md5 {
    state: [u32; 4],
    buf: [u8; 64],
    buf_len: usize,
    total: u64,
}

const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4,
    11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

fn k(i: usize) -> u32 {
    ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32
}

impl Md5 {
    pub fn new() -> Self {
        Md5 { state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476], buf: [0; 64], buf_len: 0, total: 0 }
    }

    fn block(&mut self, b: &[u8]) {
        static K: std::sync::OnceLock<[u32; 64]> = std::sync::OnceLock::new();
        let kt = K.get_or_init(|| std::array::from_fn(k));
        let m: [u32; 16] = std::array::from_fn(|i| u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]));
        let [mut a, mut bb, mut c, mut d] = self.state;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((bb & c) | (!bb & d), i),
                1 => ((d & bb) | (!d & c), (5 * i + 1) % 16),
                2 => (bb ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (bb | !d), (7 * i) % 16),
            };
            let t = d;
            d = c;
            c = bb;
            bb = bb.wrapping_add(a.wrapping_add(f).wrapping_add(kt[i]).wrapping_add(m[g]).rotate_left(S[i]));
            a = t;
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(bb);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total += data.len() as u64;
        if self.buf_len > 0 {
            let take = (64 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let b = self.buf;
                self.block(&b);
                self.buf_len = 0;
            }
        }
        while data.len() >= 64 {
            self.block(&data[..64]);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    pub fn finish(mut self) -> [u8; 16] {
        let bits = self.total.wrapping_mul(8);
        let mut pad = vec![0x80u8];
        while (self.buf_len + pad.len()) % 64 != 56 {
            pad.push(0);
        }
        pad.extend_from_slice(&bits.to_le_bytes());
        let total = self.total;
        self.update(&pad);
        self.total = total;
        let mut out = [0u8; 16];
        for (i, s) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&s.to_le_bytes());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::Md5;

    fn hex(d: [u8; 16]) -> String {
        d.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn known_vectors() {
        assert_eq!(hex(Md5::new().finish()), "d41d8cd98f00b204e9800998ecf8427e");
        let mut m = Md5::new();
        m.update(b"The quick brown fox jumps over the lazy dog");
        assert_eq!(hex(m.finish()), "9e107d9d372bb6826bd81d3542a419d6");
        let mut m = Md5::new();
        for chunk in b"12345678901234567890123456789012345678901234567890123456789012345678901234567890".chunks(7) {
            m.update(chunk);
        }
        assert_eq!(hex(m.finish()), "57edf4a22be3c955ac49da2e2107b67a");
    }
}
