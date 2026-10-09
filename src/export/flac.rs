//! FLAC encoder: fixed and LPC prediction, partitioned Rice residuals, stereo
//! decorrelation, MD5 signature and Vorbis-comment metadata. Pure Rust and
//! within the FLAC "subset", so every decoder plays the result.

use std::io::{Seek, SeekFrom, Write};

use super::md5::Md5;
use super::{Progress, Quantizer};

const BLOCK: usize = 4096;
const MAX_LPC: usize = 12;
const MAX_PARTITION: u32 = 8;

// ------------------------------------------------------------------ bit writer

struct Bits {
    buf: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn new() -> Self {
        Bits { buf: Vec::with_capacity(BLOCK * 8), acc: 0, n: 0 }
    }
    #[inline]
    fn put(&mut self, v: u64, bits: u32) {
        debug_assert!(bits <= 32);
        if bits == 0 {
            return;
        }
        self.acc = (self.acc << bits) | (v & ((1u64 << bits) - 1));
        self.n += bits;
        while self.n >= 8 {
            self.n -= 8;
            self.buf.push((self.acc >> self.n) as u8);
        }
    }
    #[inline]
    fn put_signed(&mut self, v: i64, bits: u32) {
        self.put(v as u64, bits);
    }
    #[inline]
    fn unary(&mut self, mut q: u64) {
        while q >= 32 {
            self.put(0, 32);
            q -= 32;
        }
        self.put(1, q as u32 + 1);
    }
    fn align(&mut self) {
        if self.n > 0 {
            self.put(0, 8 - self.n);
        }
    }
}

fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
        }
    }
    crc
}

fn crc16(data: &[u8]) -> u16 {
    static TABLE: std::sync::OnceLock<[u16; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u16; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = (i as u16) << 8;
            for _ in 0..8 {
                c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 };
            }
            *e = c;
        }
        t
    });
    let mut crc = 0u16;
    for &b in data {
        crc = (crc << 8) ^ t[((crc >> 8) as u8 ^ b) as usize];
    }
    crc
}

// ------------------------------------------------------------------ residuals

#[inline]
fn zigzag(r: i64) -> u64 {
    ((r << 1) ^ (r >> 63)) as u64
}

/// Rice parameters chosen for one residual signal.
struct RiceChoice {
    order: u32,
    params: Vec<u32>,
    bits: u64,
}

/// Choose the partition order and per-partition Rice parameters.
fn choose_rice(res: &[i64], block: usize, pred_order: usize) -> RiceChoice {
    // Finest usable partition order.
    let mut max_p = 0;
    while max_p < MAX_PARTITION && block % (1 << (max_p + 1)) == 0 && (block >> (max_p + 1)) > pred_order {
        max_p += 1;
    }
    let parts = 1usize << max_p;
    let psize = block >> max_p;
    let mut sums = vec![0u64; parts];
    let mut counts = vec![0u64; parts];
    let mut idx = 0;
    for p in 0..parts {
        let n = if p == 0 { psize - pred_order } else { psize };
        let mut s = 0u64;
        for r in &res[idx..idx + n] {
            s += zigzag(*r);
        }
        sums[p] = s;
        counts[p] = n as u64;
        idx += n;
    }
    let mut best = RiceChoice { order: 0, params: Vec::new(), bits: u64::MAX };
    let mut order = max_p;
    loop {
        let mut params = Vec::with_capacity(sums.len());
        let mut bits = 0u64;
        let mut wide = false;
        for (s, n) in sums.iter().zip(&counts) {
            let (k, b) = best_param(*s, *n);
            wide |= k > 14;
            params.push(k);
            bits += b;
        }
        bits += params.len() as u64 * if wide { 5 } else { 4 };
        if bits < best.bits {
            best = RiceChoice { order, params, bits };
        }
        if order == 0 {
            break;
        }
        order -= 1;
        sums = sums.chunks(2).map(|c| c[0] + c[1]).collect();
        counts = counts.chunks(2).map(|c| c[0] + c[1]).collect();
    }
    best.bits += 2 + 4; // coding method + partition order
    best
}

