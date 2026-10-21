// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/TriangleSetSampling.cpp :: sample_its_uniform_parallel
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SeamRandom.cpp :: get_object_seams
// Changes: Independent implementation of the standard MT19937-64 engine used by mesh sampling; explicit real conversion for platform stability.

/// The standard 312-word MT19937-64 recurrence, with its standard seed expansion and tempering.
/// Kept local to seam sampling; no dependency on a platform's distribution implementation.
pub(super) struct Mt19937 {
    state: [u64; 312],
    index: usize,
}
impl Mt19937 {
    pub fn new(seed: u64) -> Self {
        let mut state = [0; 312];
        state[0] = seed;
        for i in 1..312 {
            state[i] = 6_364_136_223_846_793_005u64
                .wrapping_mul(state[i - 1] ^ (state[i - 1] >> 62))
                .wrapping_add(i as u64);
        }
        Self { state, index: 312 }
    }
    pub fn next(&mut self) -> u64 {
        if self.index == 312 {
            for i in 0..312 {
                let x = (self.state[i] & 0xffff_ffff_8000_0000)
                    | (self.state[(i + 1) % 312] & 0x7fff_ffff);
                self.state[i] = self.state[(i + 156) % 312]
                    ^ (x >> 1)
                    ^ if x & 1 != 0 { 0xb502_6f5a_a966_19e9 } else { 0 };
            }
            self.index = 0;
        }
        let mut x = self.state[self.index];
        self.index += 1;
        x ^= (x >> 29) & 0x5555_5555_5555_5555;
        x ^= (x << 17) & 0x71d6_7fff_eda6_0000;
        x ^= (x << 37) & 0xfff7_eee0_0000_0000;
        x ^= x >> 43;
        x
    }
    pub fn unit(&mut self) -> f64 {
        (self.next() as f64 / 18_446_744_073_709_551_616.0).min(1.0 - f64::EPSILON)
    }
}

/// Standard MT19937 used by SeamRandom, with a fixed two-word real conversion.
pub(super) struct Mt19937_32 {
    state: [u32; 624],
    index: usize,
}
impl Mt19937_32 {
    pub fn new(seed: u32) -> Self {
        let mut state = [0; 624];
        state[0] = seed;
        for i in 1..624 {
            state[i] = 1_812_433_253u32
                .wrapping_mul(state[i - 1] ^ (state[i - 1] >> 30))
                .wrapping_add(i as u32);
        }
        Self { state, index: 624 }
    }
    pub fn next(&mut self) -> u32 {
        if self.index == 624 {
            for i in 0..624 {
                let x = (self.state[i] & 0x8000_0000) | (self.state[(i + 1) % 624] & 0x7fff_ffff);
                self.state[i] = self.state[(i + 397) % 624]
                    ^ (x >> 1)
                    ^ if x & 1 != 0 { 0x9908_b0df } else { 0 };
            }
            self.index = 0;
        }
        let mut x = self.state[self.index];
        self.index += 1;
        x ^= x >> 11;
        x ^= (x << 7) & 0x9d2c_5680;
        x ^= (x << 15) & 0xefc6_0000;
        x ^= x >> 18;
        x
    }
    pub fn unit(&mut self) -> f64 {
        let lo = self.next() as f64;
        let hi = self.next() as f64;
        ((lo + hi * 4_294_967_296.0) / 18_446_744_073_709_551_616.0).min(1.0 - f64::EPSILON)
    }
}
