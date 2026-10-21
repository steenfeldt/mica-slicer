// SPDX-License-Identifier: BSL-1.0
// Provenance: ported
// Source: boostvoronoi@4060736209b06919a08590f37480d88b429599ba src/extended_scalar/extended_int.rs :: ExtendedInt
// Changes: Fixed-capacity word storage (an `ArrayVec` of 64 words, like Boost's `extended_int<64>`) instead
// Changes: of a growable SmallVec: no heap use, no spilled/inline branch per word, clones copy only the
// Changes: used words. The operations are the same exact integer arithmetic (add/dif/mul as in Boost's
// Changes: voronoi_ctypes.hpp, including its single leading-zero-word convention); `is_zero` implemented.

// Boost.Polygon library detail/voronoi_structures.hpp header file

//          Copyright Andrii Sydorchuk 2010-2012.
// Distributed under the Boost Software License, Version 1.0.
//    (See accompanying file LICENSE_1_0.txt or copy at
//          http://www.boost.org/LICENSE_1_0.txt)

// See http://www.boost.org for updates, documentation, and revision history of C++ code..

// Ported from C++ boost 1.76.0 to Rust in 2020/2021 by Eadf (github.com/eadf)

//! Utilities for big integers. Supports next set of arithmetic operations: +, -, *.

use crate::cast;
use arrayvec::ArrayVec;
use num_traits::PrimInt as InputType;
use num_traits::{One, Zero};
use std::cmp;
use std::fmt;
use std::ops;

#[cfg(test)]
mod extendedint_tests;

/// Capacity in 32-bit words, as Boost's `extended_int<64>` (2048 bits). The predicates' largest
/// expressions stay far below this; like Boost, a longer result is truncated to the capacity.
const WORDS: usize = 64;

/// Stack allocated big integer class.
/// Supports next set of arithmetic operations: +, -, *.
/// Ported from voronoi_ctypes.hpp
///
/// The magnitude lives in little-endian 32-bit words `chunks_` (always `size()` of them); `count_`
/// carries the sign and the number of words.
#[derive(Clone)]
pub struct ExtendedInt {
    chunks_: ArrayVec<u32, WORDS>,
    count_: i32,
}

impl<I: InputType> From<I> for ExtendedInt {
    #[inline]
    ///```
    /// # use boostvoronoi::extended_scalar::extended_int::ExtendedInt;
    ///
    /// let aa = 41231332_f64;
    /// let a = ExtendedInt::from(aa as i32);
    /// approx::assert_ulps_eq!(a.d(), aa);
    /// ```
    fn from(that: I) -> Self {
        let that = cast::<I, i64>(that);
        let mut rv = Self::zero();
        if that != 0 {
            let c = that.unsigned_abs();
            rv.chunks_.push(c as u32);
            let high = (c >> 32) as u32;
            rv.count_ = if high != 0 {
                rv.chunks_.push(high);
                2
            } else {
                1
            };
            if that < 0 {
                rv.count_ = -rv.count_;
            }
        }
        rv
    }
}

impl One for ExtendedInt {
    #[inline]
    fn one() -> Self {
        Self::from(1_i32)
    }
}

impl Zero for ExtendedInt {
    #[inline]
    fn zero() -> Self {
        Self {
            chunks_: ArrayVec::new(),
            count_: 0,
        }
    }
    #[inline]
    fn is_zero(&self) -> bool {
        self.count_ == 0
    }
}

impl ExtendedInt {
    /// Return the mantissa and exponent components of this integer.
    /// `value` ≈ `mantissa` * 2^`exponent`
    pub fn p(&self) -> (f64, i32) {
        let sep = 0x100000000_u64 as f64;
        let mut rv = (0.0, 0);
        let size = self.size();
        match size {
            0 => return rv,
            1 => {
                rv.0 = f64::from(self.chunks_[0]);
            }
            2 => {
                rv.0 = f64::from(self.chunks_[1]) * sep + f64::from(self.chunks_[0]);
            }
            _ => {
                for v in self.chunks_.iter().rev().take(3) {
                    rv.0 *= sep;
                    rv.0 += f64::from(*v);
                }
                rv.1 = ((size - 3) << 5) as i32;
            }
        }
        if self.count_ < 0 {
            rv.0 = -rv.0;
        }
        rv
    }

    #[inline(always)]
    pub fn is_pos(&self) -> bool {
        self.count_ > 0
    }

    #[inline(always)]
    pub fn is_neg(&self) -> bool {
        self.count_ < 0
    }

    #[inline(always)]
    pub fn is_zero(&self) -> bool {
        self.count_ == 0
    }