/// Best Rice parameter for a partition with `n` values summing to `sum`.
fn best_param(sum: u64, n: u64) -> (u32, u64) {
    if n == 0 {
        return (0, 0);
    }
    let cost = |k: u32| n * (k as u64 + 1) + (sum >> k);
    let mean = sum / n;
    let mut k = if mean == 0 { 0 } else { 64 - mean.leading_zeros() };
    k = k.min(30);
    let mut best = (k, cost(k));
    for kk in [k.saturating_sub(1), (k + 1).min(30)] {
        let c = cost(kk);
        if c < best.1 {
            best = (kk, c);
        }
    }
    best
}

fn write_residual(w: &mut Bits, res: &[i64], block: usize, pred_order: usize, rc: &RiceChoice) {
    let wide = rc.params.iter().any(|k| *k > 14);
    w.put(if wide { 1 } else { 0 }, 2);
    w.put(rc.order as u64, 4);
    let psize = block >> rc.order;
    let mut idx = 0;
    for (p, &k) in rc.params.iter().enumerate() {
        w.put(k as u64, if wide { 5 } else { 4 });
        let n = if p == 0 { psize - pred_order } else { psize };
        for r in &res[idx..idx + n] {
            let u = zigzag(*r);
            w.unary(u >> k);
            if k > 0 {
                w.put(u & ((1u64 << k) - 1), k);
            }
        }
        idx += n;
    }
}

// ------------------------------------------------------------------ prediction

fn fixed_residual(x: &[i64], order: usize, out: &mut Vec<i64>) {
    out.clear();
    for i in order..x.len() {
        let r = match order {
            0 => x[i],
            1 => x[i] - x[i - 1],
            2 => x[i] - 2 * x[i - 1] + x[i - 2],
            3 => x[i] - 3 * x[i - 1] + 3 * x[i - 2] - x[i - 3],
            _ => x[i] - 4 * x[i - 1] + 6 * x[i - 2] - 4 * x[i - 3] + x[i - 4],
        };
        out.push(r);
    }
}

/// Cheap cost estimate: smallest sum of |residual| over the fixed orders.
fn fixed_estimate(x: &[i64]) -> u64 {
    let mut s = [0u64; 5];
    for i in 4..x.len() {
        let e0 = x[i];
        let e1 = e0 - x[i - 1];
        let e2 = e1 - (x[i - 1] - x[i - 2]);
        let e3 = e2 - (x[i - 1] - 2 * x[i - 2] + x[i - 3]);
        let e4 = e3 - (x[i - 1] - 3 * x[i - 2] + 3 * x[i - 3] - x[i - 4]);
        s[0] += e0.unsigned_abs();
        s[1] += e1.unsigned_abs();
        s[2] += e2.unsigned_abs();
        s[3] += e3.unsigned_abs();
        s[4] += e4.unsigned_abs();
    }
    *s.iter().min().unwrap()
}

/// Levinson–Durbin: LPC coefficients for each order 1..=max and their errors.
fn levinson(r: &[f64], max: usize) -> Vec<(Vec<f64>, f64)> {
    let mut out = Vec::with_capacity(max);
    let mut a = vec![0.0f64; max];
    let mut err = r[0];
    for i in 0..max {
        if err <= 0.0 {
            break;
        }
        let mut acc = r[i + 1];
        for j in 0..i {
            acc -= a[j] * r[i - j];
        }
        let k = acc / err;
        let prev = a.clone();
        a[i] = k;
        for j in 0..i {
            a[j] = prev[j] - k * prev[i - 1 - j];
        }
        err *= 1.0 - k * k;
        out.push((a[..=i].to_vec(), err.max(0.0)));
    }
    out
}

