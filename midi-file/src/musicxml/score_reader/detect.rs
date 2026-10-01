//! Fingering digits and note heads found by a small trained network
//! (tools/train_detector.py).
//!
//! The network looks at a binarized page scaled to about 21 pixels between staff lines and
//! gives, at half resolution, one heat map per class (peaks at object centers) and the box
//! size: digits 1-5, then black, half and whole note heads.
//!
//! The network runs on the GPU when there is one (gpu.rs), else on the CPU (rten).

use std::sync::OnceLock;

use rten::{Model, NodeId};
use rten_tensor::{AsView, Layout, NdTensor};

use super::gpu;

const MODEL: &[u8] = include_bytes!("detector.onnx");

/// Staff line distance the network was trained at
const INTERLINE: f32 = 21.0;
/// The page goes through the network in tiles of this size (the same on the GPU and the
/// CPU: the same page gives the same objects on every machine)
const TILE: usize = 1536;
const MARGIN: usize = 64;

/// Classes 0-4: digits 1-5
pub const HEAD_BLACK: u8 = 5;

/// An object and its box in page pixels
#[derive(Debug, Clone, Copy)]
pub struct Found {
    pub class: u8,
    pub score: f32,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

/// Heat maps and box sizes of a tile: values and shape (channels, height, width)
type Maps = ((Vec<f32>, [usize; 3]), (Vec<f32>, [usize; 3]));

struct Detector {
    gpu: Option<gpu::Net>,
    model: Model,
    input: NodeId,
    heat: NodeId,
    size: NodeId,
}

fn detector() -> Option<&'static Detector> {
    static DETECTOR: OnceLock<Option<Detector>> = OnceLock::new();
    DETECTOR
        .get_or_init(|| {
            let model = Model::load_static_slice(MODEL)
                .map_err(|e| log::warn!("Fingering detector: {e}"))
                .ok()?;
            let gpu = if std::env::var_os("NEOTHESIA_DETECT_CPU").is_some() {
                None
            } else {
                let t = std::time::Instant::now();
                let net = gpu::Net::new(MODEL);
                if std::env::var_os("SCORE_READER_TIMING").is_some() {
                    eprintln!("GPU setup {:.1?}", t.elapsed());
                }
                net
            };
            Some(Detector {
                gpu,
                input: *model.input_ids().first()?,
                heat: model.node_id("heat").ok()?,
                size: model.node_id("size").ok()?,
                model,
            })
        })
        .as_ref()
}

impl Detector {
    /// The network on one tile (`ph` x `pw`), on the GPU if possible
    fn run(&self, data: Vec<f32>, ph: usize, pw: usize) -> Option<Maps> {
        if let Some(gpu) = &self.gpu {
            if let Some(mut out) = gpu.run(&data, ph, pw)
                && out.len() == 2
            {
                let size = out.pop()?;
                return Some((out.pop()?, size));
            }
            log::warn!("Fingering detector: GPU failed, on the CPU");
        }
        let input = NdTensor::from_data([1, 1, ph, pw], data);
        let [heat, size] = self
            .model
            .run_n(
                [(self.input, input.view().into())].into(),
                [self.heat, self.size],
                None,
            )
            .ok()?;
        let maps = |t: rten::Value| -> Option<(Vec<f32>, [usize; 3])> {
            let t: NdTensor<f32, 4> = t.try_into().ok()?;
            let [_, c, h, w] = t.shape();
            Some((t.to_vec(), [c, h, w]))
        };
        Some((maps(heat)?, maps(size)?))
    }
}

/// Scale a black and white page by `scale` (ink where enough of the source is ink)
fn rescale(w: usize, h: usize, ink: &[bool], scale: f32) -> (usize, usize, Vec<f32>) {
    let (nw, nh) = (
        ((w as f32 * scale).round() as usize).max(1),
        ((h as f32 * scale).round() as usize).max(1),
    );
    if (scale - 1.0).abs() < 0.02 {
        return (w, h, ink.iter().map(|&b| b as u8 as f32).collect());
    }
    let mut out = vec![0.0; nw * nh];
    for y in 0..nh {
        let sy0 = (y as f32 / scale) as usize;
        let sy1 = (((y + 1) as f32 / scale).ceil() as usize).clamp(sy0 + 1, h);
        for x in 0..nw {
            let sx0 = (x as f32 / scale) as usize;
            let sx1 = (((x + 1) as f32 / scale).ceil() as usize).clamp(sx0 + 1, w);
            let mut n = 0;
            for sy in sy0..sy1.min(h) {
                for sx in sx0..sx1.min(w) {
                    n += ink[sy * w + sx] as usize;
                }
            }
            let area = (sy1.min(h) - sy0) * (sx1.min(w) - sx0);
            out[y * nw + x] = if n as f32 >= 0.35 * area.max(1) as f32 {
                1.0
            } else {
                0.0
            };
        }
    }
    (nw, nh, out)
}

