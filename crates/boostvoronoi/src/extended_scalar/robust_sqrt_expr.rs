// SPDX-License-Identifier: BSL-1.0
// Provenance: ported
// Source: boostvoronoi@4060736209b06919a08590f37480d88b429599ba src/extended_scalar/robust_sqrt_expr.rs :: robust_sqrt_expr
// Changes: Vendored from boostvoronoi 0.12.1 (eadf's Rust port of Boost.Polygon's Voronoi, BSL-1.0);
// Changes: unchanged.

//! Used to compute expressions that operate with square roots with predefined
//! relative error. Evaluates expressions of the next type:
//! sum(i = 1 .. n)(A\[i\] * sqrt(B\[i\])), 1 <= n <= 4.

use crate::extended_scalar::extended_exp_fpt as ef;
use crate::extended_scalar::extended_int as ei;

#[allow(unused_imports)]
use crate::{t, tln};
use num_traits::Zero;

/// Evaluates expression (re = 4 EPS):
/// A\[0\] * sqrt(B\[0\]).
pub(crate) fn eval1(a: &[ei::ExtendedInt], b: &[ei::ExtendedInt]) -> ef::ExtendedExponentFpt {
    let a = ef::ExtendedExponentFpt::from(&a[0]);
    let b = ef::ExtendedExponentFpt::from(&b[0]);
    //tln!("eval1:");
    //tln!(" a:{:.0}", a.d());
    //tln!(" b:{:.0}", b.d());
    a * (b.sqrt())
}

// Evaluates expression (re = 7 EPS):
// A[0] * sqrt(B[0]) + A[1] * sqrt(B[1]).
pub fn eval2(a: &[ei::ExtendedInt], b: &[ei::ExtendedInt]) -> ef::ExtendedExponentFpt {
    let ra = eval1(a, b);
    let rb = eval1(&a[1..], &b[1..]);

    if ra.is_zero()
        || rb.is_zero()
        || (!ra.is_neg() && !rb.is_neg())
        || (!ra.is_pos() && !rb.is_pos())
    {
        return ra + rb;
    }

    let p = &a[0] * &a[0] * &b[0] - &a[1] * &a[1] * &b[1];
    let numer = ef::ExtendedExponentFpt::from(&p);
    let divisor = ra - rb;

    numer / divisor
}

/// Evaluates expression (re = 16 EPS):
/// A\[0\] * sqrt(B\[0\]) + A\[1\] * sqrt(B\[1\]) + A\[2\] * sqrt(B\[2\]).
pub fn eval3(a: &[ei::ExtendedInt], b: &[ei::ExtendedInt]) -> ef::ExtendedExponentFpt {
    let ra = eval2(a, b);
    let rb = eval1(&a[2..], &b[2..]);

    if ra.is_zero()
        || rb.is_zero()
        || (!ra.is_neg() && !rb.is_neg())
        || (!ra.is_pos() && !rb.is_pos())
    {
        return ra + rb;
    }
    let mut ta = [ei::ExtendedInt::zero(), ei::ExtendedInt::zero()];
    let mut tb = [ei::ExtendedInt::zero(), ei::ExtendedInt::zero()];

    ta[0] = &a[0] * &a[0] * &b[0] + &a[1] * &a[1] * &b[1] - &a[2] * &a[2] * &b[2];
    tb[0] = ei::ExtendedInt::from(1);
    ta[1] = &a[0] * &a[1] * &ei::ExtendedInt::from(2_i32);
    tb[1] = &b[0] * &b[1];

    let nom = eval2(&ta[..], &tb[..]);
    let div = ra - rb;
    nom / div
}