    #[inline(always)]
    /// converts to f64
    pub fn d(&self) -> f64 {
        let p = self.p();
        libm::ldexp(p.0, p.1)
    }

    #[inline(always)]
    /// return the number of words in 'self.count'
    pub fn size(&self) -> usize {
        self.chunks_.len()
    }

    /// this method assumes self is an empty object
    fn add_others(&mut self, e1: &Self, e2: &Self) {
        if e1.count_ == 0 {
            *self = e2.clone();
            return;
        }
        if e2.count_ == 0 {
            *self = e1.clone();
            return;
        }
        if (e1.count_ > 0) ^ (e2.count_ > 0) {
            self.dif_slice(&e1.chunks_, &e2.chunks_, false);
        } else {
            self.add_slice(&e1.chunks_, &e2.chunks_);
        }
        if e1.count_ < 0 {
            self.count_ = -self.count_;
        }
    }

    /// Boost's `extended_int::add(c1, sz1, c2, sz2)` on the magnitudes; `self` must be empty.
    fn add_slice(&mut self, c1: &[u32], c2: &[u32]) {
        if c1.len() < c2.len() {
            self.add_slice(c2, c1);
            return;
        }
        let (sz1, sz2) = (c1.len(), c2.len());
        self.count_ = sz1 as i32;
        let mut temp = 0_u64;
        for (a, b) in c1[..sz2].iter().zip(c2) {
            temp += u64::from(*a) + u64::from(*b);
            self.chunks_.push(temp as u32);
            temp >>= 32;
        }
        for a in &c1[sz2..] {
            temp += u64::from(*a);
            self.chunks_.push(temp as u32);
            temp >>= 32;
        }
        if temp != 0 && sz1 != WORDS {
            self.chunks_.push(temp as u32);
            self.count_ += 1;
        }
    }

    /// this method assumes self is an empty object
    fn dif_other(&mut self, e1: &Self, e2: &Self) {
        if e1.count_ == 0 {
            *self = e2.clone();
            self.count_ = -self.count_;
            return;
        }
        if e2.count_ == 0 {
            *self = e1.clone();
            return;
        }
        if (e1.count_ > 0) ^ (e2.count_ > 0) {
            self.add_slice(&e1.chunks_, &e2.chunks_);
        } else {
            self.dif_slice(&e1.chunks_, &e2.chunks_, false);
        }
        if e1.count_ < 0 {
            self.count_ = -self.count_;
        }
    }

    /// Boost's `extended_int::dif(c1, sz1, c2, sz2, rec)` on the magnitudes; `self` must be empty.
    /// As in Boost, only one leading zero word is dropped from the difference.
    fn dif_slice(&mut self, c1: &[u32], c2: &[u32], rec: bool) {
        let (mut c1, mut c2) = (c1, c2);
        if c1.len() < c2.len() {
            self.dif_slice(c2, c1, true);
            self.count_ = -self.count_;
            return;
        } else if c1.len() == c2.len() && !rec {
            // Equal lengths: compare from the top to find which magnitude is larger.
            let mut sz = c1.len();
            loop {
                sz -= 1;
                match c1[sz].cmp(&c2[sz]) {
                    cmp::Ordering::Less => {
                        self.dif_slice(&c2[..sz + 1], &c1[..sz + 1], true);
                        self.count_ = -self.count_;
                        return;
                    }
                    cmp::Ordering::Greater => {
                        sz += 1;
                        break;
                    }
                    cmp::Ordering::Equal => (),
                }
                if sz == 0 {
                    break;
                }
            }
            if sz == 0 {
                self.count_ = 0;
                return;
            }
            c1 = &c1[..sz];
            c2 = &c2[..sz];
        }
        let sz1 = c1.len();
        let mut flag = false;
        for (a, b) in c1.iter().zip(c2) {
            self.chunks_
                .push(a.wrapping_sub(*b).wrapping_sub(u32::from(flag)));
            flag = (a < b) || ((a == b) && flag);
        }
        for a in &c1[c2.len()..] {
            self.chunks_.push(a.wrapping_sub(u32::from(flag)));
            flag = (*a == 0) && flag;
        }
        self.count_ = sz1 as i32;
        if self.chunks_[sz1 - 1] == 0 {
            let _ = self.chunks_.pop();
            self.count_ -= 1;
        }
    }

    fn mul_other(&mut self, e1: &Self, e2: &Self) {
        if e1.count_ == 0 || e2.count_ == 0 {
            self.count_ = 0;
            return;
        }
        self.mul_slice(&e1.chunks_, &e2.chunks_);
        if (e1.count_ > 0) ^ (e2.count_ > 0) {
            self.count_ = -self.count_;
        }
    }

