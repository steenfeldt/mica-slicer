// SPDX-License-Identifier: BSL-1.0
// Provenance: ported
// Source: boostvoronoi@4060736209b06919a08590f37480d88b429599ba src/predicate/circle_existence.rs :: circle_existence
// Changes: Vendored from boostvoronoi 0.12.1 (eadf's Rust port of Boost.Polygon's Voronoi, BSL-1.0);
// Changes: geo-based validate_circle_formation removed.
// Boost.Polygon library detail/voronoi_predicates.hpp header file

//          Copyright Andrii Sydorchuk 2010-2012.
// Distributed under the Boost Software License, Version 1.0.
//    (See accompanying file LICENSE_1_0.txt or copy at
//          http://www.boost.org/LICENSE_1_0.txt)

// See http://www.boost.org for updates, documentation, and revision history of C++ code.

// Ported from C++ boost 1.76.0 to Rust in 2020/2021 by Eadf (github.com/eadf)

use crate::predicate::orientation_predicate::{self, Orientation};
use crate::site_event as vse;
use crate::{InputType, geometry::Point, predicate::SiteIndex};

#[inline(always)]
pub(crate) fn ppp<I: InputType>(point1: Point<I>, point2: Point<I>, point3: Point<I>) -> bool {
    orientation_predicate::eval_p::<I>(point1, point2, point3) == Orientation::Right
}

#[inline(always)]
pub(crate) fn pps<I: InputType>(
    point1: Point<I>,
    point2: Point<I>,
    site3: &vse::SiteEvent<I>,
    segment_index: SiteIndex,
) -> bool {
    #[allow(clippy::suspicious_operation_groupings)]
    if segment_index != SiteIndex::Two {
        let orient1 = orientation_predicate::eval_p::<I>(point1, point2, site3.point0());
        let orient2 = orientation_predicate::eval_p::<I>(point1, point2, site3.point1());
        if segment_index == SiteIndex::One && point1.x >= point2.x {
            if orient1 != Orientation::Right {
                return false;
            }
        } else if segment_index == SiteIndex::Three && point2.x >= point1.x {
            if orient2 != Orientation::Right {
                return false;
            }
        } else if orient1 != Orientation::Right && orient2 != Orientation::Right {
            return false;
        }
    } else {
        return (site3.point0() != point1) || (site3.point1() != point2);
    }
    true
}

#[inline(always)]
pub(crate) fn pss<I: InputType>(
    point1: Point<I>,
    site2: &vse::SiteEvent<I>,
    site3: &vse::SiteEvent<I>,
    point_index: SiteIndex,
) -> bool {
    if site2.sorted_index() == site3.sorted_index() {
        return false;
    }
    if point_index == SiteIndex::Two {
        if !site2.is_inverse() && site3.is_inverse() {
            return false;
        }
        if site2.is_inverse() == site3.is_inverse()
            && orientation_predicate::eval_p::<I>(site2.point0(), point1, site3.point1())
                != Orientation::Right
        {
            return false;
        }
    }
    true
}

pub(crate) fn sss<I: InputType>(
    site1: &vse::SiteEvent<I>,
    site2: &vse::SiteEvent<I>,
    site3: &vse::SiteEvent<I>,
) -> bool {
    (site1.sorted_index() != site2.sorted_index()) && (site2.sorted_index() != site3.sorted_index())
}