/// Objects on a black and white page (`ink`, row major, `w` x `h`) whose staff lines are
/// `interline` pixels apart; None when the detector is not available
pub fn detect(
    w: usize,
    h: usize,
    ink: &[bool],
    interline: f32,
    threshold: f32,
) -> Option<Vec<Found>> {
    let det = detector()?;
    let scale = INTERLINE / interline.max(1.0);
    let (sw, sh, page) = rescale(w, h, ink, scale);

    let mut found: Vec<Found> = Vec::new();
    let tile = TILE;
    let step = tile - 2 * MARGIN;
    let mut ty = 0;
    loop {
        let mut tx = 0;
        loop {
            let (x0, y0) = (
                tx.min(sw.saturating_sub(tile)),
                ty.min(sh.saturating_sub(tile)),
            );
            let (tw, th) = (tile.min(sw), tile.min(sh));
            // Multiple of 32 for the network
            let (pw, ph) = (tw.div_ceil(32) * 32, th.div_ceil(32) * 32);
            let mut data = vec![0.0f32; pw * ph];
            for y in 0..th {
                let row = (y0 + y) * sw + x0;
                data[y * pw..y * pw + tw].copy_from_slice(&page[row..row + tw]);
            }
            let ((heat, [classes, hh, hw]), (size, _)) = det.run(data, ph, pw)?;
            let at = |c: usize, i: usize, j: usize| (c * hh + i) * hw + j;
            // Output resolution: half or a quarter of the input, depending on the model
            let stride = ph / hh.max(1);

            // The part of this tile that is not overlapped by its neighbours
            let keep_x0 = if x0 == 0 { 0 } else { MARGIN };
            let keep_y0 = if y0 == 0 { 0 } else { MARGIN };
            let keep_x1 = if x0 + tile >= sw { tw } else { tile - MARGIN };
            let keep_y1 = if y0 + tile >= sh { th } else { tile - MARGIN };

            for c in 0..classes {
                for i in 0..hh {
                    for j in 0..hw {
                        let v = heat[at(c, i, j)];
                        if v < threshold {
                            continue;
                        }
                        let mut peak = true;
                        'n: for di in -1i32..=1 {
                            for dj in -1i32..=1 {
                                let (ni, nj) = (i as i32 + di, j as i32 + dj);
                                if (di, dj) != (0, 0)
                                    && ni >= 0
                                    && nj >= 0
                                    && (ni as usize) < hh
                                    && (nj as usize) < hw
                                    && heat[at(c, ni as usize, nj as usize)] > v
                                {
                                    peak = false;
                                    break 'n;
                                }
                            }
                        }
                        if !peak {
                            continue;
                        }
                        let half = stride as f32 / 2.0;
                        let (cx, cy) = ((j * stride) as f32 + half, (i * stride) as f32 + half);
                        if cx < keep_x0 as f32
                            || cx >= keep_x1 as f32
                            || cy < keep_y0 as f32
                            || cy >= keep_y1 as f32
                        {
                            continue;
                        }
                        let bw = (size[at(0, i, j)] * 32.0).max(2.0);
                        let bh = (size[at(1, i, j)] * 32.0).max(2.0);
                        let (px, py) = ((x0 as f32 + cx) / scale, (y0 as f32 + cy) / scale);
                        let (bw, bh) = (bw / scale, bh / scale);
                        found.push(Found {
                            class: c as u8,
                            score: v,
                            x0: px - bw / 2.0,
                            y0: py - bh / 2.0,
                            x1: px + bw / 2.0,
                            y1: py + bh / 2.0,
                        });
                    }
                }
            }
            if x0 + tile >= sw {
                break;
            }
            tx += step;
        }
        if ty.min(sh.saturating_sub(tile)) + tile >= sh {
            break;
        }
        ty += step;
    }

    // One digit per place: the strongest of overlapping peaks (different digits)
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Found> = Vec::new();
    for f in found {
        let (cx, cy) = ((f.x0 + f.x1) / 2.0, (f.y0 + f.y1) / 2.0);
        let kind = |c: u8| c >= HEAD_BLACK;
        let clash = kept.iter().any(|k| {
            if kind(k.class) != kind(f.class) {
                return false;
            }
            let (kx, ky) = ((k.x0 + k.x1) / 2.0, (k.y0 + k.y1) / 2.0);
            (cx - kx).abs() < (k.x1 - k.x0) * 0.5 && (cy - ky).abs() < (k.y1 - k.y0) * 0.5
        });
        if !clash {
            kept.push(f);
        }
    }
    Some(kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The GPU network gives the CPU network's maps (skipped without a GPU)
    #[test]
    fn gpu_matches_cpu() {
        let Some(gpu) = gpu::Net::new(MODEL) else {
            return;
        };
        let det = detector().expect("detector");
        let (h, w) = (320, 448);
        // Strokes and blobs of ink
        let mut seed = 7u32;
        let page: Vec<f32> = (0..h * w)
            .map(|i| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let (y, x) = (i / w, i % w);
                ((y % 21 < 2) || (seed >> 28) == 0 || ((x / 9 + y / 13) % 7 == 0)) as u8 as f32
            })
            .collect();
        let out = gpu.run(&page, h, w).expect("gpu run");
        let input = NdTensor::from_data([1, 1, h, w], page);
        let cpu = det
            .model
            .run_n(
                [(det.input, input.view().into())].into(),
                [det.heat, det.size],
                None,
            )
            .expect("cpu run");
        for ((g, shape), c) in out.iter().zip(cpu) {
            let c: NdTensor<f32, 4> = c.try_into().unwrap();
            assert_eq!(&c.shape()[1..], shape);
            let diff = g
                .iter()
                .zip(c.to_vec())
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(diff < 1e-3, "max difference {diff}");
        }
    }
}
