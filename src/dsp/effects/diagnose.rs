//! Diagnostics: find clicks, clipping, silence or audio in a file and list
//! them, so each can be repaired, deleted or marked individually.

use super::prelude::*;
use super::restoration::hermite_fill;

/// One problem (or region) found by a scan; sample positions are absolute.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub start: usize,
    pub len: usize,
    /// Which channels are affected (bit per channel).
    pub channels: u8,
    /// Size in dB (click height, clipped level, or region level).
    pub level_db: f32,
}

impl Finding {
    pub fn end(&self) -> usize {
        self.start + self.len
    }
}

/// Merge findings that overlap or touch (across channels).
fn merge(mut v: Vec<Finding>, gap: usize) -> Vec<Finding> {
    v.sort_by_key(|f| f.start);
    let mut out: Vec<Finding> = Vec::new();
    for f in v {
        if let Some(last) = out.last_mut() {
            if f.start <= last.end() + gap {
                let end = last.end().max(f.end());
                last.len = end - last.start;
                last.channels |= f.channels;
                last.level_db = last.level_db.max(f.level_db);
                continue;
            }
        }
        out.push(f);
    }
    out
}

/// Clicks and pops: short bursts where the second-order prediction error
/// jumps far above its local average (same detector as Click/Pop Eliminator).
pub fn find_clicks(chs: &[Vec<f32>], sr: u32, sensitivity: f32, max_ms: f32, offset: usize) -> Vec<Finding> {
    let k = 2.0 + (100.0 - sensitivity.clamp(1.0, 100.0)) / 8.0;
    let max_len = ms_to_samples(max_ms, sr).max(1);
    let mut found = Vec::new();
    for (ci, ch) in chs.iter().enumerate() {
        let len = ch.len();
        if len < 16 {
            continue;
        }
        let mut e = vec![0.0f32; len];
        for n in 2..len {
            e[n] = (ch[n] - (2.0 * ch[n - 1] - ch[n - 2])).abs();
        }
        let win = 512usize.min(len);
        let mut prefix = vec![0.0f64; len + 1];
        for n in 0..len {
            prefix[n + 1] = prefix[n] + e[n] as f64;
        }
        let local = |n: usize| {
            let a = n.saturating_sub(win / 2);
            let b = (n + win / 2).min(len);
            ((prefix[b] - prefix[a]) / (b - a).max(1) as f64) as f32
        };
        let mut n = 3;
        while n + 3 < len {
            if e[n] > k * local(n) + 1e-4 {
                let start = n;
                let mut end = n;
                let mut m = n + 1;
                let mut height = e[n];
                while m < len && m - start <= max_len + 2 {
                    if e[m] > k * local(m) + 1e-4 {
                        end = m;
                        height = height.max(e[m]);
                    }
                    m += 1;
                }
                if end - start <= max_len {
                    found.push(Finding { start: offset + start.saturating_sub(2), len: end - start + 5, channels: 1 << ci.min(7), level_db: lin_to_db(height) });
                }
                n = end + 3;
            } else {
                n += 1;
            }
        }
    }
    merge(found, 0)
}

/// Clipping: runs of at least three samples at or above `threshold_db`.
pub fn find_clipping(chs: &[Vec<f32>], threshold_db: f32, offset: usize) -> Vec<Finding> {
    let thr = db_to_lin(threshold_db);
    let mut found = Vec::new();
    for (ci, ch) in chs.iter().enumerate() {
        let mut n = 0;
        while n < ch.len() {
            if ch[n].abs() >= thr {
                let start = n;
                let sign = ch[n].signum();
                let mut pk = ch[n].abs();
                while n + 1 < ch.len() && ch[n + 1].abs() >= thr && ch[n + 1].signum() == sign {
                    n += 1;
                    pk = pk.max(ch[n].abs());
                }
                if n + 1 - start >= 3 {
                    found.push(Finding { start: offset + start, len: n + 1 - start, channels: 1 << ci.min(7), level_db: lin_to_db(pk) });
                }
            }
            n += 1;
        }
    }
    merge(found, 0)
}

/// Peak level of every 10 ms block (max across channels).
fn block_levels(chs: &[Vec<f32>], sr: u32) -> (usize, Vec<f32>) {
    let block = ms_to_samples(10.0, sr).max(1);
    let len = len_of(chs);
    let levels = (0..len.div_ceil(block))
        .map(|b| {
            let (a, e) = (b * block, ((b + 1) * block).min(len));
            chs.iter().map(|c| c[a..e].iter().fold(0.0f32, |m, s| m.max(s.abs()))).fold(0.0, f32::max)
        })
        .collect();
    (block, levels)
}

/// Silence: stretches quieter than `threshold_db` for at least `min_ms`.
pub fn find_silence(chs: &[Vec<f32>], sr: u32, threshold_db: f32, min_ms: f32, offset: usize) -> Vec<Finding> {
    regions(chs, sr, threshold_db, min_ms, offset, false)
}

/// Audio: stretches louder than `threshold_db` lasting at least `min_ms`
/// (gaps shorter than `min_ms` are bridged).
pub fn find_audio(chs: &[Vec<f32>], sr: u32, threshold_db: f32, min_ms: f32, offset: usize) -> Vec<Finding> {
    let v = regions(chs, sr, threshold_db, 0.0, offset, true);
    let gap = ms_to_samples(min_ms, sr);
    merge(v, gap).into_iter().filter(|f| f.len >= gap).collect()
}

