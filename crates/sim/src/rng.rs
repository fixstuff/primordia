//! PCG32 (O'Neill, pcg-random.org, "XSH RR" output). Written here rather
//! than pulled from a crate so its exact sequence is part of this repo and
//! can never change under us with a dependency bump.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pcg32 {
    state: u64,
    inc: u64,
}

impl Pcg32 {
    pub fn new(seed: u64, stream: u64) -> Self {
        let mut r = Pcg32 { state: 0, inc: (stream << 1) | 1 };
        r.next_u32();
        r.state = r.state.wrapping_add(seed);
        r.next_u32();
        r
    }

    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Uniform in [0, 1), 24 bits of precision (exact in f32).
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (1.0 / 16_777_216.0)
    }

    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }

    /// Uniform integer in [0, n). Lemire's multiply-shift (tiny bias, fine here).
    pub fn below(&mut self, n: u32) -> u32 {
        ((self.next_u32() as u64 * n as u64) >> 32) as u32
    }

    pub fn state_bytes(&self) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[..8].copy_from_slice(&self.state.to_le_bytes());
        b[8..].copy_from_slice(&self.inc.to_le_bytes());
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference values from the PCG reference implementation's demo
    /// (pcg32-demo: seed 42, stream 54): the first six outputs.
    #[test]
    fn matches_reference_sequence() {
        let mut r = Pcg32::new(42, 54);
        let got: Vec<u32> = (0..6).map(|_| r.next_u32()).collect();
        assert_eq!(
            got,
            vec![0xa15c02b7, 0x7b47f409, 0xba1d3330, 0x83d2f293, 0xbfa4784b, 0xcbed606e]
        );
    }
}