    /// Boost's `extended_int::mul(c1, sz1, c2, sz2)` on the magnitudes; `self` must be empty.
    fn mul_slice(&mut self, c1: &[u32], c2: &[u32]) {
        let (sz1, sz2) = (c1.len(), c2.len());
        let mut cur: u64 = 0;
        let count = cmp::min(WORDS, sz1 + sz2 - 1);
        self.count_ = count as i32;
        for shift in 0..count {
            let mut nxt: u64 = 0;
            // c1[first] * c2[shift - first] for every pair of words inside both operands.
            let first_begin = shift.saturating_sub(sz2 - 1);
            let first_end = cmp::min(shift, sz1 - 1);
            for first in first_begin..=first_end {
                let tmp = u64::from(c1[first]) * u64::from(c2[shift - first]);
                cur += tmp & 0xFFFF_FFFF;
                nxt += tmp >> 32;
            }
            self.chunks_.push(cur as u32);
            cur = nxt + (cur >> 32);
        }
        if cur != 0 && count != WORDS {
            self.chunks_.push(cur as u32);
            self.count_ += 1;
        }
    }
}

impl Default for ExtendedInt {
    fn default() -> Self {
        Self::zero()
    }
}

impl ops::Add for ExtendedInt {
    type Output = Self;
    /// Adds `self` to `that` returning a new object containing the result
    /// ```
    /// # use boostvoronoi::extended_scalar::extended_int::ExtendedInt;
    ///
    /// let aa = 472_f64;
    /// let bb = 147_f64;
    /// let a = ExtendedInt::from(aa as i64);
    /// let b = ExtendedInt::from(bb as i64);
    /// let c = a+b;
    /// approx::assert_ulps_eq!(c.d(), aa+bb);
    ///```
    fn add(self, that: Self) -> Self {
        let mut rv = ExtendedInt::default();
        rv.add_others(&self, &that);
        rv
    }
}

impl<'b> ops::Add<&'b ExtendedInt> for &ExtendedInt {
    type Output = ExtendedInt;
    /// Adds `self` to `that` returning a new object containing the result
    /// ```
    /// # use boostvoronoi::extended_scalar::extended_int::ExtendedInt;
    ///
    /// let aa = 472_f64;
    /// let bb = 147_f64;
    /// let a = ExtendedInt::from(aa as i64);
    /// let b = ExtendedInt::from(bb as i64);
    /// let c = &a+&b;
    /// approx::assert_ulps_eq!(c.d(), aa+bb);
    ///```
    fn add(self, that: &'b ExtendedInt) -> ExtendedInt {
        let mut rv = ExtendedInt::default();
        rv.add_others(self, that);
        rv
    }
}

impl<'b> ops::Add<&'b ExtendedInt> for ExtendedInt {
    type Output = ExtendedInt;
    /// Adds `self` to `that` returning a new object containing the result
    /// ```
    /// # use boostvoronoi::extended_scalar::extended_int::ExtendedInt;
    ///
    /// let aa = 472_f64;
    /// let bb = 147_f64;
    /// let a = ExtendedInt::from(aa as i64);
    /// let b = ExtendedInt::from(bb as i64);
    /// let c = a+&b;
    /// approx::assert_ulps_eq!(c.d(), aa+bb);
    ///```
    fn add(self, that: &'b ExtendedInt) -> ExtendedInt {
        let mut rv = ExtendedInt::default();
        rv.add_others(&self, that);
        rv
    }
}

impl ops::Sub for ExtendedInt {
    type Output = Self;
    /// Subtracts `that` from `self` returning a new object containing the result
    /// ```
    /// # use boostvoronoi::extended_scalar::extended_int::ExtendedInt;
    ///
    /// let aa = 4727377593577731_f64;
    /// let bb = 759935777381_f64;
    /// let a = ExtendedInt::from(aa as i64);
    /// let b = ExtendedInt::from(bb as i64);
    /// let c = a-b;
    /// approx::assert_ulps_eq!(c.d(), aa-bb);
    ///```
    fn sub(self, that: Self) -> Self {
        let mut rv = ExtendedInt::default();
        rv.dif_other(&self, &that);
        rv
    }
}

impl<'b> ops::Sub<&'b ExtendedInt> for &ExtendedInt {
    type Output = ExtendedInt;
    /// Subtracts `that` from `self` returning a new object containing the result
    /// ```
    /// # use boostvoronoi::extended_scalar::extended_int::ExtendedInt;
    ///
    /// let aa = 4727377593577731_f64;
    /// let bb = 759935777381_f64;
    /// let a = ExtendedInt::from(aa as i64);
    /// let b = ExtendedInt::from(bb as i64);
    /// let c = &a-&b;
    /// approx::assert_ulps_eq!(c.d(), aa-bb);
    ///```
    fn sub(self, that: &'b ExtendedInt) -> ExtendedInt {
        let mut rv = ExtendedInt::default();
        rv.dif_other(self, that);
        rv
    }
}