fn regions(chs: &[Vec<f32>], sr: u32, threshold_db: f32, min_ms: f32, offset: usize, loud: bool) -> Vec<Finding> {
    let thr = db_to_lin(threshold_db);
    let (block, levels) = block_levels(chs, sr);
    let len = len_of(chs);
    let min_blocks = (min_ms / 10.0).ceil().max(1.0) as usize;
    let mask = u8::MAX >> (8 - chs.len().clamp(1, 8));
    let mut out = Vec::new();
    let mut b = 0;
    while b < levels.len() {
        if (levels[b] >= thr) == loud {
            let s = b;
            let mut pk = 0.0f32;
            while b < levels.len() && (levels[b] >= thr) == loud {
                pk = pk.max(levels[b]);
                b += 1;
            }
            if b - s >= min_blocks || loud {
                let start = s * block;
                let end = (b * block).min(len);
                out.push(Finding { start: offset + start, len: end - start, channels: mask, level_db: lin_to_db(pk) });
            }
        } else {
            b += 1;
        }
    }
    out
}

/// Repair clicks in place by interpolating over each finding.
pub fn repair_clicks(chs: &mut [Vec<f32>], found: &[Finding]) {
    for f in found {
        for (ci, ch) in chs.iter_mut().enumerate() {
            if f.channels & (1 << ci.min(7)) == 0 || ch.len() < 8 {
                continue;
            }
            let i0 = f.start.max(1);
            let i1 = (f.end() + 1).min(ch.len() - 2);
            if i1 > i0 + 1 {
                hermite_fill(ch, i0, i1);
            }
        }
    }
}

/// Rebuild clipped peaks in place (the result may exceed full scale; lower
/// the level afterwards if needed).
pub fn repair_clipping(chs: &mut [Vec<f32>], found: &[Finding]) {
    for f in found {
        for (ci, ch) in chs.iter_mut().enumerate() {
            if f.channels & (1 << ci.min(7)) == 0 || ch.len() < 8 {
                continue;
            }
            let i0 = f.start.saturating_sub(1).max(1);
            let i1 = f.end().min(ch.len() - 2);
            if i1 > i0 + 1 {
                hermite_fill(ch, i0, i1);
            }
        }
    }
}

/// Remove the found regions entirely (Delete Silence).
pub fn delete_regions(chs: &[Vec<f32>], found: &[Finding]) -> Vec<Vec<f32>> {
    chs.iter()
        .map(|ch| {
            let mut out = Vec::with_capacity(ch.len());
            let mut pos = 0;
            for f in found {
                let (a, b) = (f.start.min(ch.len()), f.end().min(ch.len()));
                if a > pos {
                    out.extend_from_slice(&ch[pos..a]);
                }
                pos = pos.max(b);
            }
            if pos < ch.len() {
                out.extend_from_slice(&ch[pos..]);
            }
            out
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn tone(n: usize) -> Vec<f32> {
        (0..n).map(|i| 0.3 * (TAU * 300.0 * i as f32 / 44100.0).sin()).collect()
    }

    #[test]
    fn finds_and_repairs_clicks() {
        let mut x = tone(44100);
        let clean = x.clone();
        for &p in &[5000usize, 20000, 31000] {
            x[p] += 0.7;
            x[p + 1] -= 0.5;
        }
        let f = find_clicks(&[x.clone()], 44100, 50.0, 1.0, 0);
        assert_eq!(f.len(), 3, "{f:?}");
        assert!(f.iter().all(|c| c.start <= 31000 && c.end() >= 5001));
        let mut y = vec![x];
        repair_clicks(&mut y, &f);
        let err = y[0].iter().zip(&clean).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(err < 0.05, "{err}");
    }

    #[test]
    fn finds_clipping_silence_and_audio() {
        let mut x = tone(44100);
        for s in x[10000..10020].iter_mut() {
            *s = 1.0;
        }
        let c = find_clipping(&[x.clone()], -0.1, 100);
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].start, c[0].len), (10100, 20));
        // Repair the flat top: it becomes a curve again.
        let c0 = find_clipping(&[x.clone()], -0.1, 0);
        let mut y = vec![x.clone()];
        repair_clipping(&mut y, &c0);
        let flat = y[0][10002..10018].windows(2).filter(|w| w[0] == w[1]).count();
        assert!(flat == 0, "still flat: {:?}", &y[0][10000..10020]);

        let mut z = tone(44100);
        for s in z[20000..30000].iter_mut() {
            *s = 0.0;
        }
        let s = find_silence(&[z.clone()], 44100, -60.0, 100.0, 0);
        assert_eq!(s.len(), 1, "{s:?}");
        assert!(s[0].start >= 19800 && s[0].start <= 20500 && s[0].end() >= 29500, "{s:?}");
        let a = find_audio(&[z.clone()], 44100, -40.0, 100.0, 0);
        assert_eq!(a.len(), 2, "{a:?}");
        let d = delete_regions(&[z], &s);
        assert!(d[0].len() < 44100 - 9000);
    }
}
