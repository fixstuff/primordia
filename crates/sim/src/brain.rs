//! The fast-loop controller: a one-hidden-layer MLP, tanh throughout.
//! Weight layout: hidden rows `[h][n_in]`, then output rows `[o][hidden + 1]`
//! with the output bias last. No allocation.

use crate::det;
use crate::genome::{MAX_HIDDEN, N_OUT};

pub fn forward(w: &[f32], inputs: &[f32], hidden: usize) -> [f32; N_OUT] {
    let n_in = inputs.len();
    let mut h = [0f32; MAX_HIDDEN];
    for (j, hj) in h.iter_mut().enumerate().take(hidden) {
        let row = &w[j * n_in..(j + 1) * n_in];
        let mut s = 0.0f32;
        for i in 0..n_in {
            s += row[i] * inputs[i];
        }
        *hj = det::tanh(s);
    }
    let base = hidden * n_in;
    let mut out = [0f32; N_OUT];
    for (k, ok) in out.iter_mut().enumerate() {
        let row = &w[base + k * (hidden + 1)..base + (k + 1) * (hidden + 1)];
        let mut s = row[hidden];
        for j in 0..hidden {
            s += row[j] * h[j];
        }
        *ok = det::tanh(s);
    }
    out
}
