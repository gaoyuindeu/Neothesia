//! Which chord a stack of fingering digits belongs to: a small network over the geometry of
//! the stack and the chord (trained with tools/train_assoc.py on PDMX scores, where the
//! PDF text layer and the score give the true pairs).
//!
//! assoc.bin: u32 feature count, means, standard deviations, u32 layer count, then per layer
//! u32 inputs, u32 outputs, weights (outputs x inputs, row major) and biases, little endian
//! f32. Empty: no model, the reader uses its hand made cost.

use std::sync::OnceLock;

/// Number of features of a (stack, chord) pair
pub const FEATURES: usize = 20;

pub struct Mlp {
    mean: Vec<f32>,
    std: Vec<f32>,
    layers: Vec<(usize, usize, Vec<f32>, Vec<f32>)>,
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn u32(&mut self) -> Option<usize> {
        let (head, rest) = self.0.split_first_chunk::<4>()?;
        self.0 = rest;
        Some(u32::from_le_bytes(*head) as usize)
    }

    fn f32s(&mut self, n: usize) -> Option<Vec<f32>> {
        let bytes = self.0.get(..n * 4)?;
        self.0 = &self.0[n * 4..];
        Some(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect(),
        )
    }
}

impl Mlp {
    pub fn load(bytes: &[u8]) -> Option<Self> {
        let mut r = Reader(bytes);
        let n = r.u32()?;
        if n != FEATURES {
            log::warn!("Fingering association model: {n} features, expected {FEATURES}");
            return None;
        }
        let mean = r.f32s(n)?;
        let std = r.f32s(n)?;
        let count = r.u32()?;
        let mut layers = Vec::new();
        let mut width = n;
        for _ in 0..count {
            let (inputs, outputs) = (r.u32()?, r.u32()?);
            if inputs != width {
                return None;
            }
            let w = r.f32s(inputs * outputs)?;
            let b = r.f32s(outputs)?;
            layers.push((inputs, outputs, w, b));
            width = outputs;
        }
        (width == 1).then_some(Self { mean, std, layers })
    }

    /// Probability that the stack belongs to the chord
    pub fn prob(&self, x: &[f32; FEATURES]) -> f32 {
        let mut v: Vec<f32> = x
            .iter()
            .zip(&self.mean)
            .zip(&self.std)
            .map(|((x, m), s)| (x - m) / s.max(1e-6))
            .collect();
        let last = self.layers.len() - 1;
        for (l, (inputs, outputs, w, b)) in self.layers.iter().enumerate() {
            let mut out = b.clone();
            for (o, out_v) in out.iter_mut().enumerate().take(*outputs) {
                let row = &w[o * inputs..(o + 1) * inputs];
                *out_v += row.iter().zip(&v).map(|(a, b)| a * b).sum::<f32>();
                if l < last {
                    *out_v = out_v.max(0.0);
                }
            }
            v = out;
        }
        1.0 / (1.0 + (-v[0]).exp())
    }
}

/// The trained model, if there is one
pub fn model() -> Option<&'static Mlp> {
    static MODEL: OnceLock<Option<Mlp>> = OnceLock::new();
    MODEL
        .get_or_init(|| {
            if std::env::var_os("SCORE_READER_NO_ASSOC").is_some() {
                return None;
            }
            let bytes: &[u8] = include_bytes!("assoc.bin");
            if bytes.is_empty() {
                return None;
            }
            Mlp::load(bytes)
        })
        .as_ref()
}
