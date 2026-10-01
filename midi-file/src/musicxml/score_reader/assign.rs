//! Minimum cost assignment (Hungarian method, shortest augmenting paths), for giving the
//! digits of a page to notes all at once instead of one by one

/// For a cost matrix of `rows` x `cols` (rows <= cols, row major), the column of each row
/// with the least total cost
pub fn assign(cost: &[f64], rows: usize, cols: usize) -> Vec<usize> {
    assert!(rows <= cols && cost.len() == rows * cols);
    let c = |i: usize, j: usize| cost[(i - 1) * cols + (j - 1)];
    // 1-based potentials and matching as in the classic formulation; column 0 is a sentinel
    let mut u = vec![0.0f64; rows + 1];
    let mut v = vec![0.0f64; cols + 1];
    let mut row_of = vec![0usize; cols + 1];
    let mut way = vec![0usize; cols + 1];
    for i in 1..=rows {
        row_of[0] = i;
        let mut j0 = 0;
        let mut min = vec![f64::INFINITY; cols + 1];
        let mut done = vec![false; cols + 1];
        loop {
            done[j0] = true;
            let i0 = row_of[j0];
            let mut delta = f64::INFINITY;
            let mut j1 = 0;
            for j in 1..=cols {
                if !done[j] {
                    let cur = c(i0, j) - u[i0] - v[j];
                    if cur < min[j] {
                        min[j] = cur;
                        way[j] = j0;
                    }
                    if min[j] < delta {
                        delta = min[j];
                        j1 = j;
                    }
                }
            }
            for j in 0..=cols {
                if done[j] {
                    u[row_of[j]] += delta;
                    v[j] -= delta;
                } else {
                    min[j] -= delta;
                }
            }
            j0 = j1;
            if row_of[j0] == 0 {
                break;
            }
        }
        // Flip the augmenting path
        loop {
            let j1 = way[j0];
            row_of[j0] = row_of[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
    }
    let mut out = vec![0; rows];
    for j in 1..=cols {
        if row_of[j] > 0 {
            out[row_of[j] - 1] = j - 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn least_total_cost() {
        // Greedy (row 0 takes its best, column 0) would cost 1 + 10; the best is 2 + 3
        let cost = [1.0, 2.0, 9.0, 3.0, 10.0, 9.0];
        assert_eq!(assign(&cost, 2, 3), vec![1, 0]);
        // Square
        let cost = [4.0, 1.0, 3.0, 2.0, 0.0, 5.0, 3.0, 2.0, 2.0];
        let a = assign(&cost, 3, 3);
        let total: f64 = a.iter().enumerate().map(|(i, &j)| cost[i * 3 + j]).sum();
        assert_eq!(total, 5.0);
    }
}