fn quantize_lpc(lpc: &[f64], precision: u32) -> Option<(Vec<i32>, u32)> {
    let cmax = lpc.iter().fold(0.0f64, |m, c| m.max(c.abs()));
    if !(cmax > 0.0) || !cmax.is_finite() {
        return None;
    }
    let max_q = (1i64 << (precision - 1)) - 1;
    let min_q = -(1i64 << (precision - 1));
    let shift = (precision as i32 - 1) - cmax.log2().ceil() as i32;
    if shift < 0 {
        return None;
    }
    let shift = shift.min(15) as u32;
    let scale = (1u64 << shift) as f64;
    let mut q = Vec::with_capacity(lpc.len());
    let mut e = 0.0f64;
    for c in lpc {
        e += c * scale;
        let v = (e.round() as i64).clamp(min_q, max_q);
        e -= v as f64;
        q.push(v as i32);
    }
    Some((q, shift))
}

fn lpc_residual(x: &[i64], q: &[i32], shift: u32, out: &mut Vec<i64>) -> bool {
    out.clear();
    let order = q.len();
    let lim = 1i64 << 30;
    for i in order..x.len() {
        let mut sum = 0i64;
        for (j, c) in q.iter().enumerate() {
            sum += *c as i64 * x[i - 1 - j];
        }
        let r = x[i] - (sum >> shift);
        if r.abs() >= lim {
            return false;
        }
        out.push(r);
    }
    true
}

enum Kind {
    Constant,
    Verbatim,
    Fixed(usize),
    Lpc(Vec<i32>, u32, u32),
}

struct Subframe {
    kind: Kind,
    residual: Vec<i64>,
    rice: Option<RiceChoice>,
    bits: u64,
}

fn analyse(x: &[i64], bps: u32, window: &[f64]) -> Subframe {
    let n = x.len();
    if x.iter().all(|v| *v == x[0]) {
        return Subframe { kind: Kind::Constant, residual: Vec::new(), rice: None, bits: 8 + bps as u64 };
    }
    let verbatim_bits = 8 + n as u64 * bps as u64;
    let mut best = Subframe { kind: Kind::Verbatim, residual: Vec::new(), rice: None, bits: verbatim_bits };
    let mut tmp = Vec::with_capacity(n);

    // Fixed predictors.
    for order in 0..=4usize.min(n.saturating_sub(1)) {
        fixed_residual(x, order, &mut tmp);
        let rc = choose_rice(&tmp, n, order);
        let bits = 8 + order as u64 * bps as u64 + rc.bits;
        if bits < best.bits {
            best = Subframe { kind: Kind::Fixed(order), residual: std::mem::take(&mut tmp), rice: Some(rc), bits };
            tmp = Vec::with_capacity(n);
        }
    }

    // Linear prediction.
    if n > 32 {
        let max_order = MAX_LPC.min(n - 1);
        let mut r = vec![0.0f64; max_order + 1];
        let w: Vec<f64> = x.iter().zip(window).map(|(v, w)| *v as f64 * w).collect();
        for lag in 0..=max_order {
            let mut s = 0.0;
            for i in lag..n {
                s += w[i] * w[i - lag];
            }
            r[lag] = s;
        }
        if r[0] > 0.0 {
            let precision: u32 = if bps <= 17 { 12 } else { 15 };
            let sets = levinson(&r, max_order);
            // Pick the order the error predicts to be cheapest, then measure.
            let mut cands: Vec<(f64, usize)> = sets
                .iter()
                .enumerate()
                .map(|(i, (_, e))| {
                    let order = i + 1;
                    let per = if *e > 0.0 { (0.5 * (0.5 * e / n as f64).log2()).max(0.0) } else { 0.0 };
                    (per * (n - order) as f64 + order as f64 * (precision + bps) as f64, order)
                })
                .collect();
            cands.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            for &(_, order) in cands.iter().take(2) {
                let Some((q, shift)) = quantize_lpc(&sets[order - 1].0, precision) else { continue };
                if !lpc_residual(x, &q, shift, &mut tmp) {
                    continue;
                }
                let rc = choose_rice(&tmp, n, order);
                let bits = 8 + order as u64 * bps as u64 + 4 + 5 + order as u64 * precision as u64 + rc.bits;
                if bits < best.bits {
                    best = Subframe { kind: Kind::Lpc(q, shift, precision), residual: std::mem::take(&mut tmp), rice: Some(rc), bits };
                    tmp = Vec::with_capacity(n);
                }
            }
        }
    }
    best
}

