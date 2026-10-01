//! Matching the heads found on the pages with the notes of the score, staff by staff

/// How alike a found position (half staff spaces from the bottom line) and a written one are
fn position_similarity(found: f32, written: i32) -> f32 {
    let d = (found - written as f32).abs();
    if d <= 0.5 {
        1.0
    } else if d <= 1.5 {
        // A head read one step off
        0.5
    } else if (d - 7.0).abs() <= 0.5 {
        // An octave sign missed or misplaced
        0.3
    } else {
        0.0
    }
}

/// 0..1: the best pairing of two chords' positions, relative to the larger chord
pub fn chord_similarity(found: &[f32], written: &[i32]) -> f32 {
    let mut used = vec![false; written.len()];
    let mut total = 0.0;
    for &f in found {
        let best = (0..written.len())
            .filter(|&j| !used[j])
            .map(|j| (j, position_similarity(f, written[j])))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((j, s)) = best
            && s > 0.0
        {
            used[j] = true;
            total += s;
        }
    }
    total / found.len().max(written.len()).max(1) as f32
}

/// For the found positions of a chord, the index of the written note each one is (by
/// position, equal positions in order)
pub fn pair_notes(found: &[f32], written: &[i32]) -> Vec<Option<usize>> {
    let mut used = vec![false; written.len()];
    let mut order: Vec<usize> = (0..found.len()).collect();
    // Best fits first
    order.sort_by(|&a, &b| {
        let best = |i: usize| {
            written
                .iter()
                .map(|&w| position_similarity(found[i], w))
                .fold(0.0, f32::max)
        };
        best(b).total_cmp(&best(a))
    });
    let mut out = vec![None; found.len()];
    for i in order {
        let pick = (0..written.len())
            .filter(|&j| !used[j])
            .map(|j| {
                (
                    j,
                    position_similarity(found[i], written[j]),
                    (found[i] - written[j] as f32).abs(),
                )
            })
            .filter(|c| c.1 > 0.0)
            .max_by(|a, b| a.1.total_cmp(&b.1).then(b.2.total_cmp(&a.2)));
        if let Some((j, _, _)) = pick {
            used[j] = true;
            out[i] = Some(j);
        }
    }
    out
}

/// Needleman-Wunsch within a band around the diagonal: (found chord, written chord) pairs
pub fn align(found: &[Vec<f32>], written: &[Vec<i32>]) -> Vec<(usize, usize)> {
    const GAP: f32 = -0.3;
    let (n, m) = (found.len(), written.len());
    if n == 0 || m == 0 {
        return Vec::new();
    }
    let band = ((n.max(m) as f32 * 0.15) as usize).max(200);
    // Column range of row i
    let center = |i: usize| (i as f64 * m as f64 / n as f64) as i64;
    let lo = |i: usize| (center(i) - band as i64).max(0) as usize;
    let hi = |i: usize| ((center(i) + band as i64) as usize).min(m);
    let width = 2 * band + 2;
    let neg = f32::MIN / 4.0;
    let mut score = vec![neg; (n + 1) * width];
    let mut from = vec![0u8; (n + 1) * width];
    let at = |i: usize, j: usize| -> Option<usize> {
        let l = if i == 0 { 0 } else { lo(i) };
        let h = if i == 0 { m.min(band) } else { hi(i) };
        (j >= l && j <= h).then(|| i * width + (j - l))
    };
    for j in 0..=m.min(band) {
        score[at(0, j).unwrap()] = GAP * j as f32;
        from[at(0, j).unwrap()] = 2;
    }
    for i in 1..=n {
        for j in lo(i)..=hi(i) {
            let idx = at(i, j).unwrap();
            let mut best = neg;
            let mut dir = 0u8;
            if j > 0
                && let Some(d) = at(i - 1, j - 1)
            {
                let s = score[d] + chord_similarity(&found[i - 1], &written[j - 1]) * 2.0 - 0.8;
                if s > best {
                    best = s;
                    dir = 0;
                }
            }
            if let Some(u) = at(i - 1, j) {
                let s = score[u] + GAP;
                if s > best {
                    best = s;
                    dir = 1;
                }
            }
            if j > 0
                && let Some(l) = at(i, j - 1)
            {
                let s = score[l] + GAP;
                if s > best {
                    best = s;
                    dir = 2;
                }
            }
            score[idx] = best;
            from[idx] = dir;
        }
    }
    // Trace back from the end (or the best cell of the last row inside the band)
    let mut i = n;
    let mut j = hi(n);
    let mut pairs = Vec::new();
    while i > 0 && j > 0 {
        let Some(idx) = at(i, j) else {
            break;
        };
        match from[idx] {
            0 => {
                pairs.push((i - 1, j - 1));
                i -= 1;
                j -= 1;
            }
            1 => i -= 1,
            _ => j -= 1,
        }
    }
    pairs.reverse();
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligns_around_missing_and_extra_chords() {
        let written: Vec<Vec<i32>> = vec![vec![0], vec![2], vec![4, 8], vec![5], vec![7], vec![9]];
        // The third chord read a step off, the fourth missing, an extra one before the last
        let found: Vec<Vec<f32>> = vec![
            vec![0.1],
            vec![2.0],
            vec![3.9, 7.2],
            vec![7.0],
            vec![12.0],
            vec![9.0],
        ];
        let pairs = align(&found, &written);
        assert!(pairs.contains(&(0, 0)));
        assert!(pairs.contains(&(2, 2)));
        assert!(pairs.contains(&(3, 4)));
        assert!(pairs.contains(&(5, 5)));
        assert_eq!(pair_notes(&[3.9, 7.2], &[4, 8]), vec![Some(0), Some(1)]);
    }
}