/// Evaluates expression (re = 25 EPS):
/// A\[0\] * sqrt(B\[0\]) + A\[1\] * sqrt(B\[1\]) +
/// A\[2\] * sqrt(B\[2\]) + A\[3\] * sqrt(B\[3\]).
pub fn eval4(a: &[ei::ExtendedInt], b: &[ei::ExtendedInt]) -> ef::ExtendedExponentFpt {
    let ra = eval2(a, b);
    let rb = eval2(&a[2..], &b[2..]);

    if ra.is_zero()
        || rb.is_zero()
        || (!ra.is_neg() && !rb.is_neg())
        || (!ra.is_pos() && !rb.is_pos())
    {
        return ra + rb;
    }
    let mut ta = [
        ei::ExtendedInt::zero(),
        ei::ExtendedInt::zero(),
        ei::ExtendedInt::zero(),
    ];
    let mut tb = [
        ei::ExtendedInt::zero(),
        ei::ExtendedInt::zero(),
        ei::ExtendedInt::zero(),
    ];

    ta[0] = &a[0] * &a[0] * &b[0] + &a[1] * &a[1] * &b[1]
        - &a[2] * &a[2] * &b[2]
        - &a[3] * &a[3] * &b[3];
    tb[0] = ei::ExtendedInt::from(1_i32);
    ta[1] = &a[0] * &a[1] * &ei::ExtendedInt::from(2_i32);
    tb[1] = &b[0] * &b[1];
    ta[2] = &a[2] * &a[3] * &ei::ExtendedInt::from(-2_i32);
    tb[2] = &b[2] * &b[3];
    eval3(&ta, &tb) / (ra - rb)
}

/// Evaluates A\[0] * sqrt(B\[0\]) + A\[1\] * sqrt(B\[1\]) +
///           A\[2] + A\[3\] * sqrt(B\[0\] * B\[1\]).
/// B\[3\] = B\[0\] * B\[1\].
#[allow(non_snake_case)]
pub(crate) fn sqrt_expr_evaluator_pss3(
    A: &[ei::ExtendedInt],
    B: &[ei::ExtendedInt],
) -> ef::ExtendedExponentFpt {
    let mut cA: [ei::ExtendedInt; 2] = [ei::ExtendedInt::zero(), ei::ExtendedInt::zero()];
    let mut cB: [ei::ExtendedInt; 2] = [ei::ExtendedInt::zero(), ei::ExtendedInt::zero()];

    let lh = eval2(A, B);
    let rh = eval2(&A[2..], &B[2..]);

    if lh.is_zero()
        || rh.is_zero()
        || (!lh.is_neg() && !rh.is_neg())
        || (!lh.is_pos() && !rh.is_pos())
    {
        return lh + rh;
    }
    cA[0] = &A[0] * &A[0] * &B[0] + &A[1] * &A[1] * &B[1]
        - &A[2] * &A[2]
        - &A[3] * &A[3] * &B[0] * &B[1];
    cB[0] = ei::ExtendedInt::from(1);
    cA[1] = (&A[0] * &A[1] - &A[2] * &A[3]) * &ei::ExtendedInt::from(2_i32);
    cB[1] = B[3].clone();
    let numer = eval2(&cA, &cB);
    let divisor = lh - rh;
    numer / divisor
}