fn write_subframe(w: &mut Bits, x: &[i64], bps: u32, sf: &Subframe) {
    let n = x.len();
    match &sf.kind {
        Kind::Constant => {
            w.put(0, 8);
            w.put_signed(x[0], bps);
        }
        Kind::Verbatim => {
            w.put(0b0000_0010, 8);
            for v in x {
                w.put_signed(*v, bps);
            }
        }
        Kind::Fixed(order) => {
            w.put((0b00_1000 | *order as u64) << 1, 8);
            for v in &x[..*order] {
                w.put_signed(*v, bps);
            }
            write_residual(w, &sf.residual, n, *order, sf.rice.as_ref().unwrap());
        }
        Kind::Lpc(q, shift, precision) => {
            let order = q.len();
            w.put((0b10_0000 | (order as u64 - 1)) << 1, 8);
            for v in &x[..order] {
                w.put_signed(*v, bps);
            }
            w.put(*precision as u64 - 1, 4);
            w.put(*shift as u64, 5);
            for c in q {
                w.put_signed(*c as i64, *precision);
            }
            write_residual(w, &sf.residual, n, order, sf.rice.as_ref().unwrap());
        }
    }
}

fn utf8_number(v: u64, out: &mut Vec<u8>) {
    if v < 0x80 {
        out.push(v as u8);
        return;
    }
    let mut n = 2;
    while n < 7 && v >= 1u64 << (5 * n + 1) {
        n += 1;
    }
    let lead_mask: u8 = !(0xFFu8 >> n);
    out.push(lead_mask | (v >> (6 * (n - 1))) as u8);
    for i in (0..n - 1).rev() {
        out.push(0x80 | ((v >> (6 * i)) & 0x3F) as u8);
    }
}

fn rate_code(sr: u32) -> u8 {
    match sr {
        88200 => 1,
        176400 => 2,
        192000 => 3,
        8000 => 4,
        16000 => 5,
        22050 => 6,
        24000 => 7,
        32000 => 8,
        44100 => 9,
        48000 => 10,
        96000 => 11,
        _ => 0,
    }
}

fn tukey(n: usize) -> Vec<f64> {
    let p = 0.5;
    let np = ((p / 2.0) * n as f64) as usize;
    (0..n)
        .map(|i| {
            if np > 0 && i < np {
                0.5 - 0.5 * (std::f64::consts::PI * i as f64 / np as f64).cos()
            } else if np > 0 && i >= n - np {
                0.5 - 0.5 * (std::f64::consts::PI * (n - 1 - i) as f64 / np as f64).cos()
            } else {
                1.0
            }
        })
        .collect()
}

