//! Fingering digits found by a small trained network (see tools/train_digits.py).
//!
//! The network looks at a binarized page scaled to about 21 pixels between staff lines and
//! gives, at half resolution, one heat map per digit 1-5 (peaks at digit centers) and the
//! box size. It was trained on pages of public domain scores (PDMX) whose PDFs carry the
//! fingering as text, so it also learned what is not fingering: measure numbers, tuplet
//! marks, tempo marks.

use std::sync::OnceLock;

use rten::{Model, NodeId};
use rten_tensor::{AsView, Layout, NdTensor};

/// Staff line distance the network was trained at
const INTERLINE: f32 = 21.0;
const TILE: usize = 768;
const MARGIN: usize = 64;
const STRIDE: usize = 2;

/// A digit and its box in page pixels
#[derive(Debug, Clone, Copy)]
pub struct Found {
    pub digit: u8,
    pub score: f32,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

struct Detector {
    model: Model,
    input: NodeId,
    heat: NodeId,
    size: NodeId,
}

fn detector() -> Option<&'static Detector> {
    static DETECTOR: OnceLock<Option<Detector>> = OnceLock::new();
    DETECTOR
        .get_or_init(|| {
            let model = Model::load_static_slice(include_bytes!("digits.onnx"))
                .map_err(|e| log::warn!("Fingering detector: {e}"))
                .ok()?;
            Some(Detector {
                input: *model.input_ids().first()?,
                heat: model.node_id("heat").ok()?,
                size: model.node_id("size").ok()?,
                model,
            })
        })
        .as_ref()
}

/// Whether the trained detector can be used
pub fn available() -> bool {
    detector().is_some()
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

/// Digits on a black and white page (`ink`, row major, `w` x `h`) whose staff lines are
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
    let step = TILE - 2 * MARGIN;
    let mut ty = 0;
    loop {
        let mut tx = 0;
        loop {
            let (x0, y0) = (
                tx.min(sw.saturating_sub(TILE)),
                ty.min(sh.saturating_sub(TILE)),
            );
            let (tw, th) = (TILE.min(sw), TILE.min(sh));
            // Multiple of 16 for the network
            let (pw, ph) = (tw.div_ceil(16) * 16, th.div_ceil(16) * 16);
            let mut data = vec![0.0f32; pw * ph];
            for y in 0..th {
                let row = (y0 + y) * sw + x0;
                data[y * pw..y * pw + tw].copy_from_slice(&page[row..row + tw]);
            }
            let input = NdTensor::from_data([1, 1, ph, pw], data);
            let Ok([heat, size]) = det.model.run_n(
                [(det.input, input.view().into())].into(),
                [det.heat, det.size],
                None,
            ) else {
                return None;
            };
            let heat: NdTensor<f32, 4> = heat.try_into().ok()?;
            let size: NdTensor<f32, 4> = size.try_into().ok()?;
            let [_, _, hh, hw] = heat.shape();

            // The part of this tile that is not overlapped by its neighbours
            let keep_x0 = if x0 == 0 { 0 } else { MARGIN };
            let keep_y0 = if y0 == 0 { 0 } else { MARGIN };
            let keep_x1 = if x0 + TILE >= sw { tw } else { TILE - MARGIN };
            let keep_y1 = if y0 + TILE >= sh { th } else { TILE - MARGIN };

            for c in 0..5 {
                for i in 0..hh {
                    for j in 0..hw {
                        let v = heat[[0, c, i, j]];
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
                                    && heat[[0, c, ni as usize, nj as usize]] > v
                                {
                                    peak = false;
                                    break 'n;
                                }
                            }
                        }
                        if !peak {
                            continue;
                        }
                        let (cx, cy) = ((j * STRIDE) as f32 + 1.0, (i * STRIDE) as f32 + 1.0);
                        if cx < keep_x0 as f32
                            || cx >= keep_x1 as f32
                            || cy < keep_y0 as f32
                            || cy >= keep_y1 as f32
                        {
                            continue;
                        }
                        let bw = (size[[0, 0, i, j]] * 32.0).max(2.0);
                        let bh = (size[[0, 1, i, j]] * 32.0).max(2.0);
                        let (px, py) = ((x0 as f32 + cx) / scale, (y0 as f32 + cy) / scale);
                        let (bw, bh) = (bw / scale, bh / scale);
                        found.push(Found {
                            digit: c as u8 + 1,
                            score: v,
                            x0: px - bw / 2.0,
                            y0: py - bh / 2.0,
                            x1: px + bw / 2.0,
                            y1: py + bh / 2.0,
                        });
                    }
                }
            }
            if x0 + TILE >= sw {
                break;
            }
            tx += step;
        }
        if ty.min(sh.saturating_sub(TILE)) + TILE >= sh {
            break;
        }
        ty += step;
    }

    // One digit per place: the strongest of overlapping peaks (different digits)
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Found> = Vec::new();
    for f in found {
        let (cx, cy) = ((f.x0 + f.x1) / 2.0, (f.y0 + f.y1) / 2.0);
        let clash = kept.iter().any(|k| {
            let (kx, ky) = ((k.x0 + k.x1) / 2.0, (k.y0 + k.y1) / 2.0);
            (cx - kx).abs() < (k.x1 - k.x0) * 0.5 && (cy - ky).abs() < (k.y1 - k.y0) * 0.5
        });
        if !clash {
            kept.push(f);
        }
    }
    Some(kept)
}