/// Evaluates A\[3\] + A\[0\] * sqrt(B\[0\]) + A\[1\] * sqrt(B\[1\]) +
///           A\[2\] * sqrt(B\[3\] * (sqrt(B\[0\] * B\[1\]) + B\[2\])).
#[allow(non_snake_case)]
pub(crate) fn sqrt_expr_evaluator_pss4(
    A: &[ei::ExtendedInt],
    B: &[ei::ExtendedInt],
) -> ef::ExtendedExponentFpt {
    let mut cA: [ei::ExtendedInt; 4] = [
        ei::ExtendedInt::zero(),
        ei::ExtendedInt::zero(),
        ei::ExtendedInt::zero(),
        ei::ExtendedInt::zero(),
    ];
    let mut cB: [ei::ExtendedInt; 4] = [
        ei::ExtendedInt::zero(),
        ei::ExtendedInt::zero(),
        ei::ExtendedInt::zero(),
        ei::ExtendedInt::zero(),
    ];
    if A[3].is_zero() {
        let lh = eval2(A, B);
        cA[0] = ei::ExtendedInt::from(1);
        cB[0] = &B[0] * &B[1];
        cA[1] = B[2].clone();
        cB[1] = ei::ExtendedInt::from(1);
        let rh = eval1(&A[2..], &B[3..]) * eval2(&cA, &cB).sqrt();
        if lh.is_zero()
            || rh.is_zero()
            || (!lh.is_neg() && !rh.is_neg())
            || (!lh.is_pos() && !rh.is_pos())
        {
            return lh + rh;
        }
        cA[0] = &A[0] * &A[0] * &B[0] + &A[1] * &A[1] * &B[1] - &A[2] * &A[2] * &B[3] * &B[2];
        cB[0] = ei::ExtendedInt::from(1_i32);
        cA[1] = &A[0] * &A[1] * &ei::ExtendedInt::from(2_i32) - &A[2] * &A[2] * &B[3];
        cB[1] = &B[0] * &B[1];
        let numer = eval2(&cA, &cB);

        return numer / (lh - rh);
    }
    cA[0] = ei::ExtendedInt::from(1);
    cB[0] = &B[0] * &B[1];
    cA[1] = B[2].clone();
    cB[1] = ei::ExtendedInt::from(1);
    let rh = eval1(&A[2..], &B[3..]) * (eval2(&cA, &cB).sqrt());
    cA[0] = A[0].clone();
    cB[0] = B[0].clone();
    cA[1] = A[1].clone();
    cB[1] = B[1].clone();
    cA[2] = A[3].clone();
    cB[2] = ei::ExtendedInt::from(1);
    let lh = eval3(&cA, &cB);

    if lh.is_zero()
        || rh.is_zero()
        || (!lh.is_neg() && !rh.is_neg())
        || (!lh.is_pos() && !rh.is_pos())
    {
        return lh + rh;
    }
    cA[0] = &A[3] * &A[0] * &ei::ExtendedInt::from(2_i32);
    cA[1] = &A[3] * &A[1] * &ei::ExtendedInt::from(2_i32);
    cA[2] = &A[0] * &A[0] * &B[0] + &A[1] * &A[1] * &B[1] + &A[3] * &A[3]
        - &A[2] * &A[2] * &B[2] * &B[3];
    cA[3] = &A[0] * &A[1] * &ei::ExtendedInt::from(2_i32) - &A[2] * &A[2] * &B[3];
    cB[3] = &B[0] * &B[1];
    let numer = sqrt_expr_evaluator_pss3(&cA, &cB);

    numer / (lh - rh)
}

#[cfg(test)]
mod test {
    use crate::extended_scalar::extended_int as ei;
    use crate::extended_scalar::robust_fpt::RobustFpt;
    use num_traits::Zero;

    #[test]
    fn sqrt_1() {
        let a = RobustFpt::from(9.0f64);
        let b = a.sqrt();
        approx::assert_ulps_eq!(b.fpv(), 3.0);
        //assert_eq!(b.re(), 1.0 / 2.0 + 1.0, "a.re fail");
        let c = b * b;
        approx::assert_ulps_eq!(c.fpv(), 9.0);
        //assert_eq!(b.re(), (1.0 / 2.0 + 1.0) * 2.0, "b.re fail");
    }

    #[test]
    fn sqrt_2() {
        let mut ca: [ei::ExtendedInt; 5] = [
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
        ];
        let mut cb: [ei::ExtendedInt; 5] = [
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
        ];

        // Evaluates expression (re = 4 EPS):
        // A[0] * sqrt(B[0]).
        ca[0] = ei::ExtendedInt::from(2);
        cb[0] = ei::ExtendedInt::from(9);

        let a = super::eval1(&ca[..], &cb[..]);

        approx::assert_ulps_eq!(a.d(), 2.0 * 3.0);
        //assert_eq!(a.re(), 4.0, "a.re fail");
    }

