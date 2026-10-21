// SPDX-License-Identifier: BSL-1.0
// Provenance: ported
// Source: boostvoronoi@4060736209b06919a08590f37480d88b429599ba src/site_event/tests.rs :: site_event tests
// Changes: Vendored from boostvoronoi 0.12.1 (eadf's Rust port of Boost.Polygon's Voronoi, BSL-1.0);
// Changes: unchanged.

#![allow(unused_imports)]
use super::super::BvError;
use super::super::diagram::Diagram;
use super::super::geometry::{Line, Point};
use super::super::site_event as vse;

#[test]
fn inverse_test_1() {
    type I1 = i32;

    let mut a_site = vse::SiteEvent::<I1>::new(
        vse::Site::Segment(Point { x: 10, y: 10 }, Point { x: 50, y: 50 }),
        1,
    );
    assert!(!a_site.is_inverse());
    let _ = a_site.inverse();
    assert!(a_site.is_inverse());
}

#[test]
fn inverse_test_2() {
    type I1 = i32;

    let mut a_site = vse::SiteEvent::<I1>::new(
        vse::Site::Segment(Point { x: 10, y: 11 }, Point { x: 12, y: 13 }),
        1,
    );
    assert!(!a_site.is_inverse());
    assert_eq!(a_site.x0(), 10);
    assert_eq!(a_site.y0(), 11);
    assert_eq!(a_site.x1(), 12);
    assert_eq!(a_site.y1(), 13);

    let _ = a_site.inverse();
    assert!(a_site.is_inverse());
    assert_eq!(a_site.x0(), 12);
    assert_eq!(a_site.y0(), 13);
    assert_eq!(a_site.x1(), 10);
    assert_eq!(a_site.y1(), 11);

    let _ = a_site.inverse();
    assert!(!a_site.is_inverse());
    assert_eq!(a_site.x0(), 10);
    assert_eq!(a_site.y0(), 11);
    assert_eq!(a_site.x1(), 12);
    assert_eq!(a_site.y1(), 13);
}