/// Encode one frame from integer channels (all the same length).
fn encode_frame(chs: &[Vec<i64>], bps: u32, sr: u32, frame_no: u64, window: &[f64]) -> Vec<u8> {
    let n = chs[0].len();
    let win: Vec<f64> = if n == BLOCK { window.to_vec() } else { tukey(n) };
    // Channel assignment and the signals to code.
    let (assign, sigs): (u8, Vec<(Vec<i64>, u32)>) = if chs.len() == 2 {
        let (l, r) = (&chs[0], &chs[1]);
        let side: Vec<i64> = l.iter().zip(r).map(|(a, b)| a - b).collect();
        let mid: Vec<i64> = l.iter().zip(r).map(|(a, b)| (a + b) >> 1).collect();
        let el = fixed_estimate(l);
        let er = fixed_estimate(r);
        let es = fixed_estimate(&side);
        let em = fixed_estimate(&mid);
        let opts = [(el + er, 1u8), (el + es, 8), (er + es, 9), (em + es, 10)];
        let pick = opts.iter().min_by_key(|o| o.0).unwrap().1;
        match pick {
            8 => (8, vec![(l.clone(), bps), (side, bps + 1)]),
            9 => (9, vec![(side, bps + 1), (r.clone(), bps)]),
            10 => (10, vec![(mid, bps), (side, bps + 1)]),
            _ => (1, vec![(l.clone(), bps), (r.clone(), bps)]),
        }
    } else {
        ((chs.len() - 1) as u8, chs.iter().map(|c| (c.clone(), bps)).collect())
    };

    let mut head = vec![0xFF, 0xF8];
    let (bs_code, bs_extra): (u8, Option<(u64, u32)>) = if n == 4096 {
        (12, None)
    } else if n <= 256 {
        (6, Some((n as u64 - 1, 8)))
    } else {
        (7, Some((n as u64 - 1, 16)))
    };
    head.push(bs_code << 4 | rate_code(sr));
    let size_code: u8 = match bps {
        8 => 1,
        12 => 2,
        16 => 4,
        20 => 5,
        24 => 6,
        _ => 0,
    };
    head.push(assign << 4 | size_code << 1);
    utf8_number(frame_no, &mut head);
    if let Some((v, bits)) = bs_extra {
        if bits == 8 {
            head.push(v as u8);
        } else {
            head.extend_from_slice(&(v as u16).to_be_bytes());
        }
    }
    head.push(crc8(&head));

    let mut w = Bits::new();
    w.buf.extend_from_slice(&head);
    for (sig, b) in &sigs {
        let sf = analyse(sig, *b, &win);
        write_subframe(&mut w, sig, *b, &sf);
    }
    w.align();
    let crc = crc16(&w.buf);
    w.buf.extend_from_slice(&crc.to_be_bytes());
    w.buf
}