    #[test]
    fn sqrt_3() {
        let mut ca: [ei::ExtendedInt; 5] = [
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
        ];
        let mut cb: [ei::ExtendedInt; 5] = [
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
        ];

        // Evaluates expression (re = 7 EPS):
        // A[0] * sqrt(B[0]) + A[1] * sqrt(B[1]).
        ca[0] = ei::ExtendedInt::from(3);
        cb[0] = ei::ExtendedInt::from(16);
        ca[1] = ei::ExtendedInt::from(2);
        cb[1] = ei::ExtendedInt::from(25);

        let a = super::eval2(&ca[..], &cb[..]);

        approx::assert_ulps_eq!(a.d(), 3.0 * 4.0 + 2.0 * 5.0);
        //assert_eq!(a.re(), 7.0, "a.re fail");
    }

    #[test]
    fn sqrt_4() {
        let mut ca: [ei::ExtendedInt; 5] = [
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
        ];
        let mut cb: [ei::ExtendedInt; 5] = [
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
            ei::ExtendedInt::zero(),
        ];

        // A[0] * sqrt(B[0]) + A[1] * sqrt(B[1]) + A[2] * sqrt(B[2]).

        ca[0] = ei::ExtendedInt::from(3);
        cb[0] = ei::ExtendedInt::from(16);
        ca[1] = ei::ExtendedInt::from(2);
        cb[1] = ei::ExtendedInt::from(25);
        ca[2] = ei::ExtendedInt::from(7);
        cb[2] = ei::ExtendedInt::from(49);

        let a = super::eval3(&ca[..], &cb[..]);

        approx::assert_ulps_eq!(a.d(), 3.0 * 4.0 + 2.0 * 5.0 + 7.0 * 7.0);
        //assert_eq!(a.re(), 7.0, "a.re fail");
    }

    #[test]
    fn sqrt_5() {
        let ca: [ei::ExtendedInt; 5] = [
            ei::ExtendedInt::from(3),
            ei::ExtendedInt::from(2),
            ei::ExtendedInt::from(7),
            ei::ExtendedInt::from(8),
            ei::ExtendedInt::zero(),
        ];
        let cb: [ei::ExtendedInt; 5] = [
            ei::ExtendedInt::from(16),
            ei::ExtendedInt::from(25),
            ei::ExtendedInt::from(49),
            ei::ExtendedInt::from(64),
            ei::ExtendedInt::zero(),
        ];

        // A[0] * sqrt(B[0]) + A[1] * sqrt(B[1]) +
        // A[2] * sqrt(B[2]) + A[3] * sqrt(B[3]).
        let a = super::eval4(&ca[..], &cb[..]);

        approx::assert_ulps_eq!(a.d(), 3.0 * 4.0 + 2.0 * 5.0 + 7.0 * 7.0 + 8.0 * 8.0);
    }

    #[test]
    fn sqrt_6() {
        let ca: [ei::ExtendedInt; 5] = [
            ei::ExtendedInt::from(20205600),
            ei::ExtendedInt::from(12),
            ei::ExtendedInt::from(1147151200i64),
            ei::ExtendedInt::from(-472),
            ei::ExtendedInt::zero(),
        ];
        let cb: [ei::ExtendedInt; 5] = [
            ei::ExtendedInt::from(1825),
            ei::ExtendedInt::from(6218073520360000i64),
            ei::ExtendedInt::from(1),
            ei::ExtendedInt::from(3407163572800i64),
            ei::ExtendedInt::zero(),
        ];

        let a = super::eval4(&ca[..], &cb[..]);
        approx::assert_ulps_eq!(a.d().floor(), 2085350584.0);
    }

    #[test]
    fn sqrt_7() {
        let ca: [ei::ExtendedInt; 5] = [
            ei::ExtendedInt::from(74125000i64),
            ei::ExtendedInt::from(17),
            ei::ExtendedInt::from(370703125i64),
            ei::ExtendedInt::from(-450),
            ei::ExtendedInt::zero(),
        ];
        let cb: [ei::ExtendedInt; 5] = [
            ei::ExtendedInt::from(1825),
            ei::ExtendedInt::from(0),
            ei::ExtendedInt::from(1),
            ei::ExtendedInt::from(0),
            ei::ExtendedInt::zero(),
        ];

        let a = super::eval4(&ca[..], &cb[..]);
        approx::assert_ulps_eq!(a.d().floor(), 3537324513.0);
    }
}
