//! Iterative radix-2 FFT.

use std::ops::{Add, AddAssign, Mul, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Complex {
    pub re: f32,
    pub im: f32,
}

impl Complex {
    pub const ZERO: Complex = Complex { re: 0.0, im: 0.0 };
    #[inline]
    pub fn new(re: f32, im: f32) -> Self {
        Complex { re, im }
    }
    #[inline]
    pub fn from_polar(mag: f32, phase: f32) -> Self {
        Complex { re: mag * phase.cos(), im: mag * phase.sin() }
    }
    #[inline]
    pub fn norm(self) -> f32 {
        (self.re * self.re + self.im * self.im).sqrt()
    }
    #[inline]
    pub fn norm_sqr(self) -> f32 {
        self.re * self.re + self.im * self.im
    }
    #[inline]
    pub fn arg(self) -> f32 {
        self.im.atan2(self.re)
    }
    #[inline]
    pub fn conj(self) -> Self {
        Complex { re: self.re, im: -self.im }
    }
    #[inline]
    pub fn scale(self, k: f32) -> Self {
        Complex { re: self.re * k, im: self.im * k }
    }
}

impl Add for Complex {
    type Output = Complex;
    #[inline]
    fn add(self, o: Complex) -> Complex {
        Complex { re: self.re + o.re, im: self.im + o.im }
    }
}
impl AddAssign for Complex {
    #[inline]
    fn add_assign(&mut self, o: Complex) {
        self.re += o.re;
        self.im += o.im;
    }
}
impl Sub for Complex {
    type Output = Complex;
    #[inline]
    fn sub(self, o: Complex) -> Complex {
        Complex { re: self.re - o.re, im: self.im - o.im }
    }
}
impl Mul for Complex {
    type Output = Complex;
    #[inline]
    fn mul(self, o: Complex) -> Complex {
        Complex {
            re: self.re * o.re - self.im * o.im,
            im: self.re * o.im + self.im * o.re,
        }
    }
}

pub struct Fft {
    n: usize,
    twiddles: Vec<Complex>,
    rev: Vec<usize>,
}

impl Fft {
    pub fn new(n: usize) -> Self {
        assert!(n.is_power_of_two() && n >= 2, "FFT size must be a power of two");
        let bits = n.trailing_zeros();
        let rev = (0..n)
            .map(|i| i.reverse_bits() >> (usize::BITS - bits))
            .collect();
        let twiddles = (0..n / 2)
            .map(|k| {
                let a = -std::f64::consts::TAU * k as f64 / n as f64;
                Complex::new(a.cos() as f32, a.sin() as f32)
            })
            .collect();
        Fft { n, twiddles, rev }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    fn run(&self, buf: &mut [Complex], inverse: bool) {
        let n = self.n;
        assert_eq!(buf.len(), n);
        for i in 0..n {
            let j = self.rev[i];
            if j > i {
                buf.swap(i, j);
            }
        }
        let mut size = 2;
        while size <= n {
            let half = size / 2;
            let step = n / size;
            for start in (0..n).step_by(size) {
                for k in 0..half {
                    let mut w = self.twiddles[k * step];
                    if inverse {
                        w = w.conj();
                    }
                    let a = buf[start + k];
                    let b = buf[start + k + half] * w;
                    buf[start + k] = a + b;
                    buf[start + k + half] = a - b;
                }
            }
            size *= 2;
        }
    }

    pub fn forward(&self, buf: &mut [Complex]) {
        self.run(buf, false);
    }

    /// Inverse transform, scaled by 1/n so forward→inverse is identity.
    pub fn inverse(&self, buf: &mut [Complex]) {
        self.run(buf, true);
        let k = 1.0 / self.n as f32;
        for c in buf.iter_mut() {
            *c = c.scale(k);
        }
    }
}

/// Linear convolution of `x` with `h` via FFT overlap-add.
/// Returns `x.len() + h.len() - 1` samples.
pub fn fft_convolve(x: &[f32], h: &[f32]) -> Vec<f32> {
    if x.is_empty() || h.is_empty() {
        return Vec::new();
    }
    let out_len = x.len() + h.len() - 1;
    let n = (2 * h.len()).next_power_of_two().max(1024);
    let block = n - h.len() + 1;
    let fft = Fft::new(n);
    let mut hf = vec![Complex::ZERO; n];
    for (i, &v) in h.iter().enumerate() {
        hf[i].re = v;
    }
    fft.forward(&mut hf);
    let mut out = vec![0.0f32; out_len];
    let mut buf = vec![Complex::ZERO; n];
    let mut pos = 0;
    while pos < x.len() {
        let end = (pos + block).min(x.len());
        for c in buf.iter_mut() {
            *c = Complex::ZERO;
        }
        for (i, &v) in x[pos..end].iter().enumerate() {
            buf[i].re = v;
        }
        fft.forward(&mut buf);
        for (b, hv) in buf.iter_mut().zip(hf.iter()) {
            *b = *b * *hv;
        }
        fft.inverse(&mut buf);
        for i in 0..n {
            let o = pos + i;
            if o >= out_len {
                break;
            }
            out[o] += buf[i].re;
        }
        pos = end;
    }
    out
}