/// Write a complete FLAC file.
pub fn write<W: Write + Seek>(
    out: &mut W,
    chs: &[Vec<f32>],
    sample_rate: u32,
    bps: u32,
    dither: bool,
    vorbis: &[u8],
    progress: &Progress,
) -> Result<(), String> {
    let err = |e: std::io::Error| format!("Write failed: {e}");
    if !(1..=8).contains(&chs.len()) {
        return Err("FLAC supports 1 to 8 channels.".into());
    }
    if sample_rate == 0 || sample_rate > 655_350 {
        return Err("FLAC can't store this sample rate.".into());
    }
    let n_ch = chs.len();
    let len = chs[0].len();
    let start = out.stream_position().map_err(err)?;
    out.write_all(b"fLaC").map_err(err)?;
    // STREAMINFO placeholder, rewritten at the end.
    out.write_all(&[0x00, 0, 0, 34]).map_err(err)?;
    out.write_all(&[0u8; 34]).map_err(err)?;
    // Vorbis comment (always present: carries the encoder name), then padding.
    out.write_all(&[0x04]).map_err(err)?;
    out.write_all(&(vorbis.len() as u32).to_be_bytes()[1..]).map_err(err)?;
    out.write_all(vorbis).map_err(err)?;
    let pad = 1024u32;
    out.write_all(&[0x81]).map_err(err)?;
    out.write_all(&pad.to_be_bytes()[1..]).map_err(err)?;
    out.write_all(&vec![0u8; pad as usize]).map_err(err)?;

    let window = tukey(BLOCK);
    let mut q = Quantizer::new(dither && bps < 32);
    let mut md5 = Md5::new();
    let mut min_frame = u32::MAX;
    let mut max_frame = 0u32;
    let bytes_ps = (bps as usize + 7) / 8;
    let mut raw = Vec::with_capacity(BLOCK * n_ch * bytes_ps);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 16);
    let batch = threads * 4;
    let mut frame_no = 0u64;
    let mut i = 0;
    while i < len {
        if progress.cancelled() {
            return Err("Cancelled".into());
        }
        // Quantise a batch of blocks in order (dither and MD5 are sequential),
        // then encode the blocks in parallel.
        let mut blocks: Vec<Vec<Vec<i64>>> = Vec::with_capacity(batch);
        while blocks.len() < batch && i < len {
            let end = (i + BLOCK).min(len);
            let mut ints: Vec<Vec<i64>> = vec![Vec::with_capacity(end - i); n_ch];
            raw.clear();
            for f in i..end {
                for (c, ch) in chs.iter().enumerate() {
                    let v = q.quantize(ch[f], bps);
                    ints[c].push(v as i64);
                    raw.extend_from_slice(&v.to_le_bytes()[..bytes_ps]);
                }
            }
            md5.update(&raw);
            blocks.push(ints);
            i = end;
        }
        let first = frame_no;
        let mut frames: Vec<Vec<u8>> = vec![Vec::new(); blocks.len()];
        let per = (blocks.len() + threads - 1) / threads;
        std::thread::scope(|sc| {
            for (k, (outs, ins)) in frames.chunks_mut(per).zip(blocks.chunks(per)).enumerate() {
                let window = &window;
                sc.spawn(move || {
                    for (j, (o, b)) in outs.iter_mut().zip(ins).enumerate() {
                        *o = encode_frame(b, bps, sample_rate, first + (k * per + j) as u64, window);
                    }
                });
            }
        });
        for frame in &frames {
            min_frame = min_frame.min(frame.len() as u32);
            max_frame = max_frame.max(frame.len() as u32);
            out.write_all(frame).map_err(err)?;
        }
        frame_no += frames.len() as u64;
        progress.set(i as f32 / len.max(1) as f32);
    }
    if frame_no == 0 {
        min_frame = 0;
    }

    // STREAMINFO.
    let mut si = Vec::with_capacity(34);
    si.extend_from_slice(&(BLOCK as u16).to_be_bytes()); // min block (all but the last)
    si.extend_from_slice(&(BLOCK as u16).to_be_bytes());
    si.extend_from_slice(&min_frame.to_be_bytes()[1..]);
    si.extend_from_slice(&max_frame.to_be_bytes()[1..]);
    let total = len as u64 & 0xF_FFFF_FFFF;
    let packed: u64 = (sample_rate as u64) << 44 | ((n_ch as u64 - 1) << 41) | ((bps as u64 - 1) << 36) | total;
    si.extend_from_slice(&packed.to_be_bytes());
    si.extend_from_slice(&md5.finish());
    let end_pos = out.stream_position().map_err(err)?;
    out.seek(SeekFrom::Start(start + 8)).map_err(err)?;
    out.write_all(&si).map_err(err)?;
    out.seek(SeekFrom::Start(end_pos)).map_err(err)?;
    out.flush().map_err(err)?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn encode_frame_for_test(chs: &[Vec<i64>], bps: u32) -> Vec<u8> {
    encode_frame(chs, bps, 44100, 0, &tukey(BLOCK))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_numbers() {
        let mut v = Vec::new();
        utf8_number(0x7F, &mut v);
        assert_eq!(v, [0x7F]);
        v.clear();
        utf8_number(0x80, &mut v);
        assert_eq!(v, [0xC2, 0x80]);
        v.clear();
        utf8_number(0x800, &mut v);
        assert_eq!(v, [0xE0, 0xA0, 0x80]);
    }

    #[test]
    fn crcs() {
        assert_eq!(crc8(b"123456789"), 0xF4);
        assert_eq!(crc16(b"123456789"), 0xFEE8);
    }

    #[test]
    fn lpc_beats_verbatim_on_a_sine() {
        let x: Vec<i64> = (0..BLOCK).map(|i| ((i as f64 * 0.05).sin() * 20000.0) as i64).collect();
        let sf = analyse(&x, 16, &tukey(BLOCK));
        assert!(matches!(sf.kind, Kind::Lpc(..) | Kind::Fixed(_)));
        assert!(sf.bits < (BLOCK as u64 * 16) / 3, "{} bits", sf.bits);
    }
}
