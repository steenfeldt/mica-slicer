// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 tests/libslic3r/test_arachne.cpp :: Arachne regression cases
// Changes: Rust assertions; add positive-width and exact-closure checks to upstream crash-only cases.
use super::*;
use crate::geometry::clip::union_ex_positive;
// Arachne - Closed ExtrusionLine
#[test]
fn upstream_case_00() {
    let polygons = [vec![
        Point::new(-40000000, 10000000),
        Point::new(-62480000, 10000000),
        Point::new(-62480000, -7410000),
        Point::new(-58430000, -7330000),
        Point::new(-58400000, -5420000),
        Point::new(-58720000, -4710000),
        Point::new(-58940000, -3870000),
        Point::new(-59020000, -3000000),
    ]];
    let settings = ArachneSettings {
        ..ArachneSettings::default()
    };
    for angle in [0.0_f64] {
        let polys: Vec<Polygon> = polygons
            .iter()
            .map(|p| {
                p.iter()
                    .map(|p| {
                        Point::new(
                            (p.x as f64 * angle.cos() - p.y as f64 * angle.sin()).round() as i64,
                            (p.x as f64 * angle.sin() + p.y as f64 * angle.cos()).round() as i64,
                        )
                    })
                    .collect()
            })
            .collect();
        let out = WallToolPaths::generate(
            &union_ex_positive(&polys),
            407079.,
            407079.,
            5,
            0.2,
            0.4,
            &settings,
        );
        assert!(!out.paths.is_empty());
        for line in &out.paths {
            assert!(
                line.junctions
                    .iter()
                    .all(|j| j.width > 0. && j.width.is_finite())
            );
            if line.is_closed {
                assert_eq!(
                    line.junctions.first().unwrap().p,
                    line.junctions.last().unwrap().p
                );
            }
        }
    }
}
// Arachne - Missing perimeter - #8472
#[test]
fn upstream_case_01() {
    let polygons = [vec![
        Point::new(-9000000, 8054793),
        Point::new(7000000, 8054793),
        Point::new(7000000, 10211874),
        Point::new(-8700000, 10211874),
        Point::new(-9000000, 9824444),
    ]];
    let settings = ArachneSettings {
        wall_distribution_count: 3,
        ..ArachneSettings::default()
    };
    for angle in [0.0_f64] {
        let polys: Vec<Polygon> = polygons
            .iter()
            .map(|p| {
                p.iter()
                    .map(|p| {
                        Point::new(
                            (p.x as f64 * angle.cos() - p.y as f64 * angle.sin()).round() as i64,
                            (p.x as f64 * angle.sin() + p.y as f64 * angle.cos()).round() as i64,
                        )
                    })
                    .collect()
            })
            .collect();
        let out = WallToolPaths::generate(
            &union_ex_positive(&polys),
            437079.,
            437079.,
            3,
            0.2,
            0.4,
            &settings,
        );
        assert_eq!(
            out.paths
                .iter()
                .map(|l| l.inset)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3
        );
        assert!(!out.paths.is_empty());
        for line in &out.paths {
            assert!(
                line.junctions
                    .iter()
                    .all(|j| j.width > 0. && j.width.is_finite())
            );
            if line.is_closed {
                assert_eq!(
                    line.junctions.first().unwrap().p,
                    line.junctions.last().unwrap().p
                );
            }
        }
    }
}
// Arachne - #8593 - Missing a part of the extrusion
#[test]
fn upstream_case_02() {
    let polygons = [vec![
        Point::new(1800000, 28500000),
        Point::new(1100000, 30000000),
        Point::new(1000000, 30900000),
        Point::new(600000, 32300000),
        Point::new(-600000, 32300000),
        Point::new(-1000000, 30900000),
        Point::new(-1100000, 30000000),
        Point::new(-1800000, 29000000),
    ]];
    let settings = ArachneSettings {
        min_bead_width: FloatOrPercent::Mm(0.315),
        wall_transition_length: FloatOrPercent::Mm(1.),
        wall_transition_angle: 40.,
        ..ArachneSettings::default()
    };
    for angle in [0., -std::f64::consts::PI / 2., -std::f64::consts::PI / 15.] {
        let polys: Vec<Polygon> = polygons
            .iter()
            .map(|p| {
                p.iter()
                    .map(|p| {
                        Point::new(
                            (p.x as f64 * angle.cos() - p.y as f64 * angle.sin()).round() as i64,
                            (p.x as f64 * angle.sin() + p.y as f64 * angle.cos()).round() as i64,
                        )
                    })
                    .collect()
            })
            .collect();
        let out = WallToolPaths::generate(
            &union_ex_positive(&polys),
            377079.,
            377079.,
            3,
            0.2,
            0.4,
            &settings,
        );
        assert!(!out.paths.is_empty());
        for line in &out.paths {
            assert!(
                line.junctions
                    .iter()
                    .all(|j| j.width > 0. && j.width.is_finite())
            );
            if line.is_closed {
                assert_eq!(
                    line.junctions.first().unwrap().p,
                    line.junctions.last().unwrap().p
                );
            }
        }
    }
}
// Arachne - #8573 - A gap in the perimeter - 1
#[test]
fn upstream_case_03() {
    let polygons = [vec![
        Point::new(13960000, 500000),
        Point::new(13920000, 1210000),
        Point::new(13490000, 2270000),
        Point::new(12960000, 3400000),
        Point::new(12470000, 4320000),
        Point::new(12160000, 4630000),
        Point::new(12460000, 3780000),
        Point::new(12700000, 2850000),
        Point::new(12880000, 1910000),
        Point::new(12950000, 1270000),
        Point::new(13000000, 500000),
    ]];
    let settings = ArachneSettings {
        ..ArachneSettings::default()
    };
    for angle in [0.0_f64] {
        let polys: Vec<Polygon> = polygons
            .iter()
            .map(|p| {
                p.iter()
                    .map(|p| {
                        Point::new(
                            (p.x as f64 * angle.cos() - p.y as f64 * angle.sin()).round() as i64,
                            (p.x as f64 * angle.sin() + p.y as f64 * angle.cos()).round() as i64,
                        )
                    })
                    .collect()
            })
            .collect();
        let out = WallToolPaths::generate(
            &union_ex_positive(&polys),
            407079.,
            407079.,
            2,
            0.2,
            0.4,
            &settings,
        );
        assert!(!out.paths.is_empty());
        for line in &out.paths {
            assert!(
                line.junctions
                    .iter()
                    .all(|j| j.width > 0. && j.width.is_finite())
            );
            if line.is_closed {
                assert_eq!(
                    line.junctions.first().unwrap().p,
                    line.junctions.last().unwrap().p
                );
            }
        }
    }
}
// Arachne - #8444 - A gap in the perimeter - 2
#[test]
fn upstream_case_04() {
    let polygons = [vec![
        Point::new(14413938, 3825902),
        Point::new(16817613, 711749),
        Point::new(19653030, 67154),
        Point::new(20075592, 925370),
        Point::new(20245428, 1339788),
        Point::new(20493219, 2121894),
        Point::new(20570295, 2486625),
        Point::new(20616559, 2835232),
        Point::new(20631964, 3166882),
        Point::new(20591800, 3858877),
        Point::new(19928267, 2153012),
        Point::new(19723020, 1829802),
        Point::new(19482017, 1612364),
        Point::new(19344810, 1542433),
        Point::new(19200249, 1500902),
        Point::new(19047680, 1487200),
        Point::new(18631073, 1520777),
        Point::new(18377524, 1567627),
        Point::new(18132517, 1641174),
        Point::new(17896307, 1741360),
        Point::new(17669042, 1868075),
        Point::new(17449999, 2021790),
    ]];
    let settings = ArachneSettings {
        ..ArachneSettings::default()
    };
    for angle in [0.0_f64] {
        let polys: Vec<Polygon> = polygons
            .iter()
            .map(|p| {
                p.iter()
                    .map(|p| {
                        Point::new(
                            (p.x as f64 * angle.cos() - p.y as f64 * angle.sin()).round() as i64,
                            (p.x as f64 * angle.sin() + p.y as f64 * angle.cos()).round() as i64,
                        )
                    })
                    .collect()
            })
            .collect();
        let out = WallToolPaths::generate(
            &union_ex_positive(&polys),
            594159.,
            594159.,
            2,
            0.2,
            0.4,
            &settings,
        );
        assert!(!out.paths.is_empty());
        for line in &out.paths {
            assert!(
                line.junctions
                    .iter()
                    .all(|j| j.width > 0. && j.width.is_finite())
            );
            if line.is_closed {
                assert_eq!(
                    line.junctions.first().unwrap().p,
                    line.junctions.last().unwrap().p
                );
            }
        }
    }
}
// Arachne - #8528 - A hole when number of perimeters is changing
#[test]
fn upstream_case_05() {
    let polygons = [vec![
        Point::new(-30000000, 27650000),
        Point::new(-30000000, 33500000),
        Point::new(-40000000, 33500000),
        Point::new(-40500000, 33500000),
        Point::new(-41100000, 33400000),
        Point::new(-41600000, 33200000),
        Point::new(-42100000, 32900000),
        Point::new(-42600000, 32600000),
        Point::new(-43000000, 32200000),
        Point::new(-43300000, 31700000),
        Point::new(-43600000, 31200000),
        Point::new(-43800000, 30700000),
        Point::new(-43900000, 30100000),
        Point::new(-43900000, 29600000),
        Point::new(-43957080, 25000000),
        Point::new(-39042920, 25000000),
        Point::new(-39042920, 27650000),
    ]];
    let settings = ArachneSettings {
        min_bead_width: FloatOrPercent::Mm(0.66),
        ..ArachneSettings::default()
    };
    for angle in [0.0_f64] {
        let polys: Vec<Polygon> = polygons
            .iter()
            .map(|p| {
                p.iter()
                    .map(|p| {
                        Point::new(
                            (p.x as f64 * angle.cos() - p.y as f64 * angle.sin()).round() as i64,
                            (p.x as f64 * angle.sin() + p.y as f64 * angle.cos()).round() as i64,
                        )
                    })
                    .collect()
            })
            .collect();
        let out = WallToolPaths::generate(
            &union_ex_positive(&polys),
            814159.,
            814159.,
            5,
            0.4,
            0.4,
            &settings,
        );
        assert!(!out.paths.is_empty());
        for line in &out.paths {
            assert!(
                line.junctions
                    .iter()
                    .all(|j| j.width > 0. && j.width.is_finite())
            );
            if line.is_closed {
                assert_eq!(
                    line.junctions.first().unwrap().p,
                    line.junctions.last().unwrap().p
                );
            }
        }
    }
}
// Arachne - #8555 - Inconsistent single perimeter
#[test]
fn upstream_case_06() {
    let polygons = [
        vec![
            Point::new(5527411, -38490007),
            Point::new(11118814, -36631169),
            Point::new(13529600, -36167120),
            Point::new(11300145, -36114514),
            Point::new(10484024, -36113916),
            Point::new(5037323, -37985945),
            Point::new(4097054, -39978866),
        ],
        vec![
            Point::new(5566841, -38517205),
            Point::new(11185208, -36649404),
            Point::new(13462719, -36211009),
            Point::new(11357290, -36161329),
            Point::new(10583855, -36160763),
            Point::new(5105952, -38043516),
            Point::new(4222019, -39917031),
        ],
        vec![
            Point::new(5606269, -38544404),
            Point::new(11251599, -36667638),
            Point::new(13391666, -36255700),
            Point::new(10683552, -36207653),
            Point::new(5174580, -38101085),
            Point::new(4346981, -39855197),
        ],
        vec![
            Point::new(5645699, -38571603),
            Point::new(11317993, -36685873),
            Point::new(13324786, -36299588),
            Point::new(10783383, -36254499),
            Point::new(5243209, -38158655),
            Point::new(4471947, -39793362),
        ],
        vec![
            Point::new(5685128, -38598801),
            Point::new(11384385, -36704108),
            Point::new(13257907, -36343476),
            Point::new(10883211, -36301345),
            Point::new(5311836, -38216224),
            Point::new(4596909, -39731528),
        ],
        vec![
            Point::new(5724558, -38626000),
            Point::new(11450778, -36722343),
            Point::new(13191026, -36387365),
            Point::new(10983042, -36348191),
            Point::new(5380466, -38273795),
            Point::new(4721874, -39669693),
        ],
    ];
    let settings = ArachneSettings {
        ..ArachneSettings::default()
    };
    for input in polygons.chunks(1) {
        for angle in [0.0_f64] {
            let polys: Vec<Polygon> = input
                .iter()
                .map(|p| {
                    p.iter()
                        .map(|p| {
                            Point::new(
                                (p.x as f64 * angle.cos() - p.y as f64 * angle.sin()).round()
                                    as i64,
                                (p.x as f64 * angle.sin() + p.y as f64 * angle.cos()).round()
                                    as i64,
                            )
                        })
                        .collect()
                })
                .collect();
            let out = WallToolPaths::generate(
                &union_ex_positive(&polys),
                417809.,
                417809.,
                2,
                0.15,
                0.4,
                &settings,
            );
            assert!(!out.paths.is_empty());
            for line in &out.paths {
                assert!(
                    line.junctions
                        .iter()
                        .all(|j| j.width > 0. && j.width.is_finite())
                );
                if line.is_closed {
                    assert_eq!(
                        line.junctions.first().unwrap().p,
                        line.junctions.last().unwrap().p
                    );
                }
            }
        }
    }
}
// Arachne - #8633 - Shorter open perimeter
#[test]
fn upstream_case_07() {
    let polygons = [
        vec![
            Point::new(6507498, 4189461),
            Point::new(6460382, 3601960),
            Point::new(6390896, 3181097),
            Point::new(6294072, 2765838),
            Point::new(6170293, 2357794),
            Point::new(7090581, 2045388),
            Point::new(7232821, 2514293),
            Point::new(7344089, 2991501),
            Point::new(7423910, 3474969),
            Point::new(7471937, 3962592),
            Point::new(7487443, 4436235),
            Point::new(6515575, 4436235),
        ],
        vec![
            Point::new(6507498, 4189461),
            Point::new(6460382, 3601960),
            Point::new(6390896, 3181097),
            Point::new(6294072, 2765838),
            Point::new(6170293, 2357794),
            Point::new(6917958, 1586830),
            Point::new(7090552, 2045398),
            Point::new(7232821, 2514293),
            Point::new(7344089, 2991501),
            Point::new(7423910, 3474969),
            Point::new(7471937, 3962592),
            Point::new(7487443, 4436235),
            Point::new(6515575, 4436235),
        ],
    ];
    let settings = ArachneSettings {
        min_bead_width: FloatOrPercent::Mm(0.51),
        min_feature_size: FloatOrPercent::Mm(0.15),
        wall_transition_length: FloatOrPercent::Mm(0.6),
        ..ArachneSettings::default()
    };
    for input in polygons.chunks(1) {
        for angle in [0.0_f64] {
            let polys: Vec<Polygon> = input
                .iter()
                .map(|p| {
                    p.iter()
                        .map(|p| {
                            Point::new(
                                (p.x as f64 * angle.cos() - p.y as f64 * angle.sin()).round()
                                    as i64,
                                (p.x as f64 * angle.sin() + p.y as f64 * angle.cos()).round()
                                    as i64,
                            )
                        })
                        .collect()
                })
                .collect();
            let out = WallToolPaths::generate(
                &union_ex_positive(&polys),
                617809.,
                617809.,
                1,
                0.15,
                0.4,
                &settings,
            );
            assert!(!out.paths.is_empty());
            for line in &out.paths {
                assert!(
                    line.junctions
                        .iter()
                        .all(|j| j.width > 0. && j.width.is_finite())
                );
                if line.is_closed {
                    assert_eq!(
                        line.junctions.first().unwrap().p,
                        line.junctions.last().unwrap().p
                    );
                }
            }
        }
    }
}
// Arachne - #8597 - removeSmallAreas
#[test]
fn upstream_case_08() {
    let polygons = [
        vec![
            Point::new(-38768167, -3636556),
            Point::new(-38763631, -3617883),
            Point::new(-38763925, -3617820),
            Point::new(-38990169, -3919539),
            Point::new(-38928506, -3919539),
        ],
        vec![
            Point::new(-39521732, -4480560),
            Point::new(-39383333, -4398498),
            Point::new(-39119825, -3925307),
            Point::new(-39165608, -3926212),
            Point::new(-39302205, -3959445),
            Point::new(-39578719, -4537002),
        ],
    ];
    let settings = ArachneSettings {
        ..ArachneSettings::default()
    };
    for angle in [0.0_f64] {
        let polys: Vec<Polygon> = polygons
            .iter()
            .map(|p| {
                p.iter()
                    .map(|p| {
                        Point::new(
                            (p.x as f64 * angle.cos() - p.y as f64 * angle.sin()).round() as i64,
                            (p.x as f64 * angle.sin() + p.y as f64 * angle.cos()).round() as i64,
                        )
                    })
                    .collect()
            })
            .collect();
        let out = WallToolPaths::generate(
            &union_ex_positive(&polys),
            407079.,
            407079.,
            2,
            0.2,
            0.4,
            &settings,
        );
        assert_eq!(
            out.paths
                .iter()
                .map(|l| l.inset)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            1
        );
        assert!(!out.paths.is_empty());
        for line in &out.paths {
            assert!(
                line.junctions
                    .iter()
                    .all(|j| j.width > 0. && j.width.is_finite())
            );
            if line.is_closed {
                assert_eq!(
                    line.junctions.first().unwrap().p,
                    line.junctions.last().unwrap().p
                );
            }
        }
    }
}
// Arachne - Missing infill
#[test]
fn upstream_case_09() {
    let polygons = [
        vec![
            Point::new(5525881, 3649657),
            Point::new(452351, -2035297),
            Point::new(-1014702, -2144286),
            Point::new(-5142096, -9101108),
            Point::new(5525882, -9101108),
        ],
        vec![
            Point::new(1415524, -2217520),
            Point::new(1854189, -2113857),
            Point::new(1566974, -2408538),
        ],
        vec![
            Point::new(-42854, -3771357),
            Point::new(310500, -3783332),
            Point::new(77735, -4059215),
        ],
    ];
    let settings = ArachneSettings {
        ..ArachneSettings::default()
    };
    for angle in [0.0_f64] {
        let polys: Vec<Polygon> = polygons
            .iter()
            .map(|p| {
                p.iter()
                    .map(|p| {
                        Point::new(
                            (p.x as f64 * angle.cos() - p.y as f64 * angle.sin()).round() as i64,
                            (p.x as f64 * angle.sin() + p.y as f64 * angle.cos()).round() as i64,
                        )
                    })
                    .collect()
            })
            .collect();
        let out = WallToolPaths::generate(
            &union_ex_positive(&polys),
            357079.,
            357079.,
            2,
            0.2,
            0.4,
            &settings,
        );
        assert!(!out.paths.is_empty());
        for line in &out.paths {
            assert!(
                line.junctions
                    .iter()
                    .all(|j| j.width > 0. && j.width.is_finite())
            );
            if line.is_closed {
                assert_eq!(
                    line.junctions.first().unwrap().p,
                    line.junctions.last().unwrap().p
                );
            }
        }
    }
}
// Arachne - #8849 - Missing part of model
#[test]
fn upstream_case_10() {
    let polygons = [vec![
        Point::new(-29700000, -10600000),
        Point::new(-28200000, -10600000),
        Point::new(20000000, -10600000),
        Point::new(20000000, -9900000),
        Point::new(-28200000, -9900000),
        Point::new(-28200000, 0),
        Point::new(-29700000, 0),
    ]];
    let settings = ArachneSettings {
        ..ArachneSettings::default()
    };
    for angle in [0.0_f64] {
        let polys: Vec<Polygon> = polygons
            .iter()
            .map(|p| {
                p.iter()
                    .map(|p| {
                        Point::new(
                            (p.x as f64 * angle.cos() - p.y as f64 * angle.sin()).round() as i64,
                            (p.x as f64 * angle.sin() + p.y as f64 * angle.cos()).round() as i64,
                        )
                    })
                    .collect()
            })
            .collect();
        let out = WallToolPaths::generate(
            &union_ex_positive(&polys),
            449999.,
            757079.,
            2,
            0.32,
            0.4,
            &settings,
        );
        assert!(!out.paths.is_empty());
        for line in &out.paths {
            assert!(
                line.junctions
                    .iter()
                    .all(|j| j.width > 0. && j.width.is_finite())
            );
            if line.is_closed {
                assert_eq!(
                    line.junctions.first().unwrap().p,
                    line.junctions.last().unwrap().p
                );
            }
        }
    }
}
// Arachne - #8446 - Degenerated Voronoi diagram - Linear edges
#[test]
fn upstream_case_11() {
    let polygons = [vec![
        Point::new(42240656, 9020315),
        Point::new(4474248, 42960681),
        Point::new(-4474248, 42960681),
        Point::new(-4474248, 23193537),
        Point::new(-6677407, 22661038),
        Point::new(-8830542, 21906307),
        Point::new(-9702935, 21539826),
        Point::new(-13110431, 19607811),
        Point::new(-18105334, 15167780),
        Point::new(-20675743, 11422461),
        Point::new(-39475413, 17530840),
        Point::new(-42240653, 9020315),
    ]];
    let settings = ArachneSettings {
        ..ArachneSettings::default()
    };
    for angle in [0.0_f64] {
        let polys: Vec<Polygon> = polygons
            .iter()
            .map(|p| {
                p.iter()
                    .map(|p| {
                        Point::new(
                            (p.x as f64 * angle.cos() - p.y as f64 * angle.sin()).round() as i64,
                            (p.x as f64 * angle.sin() + p.y as f64 * angle.cos()).round() as i64,
                        )
                    })
                    .collect()
            })
            .collect();
        let out = WallToolPaths::generate(
            &union_ex_positive(&polys),
            407079.,
            407079.,
            1,
            0.2,
            0.4,
            &settings,
        );
        assert!(out.paths.iter().map(ExtrusionLine::length).sum::<f64>() <= 211.5 * SCALE);
        assert!(!out.paths.is_empty());
        for line in &out.paths {
            assert!(
                line.junctions
                    .iter()
                    .all(|j| j.width > 0. && j.width.is_finite())
            );
            if line.is_closed {
                assert_eq!(
                    line.junctions.first().unwrap().p,
                    line.junctions.last().unwrap().p
                );
            }
        }
    }
}
// Arachne - #8846 - Degenerated Voronoi diagram - One Parabola
#[test]
fn upstream_case_12() {
    let polygons = [
        vec![
            Point::new(101978540, -41304489),
            Point::new(101978540, 41304489),
            Point::new(94709788, 42514051),
            Point::new(94709788, 48052315),
            Point::new(93352716, 48052315),
            Point::new(93352716, 42514052),
            Point::new(75903540, 42514051),
            Point::new(75903540, 48052315),
            Point::new(74546460, 48052315),
            Point::new(74546460, 42514052),
            Point::new(69634788, 42514051),
            Point::new(69634788, 48052315),
            Point::new(68277708, 48052315),
            Point::new(68277708, 42514051),
            Point::new(63366040, 42514051),
            Point::new(63366040, 48052315),
            Point::new(62008960, 48052315),
            Point::new(62008960, 42514051),
            Point::new(57097292, 42514051),
            Point::new(57097292, 48052315),
            Point::new(55740212, 48052315),
            Point::new(55740212, 42514052),
            Point::new(50828540, 42514052),
            Point::new(50828540, 48052315),
            Point::new(49471460, 48052315),
            Point::new(49471460, 42514051),
            Point::new(25753540, 42514051),
            Point::new(25753540, 48052315),
            Point::new(24396460, 48052315),
            Point::new(24396460, 42514051),
            Point::new(19484790, 42514052),
            Point::new(19484790, 48052315),
            Point::new(18127710, 48052315),
            Point::new(18127710, 42514051),
            Point::new(-5590210, 42514051),
            Point::new(-5590210, 48052315),
            Point::new(-6947290, 48052315),
            Point::new(-6947290, 42514051),
            Point::new(-11858960, 42514051),
            Point::new(-11858960, 48052315),
            Point::new(-13216040, 48052315),
            Point::new(-13216040, 42514051),
            Point::new(-18127710, 42514051),
            Point::new(-18127710, 48052315),
            Point::new(-19484790, 48052315),
            Point::new(-19484790, 42514052),
            Point::new(-49471460, 42514051),
            Point::new(-49471460, 48052315),
            Point::new(-50828540, 48052315),
            Point::new(-50828540, 42514052),
            Point::new(-55740212, 42514052),
            Point::new(-55740212, 48052315),
            Point::new(-57097292, 48052315),
            Point::new(-57097292, 42514051),
            Point::new(-68277708, 42514051),
            Point::new(-68277708, 48052315),
            Point::new(-69634788, 48052315),
            Point::new(-69634788, 42514051),
            Point::new(-74546460, 42514052),
            Point::new(-74546460, 48052315),
            Point::new(-75903540, 48052315),
            Point::new(-75903540, 42514051),
            Point::new(-80815204, 42514051),
            Point::new(-80815204, 48052315),
            Point::new(-82172292, 48052315),
            Point::new(-82172292, 42514051),
            Point::new(-87083956, 42514051),
            Point::new(-87083956, 48052315),
            Point::new(-88441044, 48052315),
            Point::new(-88441044, 42514051),
            Point::new(-99621460, 42514051),
            Point::new(-99621460, 48052315),
            Point::new(-100978540, 48052315),
            Point::new(-100978540, 42528248),
            Point::new(-101978540, 41304489),
            Point::new(-101978540, -41304489),
            Point::new(-100978540, -48052315),
            Point::new(-99621460, -48052315),
        ],
        vec![
            Point::new(-100671460, -40092775),
            Point::new(-100671460, 40092775),
            Point::new(100671460, 40092775),
            Point::new(100671460, -40092775),
        ],
    ];
    let settings = ArachneSettings {
        ..ArachneSettings::default()
    };
    for angle in [0.0_f64] {
        let polys: Vec<Polygon> = polygons
            .iter()
            .map(|p| {
                p.iter()
                    .map(|p| {
                        Point::new(
                            (p.x as f64 * angle.cos() - p.y as f64 * angle.sin()).round() as i64,
                            (p.x as f64 * angle.sin() + p.y as f64 * angle.cos()).round() as i64,
                        )
                    })
                    .collect()
            })
            .collect();
        let out = WallToolPaths::generate(
            &union_ex_positive(&polys),
            607079.,
            607079.,
            1,
            0.2,
            0.4,
            &settings,
        );
        assert!(
            out.paths.iter().map(ExtrusionLine::length).sum::<f64>() <= 1335.0 * SCALE,
            "length {}",
            out.paths.iter().map(ExtrusionLine::length).sum::<f64>() / SCALE
        );
        assert!(!out.paths.is_empty());
        for line in &out.paths {
            assert!(
                line.junctions
                    .iter()
                    .all(|j| j.width > 0. && j.width.is_finite())
            );
            if line.is_closed {
                assert_eq!(
                    line.junctions.first().unwrap().p,
                    line.junctions.last().unwrap().p
                );
            }
        }
    }
}
// Arachne - #9357 - Degenerated Voronoi diagram - Two parabolas
#[test]
fn upstream_case_13() {
    let polygons = [vec![
        Point::new(78998946, -11733905),
        Point::new(40069507, -7401251),
        Point::new(39983905, -6751055),
        Point::new(39983905, 8251054),
        Point::new(79750000, 10522762),
        Point::new(79983905, 10756667),
        Point::new(79983905, 12248946),
        Point::new(79950248, 12504617),
        Point::new(79709032, 12928156),
        Point::new(79491729, 13102031),
        Point::new(78998946, 13233905),
        Point::new(38501054, 13233905),
        Point::new(37258117, 12901005),
        Point::new(36349000, 11991885),
        Point::new(36100868, 11392844),
        Point::new(36016095, 10748947),
        Point::new(36016095, -6751054),
        Point::new(35930493, -7401249),
        Point::new(4685798, -11733905),
    ]];
    let settings = ArachneSettings {
        ..ArachneSettings::default()
    };
    for angle in [0.0_f64] {
        let polys: Vec<Polygon> = polygons
            .iter()
            .map(|p| {
                p.iter()
                    .map(|p| {
                        Point::new(
                            (p.x as f64 * angle.cos() - p.y as f64 * angle.sin()).round() as i64,
                            (p.x as f64 * angle.sin() + p.y as f64 * angle.cos()).round() as i64,
                        )
                    })
                    .collect()
            })
            .collect();
        let out = WallToolPaths::generate(
            &union_ex_positive(&polys),
            407079.,
            407079.,
            1,
            0.2,
            0.4,
            &settings,
        );
        assert!(out.paths.iter().map(ExtrusionLine::length).sum::<f64>() <= 256.0 * SCALE);
        assert!(!out.paths.is_empty());
        for line in &out.paths {
            assert!(
                line.junctions
                    .iter()
                    .all(|j| j.width > 0. && j.width.is_finite())
            );
            if line.is_closed {
                assert_eq!(
                    line.junctions.first().unwrap().p,
                    line.junctions.last().unwrap().p
                );
            }
        }
    }
}
// Arachne - #8846 - Degenerated Voronoi diagram - Voronoi edges intersecting input segment
#[test]
fn upstream_case_14() {
    let polygons = [vec![
        Point::new(60000000, 58000000),
        Point::new(-20000000, 53229451),
        Point::new(49312250, 53229452),
        Point::new(49443687, 53666225),
        Point::new(55358348, 50908580),
        Point::new(53666223, 49443687),
        Point::new(53229452, 49312250),
        Point::new(53229452, -49312250),
        Point::new(53666014, -49443623),
        Point::new(-10000000, -58000000),
        Point::new(60000000, -58000000),
    ]];
    let settings = ArachneSettings {
        ..ArachneSettings::default()
    };
    for angle in [0.0_f64] {
        let polys: Vec<Polygon> = polygons
            .iter()
            .map(|p| {
                p.iter()
                    .map(|p| {
                        Point::new(
                            (p.x as f64 * angle.cos() - p.y as f64 * angle.sin()).round() as i64,
                            (p.x as f64 * angle.sin() + p.y as f64 * angle.cos()).round() as i64,
                        )
                    })
                    .collect()
            })
            .collect();
        let out = WallToolPaths::generate(
            &union_ex_positive(&polys),
            407079.,
            407079.,
            1,
            0.32,
            0.4,
            &settings,
        );
        assert!(out.paths.iter().map(ExtrusionLine::length).sum::<f64>() <= 500.0 * SCALE);
        assert!(!out.paths.is_empty());
        for line in &out.paths {
            assert!(
                line.junctions
                    .iter()
                    .all(|j| j.width > 0. && j.width.is_finite())
            );
            if line.is_closed {
                assert_eq!(
                    line.junctions.first().unwrap().p,
                    line.junctions.last().unwrap().p
                );
            }
        }
    }
}
// Arachne - #10034 - Degenerated Voronoi diagram - That wasn't fixed by rotation by PI / 6
#[test]
fn upstream_case_15() {
    let polygons = [
        vec![
            Point::new(43612632, -25179766),
            Point::new(58456010, 529710),
            Point::new(51074898, 17305660),
            Point::new(49390982, 21042355),
            Point::new(48102357, 23840161),
            Point::new(46769686, 26629546),
            Point::new(45835761, 28472742),
            Point::new(45205450, 29623133),
            Point::new(45107431, 29878059),
            Point::new(45069846, 30174950),
            Point::new(45069846, 50759533),
            Point::new(-45069846, 50759533),
            Point::new(-45069852, 29630557),
            Point::new(-45105780, 29339980),
            Point::new(-45179725, 29130704),
            Point::new(-46443313, 26398986),
            Point::new(-52272109, 13471493),
            Point::new(-58205450, 95724),
            Point::new(-29075091, -50359531),
            Point::new(29075086, -50359531),
        ],
        vec![
            Point::new(-37733905, 45070445),
            Point::new(-37813254, 45116257),
            Point::new(-39353851, 47784650),
            Point::new(-39353851, 47876274),
            Point::new(-38632470, 49125743),
            Point::new(-38553121, 49171555),
            Point::new(-33833475, 49171555),
            Point::new(-33754126, 49125743),
            Point::new(-33032747, 47876277),
            Point::new(-33032747, 47784653),
            Point::new(-34007855, 46095721),
            Point::new(-34573350, 45116257),
            Point::new(-34652699, 45070445),
        ],
        vec![
            Point::new(-44016799, 40706401),
            Point::new(-44116953, 40806555),
            Point::new(-44116953, 46126289),
            Point::new(-44016799, 46226443),
            Point::new(-42211438, 46226443),
            Point::new(-42132089, 46180631),
            Point::new(-40591492, 43512233),
            Point::new(-40591492, 43420609),
            Point::new(-41800123, 41327194),
            Point::new(-42132089, 40752213),
            Point::new(-42211438, 40706401),
        ],
        vec![
            Point::new(6218189, 10966609),
            Point::new(6138840, 11012421),
            Point::new(4598238, 13680817),
            Point::new(4598238, 13772441),
            Point::new(6138840, 16440843),
            Point::new(6218189, 16486655),
            Point::new(9299389, 16486655),
            Point::new(9378738, 16440843),
            Point::new(10919340, 13772441),
            Point::new(10919340, 13680817),
            Point::new(10149039, 12346618),
            Point::new(9378738, 11012421),
            Point::new(9299389, 10966609),
        ],
        vec![
            Point::new(13576879, 6718065),
            Point::new(13497530, 6763877),
            Point::new(11956926, 9432278),
            Point::new(11956926, 9523902),
            Point::new(13497528, 12192302),
            Point::new(13576877, 12238114),
            Point::new(16658079, 12238112),
            Point::new(16737428, 12192300),
            Point::new(18278031, 9523904),
            Point::new(18278031, 9432280),
            Point::new(17507729, 8098077),
            Point::new(16737428, 6763877),
            Point::new(16658079, 6718065),
        ],
    ];
    let settings = ArachneSettings {
        ..ArachneSettings::default()
    };
    for angle in [0.0_f64] {
        let polys: Vec<Polygon> = polygons
            .iter()
            .map(|p| {
                p.iter()
                    .map(|p| {
                        Point::new(
                            (p.x as f64 * angle.cos() - p.y as f64 * angle.sin()).round() as i64,
                            (p.x as f64 * angle.sin() + p.y as f64 * angle.cos()).round() as i64,
                        )
                    })
                    .collect()
            })
            .collect();
        let out = WallToolPaths::generate(
            &union_ex_positive(&polys),
            407079.,
            407079.,
            1,
            0.2,
            0.4,
            &settings,
        );
        assert!(!out.paths.is_empty());
        for line in &out.paths {
            assert!(
                line.junctions
                    .iter()
                    .all(|j| j.width > 0. && j.width.is_finite())
            );
            if line.is_closed {
                assert_eq!(
                    line.junctions.first().unwrap().p,
                    line.junctions.last().unwrap().p
                );
            }
        }
    }
}
// Arachne - SPE-1837 - No perimeters generated
#[test]
fn upstream_case_16() {
    let polygons = [vec![
        Point::new(10000000, 10000000),
        Point::new(-10000000, 10000000),
        Point::new(-10000000, -10000000),
        Point::new(10000000, -10000000),
    ]];
    let settings = ArachneSettings {
        ..ArachneSettings::default()
    };
    for angle in [0.0_f64] {
        let polys: Vec<Polygon> = polygons
            .iter()
            .map(|p| {
                p.iter()
                    .map(|p| {
                        Point::new(
                            (p.x as f64 * angle.cos() - p.y as f64 * angle.sin()).round() as i64,
                            (p.x as f64 * angle.sin() + p.y as f64 * angle.cos()).round() as i64,
                        )
                    })
                    .collect()
            })
            .collect();
        let out = WallToolPaths::generate(
            &union_ex_positive(&polys),
            300000.,
            700000.,
            1,
            0.2,
            0.4,
            &settings,
        );
        assert!(!out.paths.is_empty());
        assert!(!out.paths.is_empty());
        for line in &out.paths {
            assert!(
                line.junctions
                    .iter()
                    .all(|j| j.width > 0. && j.width.is_finite())
            );
            if line.is_closed {
                assert_eq!(
                    line.junctions.first().unwrap().p,
                    line.junctions.last().unwrap().p
                );
            }
        }
    }
}
// Arachne - SPE-2298 - Missing twin edge
#[test]
fn upstream_case_17() {
    let polygons = [vec![
        Point::new(45275325, -26003582),
        Point::new(46698318, -24091837),
        Point::new(45534079, -7648226),
        Point::new(44427730, 6913138),
        Point::new(42406709, 31931594),
        Point::new(42041617, 31895427),
        Point::new(42556409, 25628802),
        Point::new(43129149, 18571997),
        Point::new(44061956, 6884616),
        Point::new(44482729, 1466404),
        Point::new(45172290, -7674740),
        Point::new(46329004, -23890062),
        Point::new(46303776, -23895512),
        Point::new(45146815, -7676652),
        Point::new(44457276, 1464203),
        Point::new(44036504, 6882422),
        Point::new(43103702, 18569730),
        Point::new(42015592, 31899494),
        Point::new(41650258, 31866937),
        Point::new(44100538, 1436619),
    ]];
    let settings = ArachneSettings {
        ..ArachneSettings::default()
    };
    for angle in [0.0_f64] {
        let polys: Vec<Polygon> = polygons
            .iter()
            .map(|p| {
                p.iter()
                    .map(|p| {
                        Point::new(
                            (p.x as f64 * angle.cos() - p.y as f64 * angle.sin()).round() as i64,
                            (p.x as f64 * angle.sin() + p.y as f64 * angle.cos()).round() as i64,
                        )
                    })
                    .collect()
            })
            .collect();
        let out = WallToolPaths::generate(
            &union_ex_positive(&polys),
            407079.,
            407079.,
            1,
            0.2,
            0.4,
            &settings,
        );
        assert!(!out.paths.is_empty());
        assert!(!out.paths.is_empty());
        for line in &out.paths {
            assert!(
                line.junctions
                    .iter()
                    .all(|j| j.width > 0. && j.width.is_finite())
            );
            if line.is_closed {
                assert_eq!(
                    line.junctions.first().unwrap().p,
                    line.junctions.last().unwrap().p
                );
            }
        }
    }
}
// Arachne - SPE-2298 - Missing twin edge - 2
#[test]
fn upstream_case_18() {
    let polygons = [vec![
        Point::new(-8908308, -51405945),
        Point::new(-12709229, -51250796),
        Point::new(-12746335, -51233657),
        Point::new(-12830242, -51142897),
        Point::new(-12826443, -51134671),
        Point::new(-13181213, -51120650),
        Point::new(-13184646, -51206854),
        Point::new(-19253324, -50972142),
        Point::new(-19253413, -50972139),
        Point::new(-20427346, -50924668),
        Point::new(-20427431, -50924664),
        Point::new(-25802429, -50698485),
        Point::new(-25802568, -50698481),
        Point::new(-28983179, -50556020),
        Point::new(-28984425, -50555950),
        Point::new(-29799753, -50499586),
        Point::new(-29801136, -50499472),
        Point::new(-29856539, -50494137),
        Point::new(-29857834, -50493996),
        Point::new(-30921022, -50364409),
        Point::new(-30922312, -50364235),
        Point::new(-31012584, -50350908),
        Point::new(-31022222, -50358055),
        Point::new(-31060596, -50368155),
        Point::new(-31429495, -50322406),
        Point::new(-31460950, -50531962),
        Point::new(-31194587, -50578945),
        Point::new(-30054463, -50718244),
        Point::new(-28903516, -50799260),
        Point::new(-14217296, -51420133),
        Point::new(-8916965, -51624212),
    ]];
    let settings = ArachneSettings {
        ..ArachneSettings::default()
    };
    for angle in [0.0_f64] {
        let polys: Vec<Polygon> = polygons
            .iter()
            .map(|p| {
                p.iter()
                    .map(|p| {
                        Point::new(
                            (p.x as f64 * angle.cos() - p.y as f64 * angle.sin()).round() as i64,
                            (p.x as f64 * angle.sin() + p.y as f64 * angle.cos()).round() as i64,
                        )
                    })
                    .collect()
            })
            .collect();
        let out = WallToolPaths::generate(
            &union_ex_positive(&polys),
            407079.,
            407079.,
            1,
            0.2,
            0.4,
            &settings,
        );
        assert!(!out.paths.is_empty());
        assert!(!out.paths.is_empty());
        for line in &out.paths {
            assert!(
                line.junctions
                    .iter()
                    .all(|j| j.width > 0. && j.width.is_finite())
            );
            if line.is_closed {
                assert_eq!(
                    line.junctions.first().unwrap().p,
                    line.junctions.last().unwrap().p
                );
            }
        }
    }
}
// Arachne - SPE-2496 - Negative extrusion width
#[test]
fn upstream_case_19() {
    let polygons = [vec![
        Point::new(-4982523, -5994247),
        Point::new(-4700644, -6050605),
        Point::new(5959771, 4609799),
        Point::new(5901029, 4779898),
        Point::new(5871716, 4899000),
        Point::new(5864557, 5026026),
        Point::new(5890832, 5722622),
        Point::new(5870131, 5738234),
        Point::new(5304622, 5553229),
        Point::new(4580330, 5240254),
        Point::new(4109435, 4998946),
        Point::new(3606964, 4699087),
        Point::new(2676357, 4015459),
        Point::new(1076547, 2101298),
        Point::new(-900993, 41373),
        Point::new(-1481954, -514246),
        Point::new(-4119704, -2738265),
        Point::new(-4484070, -3261707),
        Point::new(-4628548, -3650430),
        Point::new(-4712361, -3810835),
        Point::new(-5329484, -4699252),
        Point::new(-5670086, -5625540),
        Point::new(-5727314, -6080805),
        Point::new(-5726304, -6081822),
    ]];
    let settings = ArachneSettings {
        min_bead_width: FloatOrPercent::Mm(0.1),
        ..ArachneSettings::default()
    };
    for angle in [0.0_f64] {
        let polys: Vec<Polygon> = polygons
            .iter()
            .map(|p| {
                p.iter()
                    .map(|p| {
                        Point::new(
                            (p.x as f64 * angle.cos() - p.y as f64 * angle.sin()).round() as i64,
                            (p.x as f64 * angle.sin() + p.y as f64 * angle.cos()).round() as i64,
                        )
                    })
                    .collect()
            })
            .collect();
        let out = WallToolPaths::generate(
            &union_ex_positive(&polys),
            407079.,
            407079.,
            3,
            0.2,
            0.4,
            &settings,
        );
        assert!(!out.paths.is_empty());
        for line in &out.paths {
            assert!(
                line.junctions
                    .iter()
                    .all(|j| j.width > 0. && j.width.is_finite())
            );
            if line.is_closed {
                assert_eq!(
                    line.junctions.first().unwrap().p,
                    line.junctions.last().unwrap().p
                );
            }
        }
    }
}
