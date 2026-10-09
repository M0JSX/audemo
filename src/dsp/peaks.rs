//! Multi-resolution min/max cache for fast waveform drawing.

const BLOCKS: [usize; 3] = [32, 512, 8192];

pub struct PeakLevel {
    pub block: usize,
    /// Per channel, (min, max) per block.
    pub data: Vec<Vec<(f32, f32)>>,
}

pub struct PeakCache {
    pub levels: Vec<PeakLevel>,
}

impl PeakCache {
    pub fn empty() -> Self {
        PeakCache { levels: Vec::new() }
    }

    pub fn build(chs: &[Vec<f32>]) -> Self {
        let mut levels: Vec<PeakLevel> = Vec::new();
        for (li, &block) in BLOCKS.iter().enumerate() {
            let data = if li == 0 {
                chs.iter()
                    .map(|c| {
                        c.chunks(block)
                            .map(|b| {
                                b.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &s| {
                                    (lo.min(s), hi.max(s))
                                })
                            })
                            .collect()
                    })
                    .collect()
            } else {
                let prev = &levels[li - 1];
                let factor = block / prev.block;
                prev.data
                    .iter()
                    .map(|c| {
                        c.chunks(factor)
                            .map(|b| {
                                b.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &(a, z)| {
                                    (lo.min(a), hi.max(z))
                                })
                            })
                            .collect()
                    })
                    .collect()
            };
            levels.push(PeakLevel { block, data });
        }
        PeakCache { levels }
    }

    /// (min, max) of channel `ch` over samples [start, end).
    pub fn min_max(&self, chs: &[Vec<f32>], ch: usize, start: usize, end: usize) -> (f32, f32) {
        let c = match chs.get(ch) {
            Some(c) => c,
            None => return (0.0, 0.0),
        };
        let end = end.min(c.len());
        if start >= end {
            return (0.0, 0.0);
        }
        let span = end - start;
        let level = self.levels.iter().rev().find(|l| l.block * 2 <= span);
        let (lo, hi) = match level {
            None => c[start..end]
                .iter()
                .fold((f32::MAX, f32::MIN), |(lo, hi), &s| (lo.min(s), hi.max(s))),
            Some(l) => {
                let d = &l.data[ch];
                let a = start / l.block;
                let b = ((end + l.block - 1) / l.block).min(d.len());
                d[a..b]
                    .iter()
                    .fold((f32::MAX, f32::MIN), |(lo, hi), &(x, y)| (lo.min(x), hi.max(y)))
            }
        };
        if lo > hi {
            (0.0, 0.0)
        } else {
            (lo, hi)
        }
    }
}