impl<'b> ops::Sub<&'b ExtendedInt> for ExtendedInt {
    type Output = ExtendedInt;
    /// Subtracts `that` from `self` returning a new object containing the result
    /// ```
    /// # use boostvoronoi::extended_scalar::extended_int::ExtendedInt;
    ///
    /// let aa = 4727377593577731_f64;
    /// let bb = 759935777381_f64;
    /// let a = ExtendedInt::from(aa as i64);
    /// let b = ExtendedInt::from(bb as i64);
    /// let c = a-&b;
    /// approx::assert_ulps_eq!(c.d(), aa-bb);
    ///```
    fn sub(self, that: &'b ExtendedInt) -> ExtendedInt {
        let mut rv = ExtendedInt::default();
        rv.dif_other(&self, that);
        rv
    }
}

impl ops::Mul for ExtendedInt {
    type Output = Self;
    /// Multiplies `self` with `that` returning a new object containing the result
    /// ```
    /// # use boostvoronoi::extended_scalar::extended_int::ExtendedInt;
    ///
    /// let aa = 4727377593577731_f64;
    /// let bb = 759935777381_f64;
    /// let a = ExtendedInt::from(aa as i64);
    /// let b = ExtendedInt::from(bb as i64);
    /// let c = a*b;
    /// approx::assert_ulps_eq!(c.d(), aa*bb);
    ///```
    fn mul(self, that: Self) -> Self {
        let mut rv = ExtendedInt::default();
        rv.mul_other(&self, &that);
        rv
    }
}

impl<'b> ops::Mul<&'b ExtendedInt> for &ExtendedInt {
    type Output = ExtendedInt;
    /// Multiplies `self` with `that` returning a new object containing the result
    /// ```
    /// # use boostvoronoi::extended_scalar::extended_int::ExtendedInt;
    ///
    /// let aa = 4727377593577731_f64;
    /// let bb = 759935777381_f64;
    /// let a = ExtendedInt::from(aa as i64);
    /// let b = ExtendedInt::from(bb as i64);
    /// let c = &a*&b;
    /// approx::assert_ulps_eq!(c.d(), aa*bb);
    ///```
    fn mul(self, that: &'b ExtendedInt) -> ExtendedInt {
        let mut rv = ExtendedInt::default();
        rv.mul_other(self, that);
        rv
    }
}

impl<'b> ops::Mul<&'b ExtendedInt> for ExtendedInt {
    type Output = ExtendedInt;
    /// Multiplies `self` with `that` returning a new object containing the result
    /// ```
    /// # use boostvoronoi::extended_scalar::extended_int::ExtendedInt;
    ///
    /// let aa = 4727377593577731_f64;
    /// let bb = 759935777381_f64;
    /// let a = ExtendedInt::from(aa as i64);
    /// let b = ExtendedInt::from(bb as i64);
    /// let c = a*&b;
    /// approx::assert_ulps_eq!(c.d(), aa*bb);
    ///```
    fn mul(self, that: &'b ExtendedInt) -> ExtendedInt {
        let mut rv = ExtendedInt::default();
        rv.mul_other(&self, that);
        rv
    }
}

impl ops::Mul<i32> for ExtendedInt {
    type Output = ExtendedInt;
    /// Multiplies `self` with `that` returning a new object containing the result
    /// ```
    /// # use boostvoronoi::extended_scalar::extended_int::ExtendedInt;
    ///
    /// let aa = 4727377593577731_f64;
    /// let bb = 759935_f64;
    /// let a = ExtendedInt::from(aa as i64);
    /// let b = bb as i32;
    /// let c = a*b;
    /// approx::assert_ulps_eq!(c.d(), aa*bb);
    ///```
    fn mul(self, that: i32) -> ExtendedInt {
        let mut rv = ExtendedInt::default();
        let that = ExtendedInt::from(that);
        rv.mul_other(&self, &that);
        rv
    }
}

impl ops::Neg for ExtendedInt {
    type Output = Self;
    /// Negates the value of `self`
    /// ```
    /// # use boostvoronoi::extended_scalar::extended_int::ExtendedInt;
    ///
    /// let aa = 4727377593577731_f64;
    /// let a = -ExtendedInt::from(aa as i64);
    /// approx::assert_ulps_eq!(a.d(), -aa);
    ///```
    fn neg(mut self) -> Self {
        self.count_ = -self.count_;
        self
    }
}

impl fmt::Debug for ExtendedInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.0}", self.d())
    }
}
