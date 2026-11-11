//! The chemical field: a toroidal grid, one f32 per cell per chemical.
//! Explicit 5-point diffusion, stable for rate <= 0.25.

use crate::det;

pub const NCHEM: usize = 2;

#[derive(Clone, Debug)]
pub struct Grid {
    pub w: usize,
    pub h: usize,
    /// World units per cell.
    pub cell: f32,
    pub chem: [Vec<f32>; NCHEM],
    scratch: Vec<f32>,
}

impl Grid {
    pub fn new(w: usize, h: usize, cell: f32) -> Grid {
        Grid {
            w,
            h,
            cell,
            chem: [vec![0.0; w * h], vec![0.0; w * h]],
            scratch: vec![0.0; w * h],
        }
    }

    pub fn world_w(&self) -> f32 {
        self.w as f32 * self.cell
    }
    pub fn world_h(&self) -> f32 {
        self.h as f32 * self.cell
    }

    /// Cell index of a world position; positions off the torus wrap.
    #[inline]
    pub fn index(&self, x: f32, y: f32) -> usize {
        let cx = (det::floor(x / self.cell) as i64).rem_euclid(self.w as i64) as usize;
        let cy = (det::floor(y / self.cell) as i64).rem_euclid(self.h as i64) as usize;
        cy * self.w + cx
    }

    #[inline]
    pub fn sample(&self, x: f32, y: f32, c: usize) -> f32 {
        self.chem[c][self.index(x, y)]
    }

    /// Remove up to `want` from the cell; returns what was taken.
    #[inline]
    pub fn take(&mut self, x: f32, y: f32, c: usize, want: f32) -> f32 {
        let i = self.index(x, y);
        let got = det::clamp(want, 0.0, self.chem[c][i]);
        self.chem[c][i] -= got;
        got
    }

    #[inline]
    pub fn add(&mut self, x: f32, y: f32, c: usize, amount: f32) {
        let i = self.index(x, y);
        self.chem[c][i] += amount;
    }

    pub fn diffuse(&mut self, rate: f32, decay: f32) {
        let (w, h) = (self.w, self.h);
        let keep = 1.0 - decay;
        for c in 0..NCHEM {
            let src = &self.chem[c];
            let dst = &mut self.scratch;
            for y in 0..h {
                let up = if y == 0 { h - 1 } else { y - 1 } * w;
                let dn = if y == h - 1 { 0 } else { y + 1 } * w;
                let row = y * w;
                for x in 0..w {
                    let l = if x == 0 { w - 1 } else { x - 1 };
                    let r = if x == w - 1 { 0 } else { x + 1 };
                    let v = src[row + x];
                    let lap = src[row + l] + src[row + r] + src[up + x] + src[dn + x] - 4.0 * v;
                    dst[row + x] = (v + rate * lap) * keep;
                }
            }
            std::mem::swap(&mut self.chem[c], &mut self.scratch);
        }
    }

    pub fn total(&self, c: usize) -> f64 {
        self.chem[c].iter().map(|&v| v as f64).sum()
    }
}
