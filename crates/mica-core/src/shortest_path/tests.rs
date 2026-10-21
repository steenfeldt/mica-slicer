// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use super::*;

/// Runs one case in the text format of the reference harness and formats the result the same way.
fn run_case(line: &str) -> String {
    let mut it = line.split_whitespace();
    let mode = it.next().unwrap();
    let mut num = || it.next().unwrap().parse::<i64>().unwrap();
    let has_start = num() != 0;
    let start = Point::new(num(), num());
    let reversed = num() != 0;
    let n = num() as usize;
    let mut entities = Vec::with_capacity(n);
    for _ in 0..n {
        let first = Point::new(num(), num());
        let last = Point::new(num(), num());
        let can_reverse = num() != 0;
        let is_loop = num() != 0;
        entities.push(Entity {
            first,
            last,
            is_loop,
            can_reverse,
        });
    }
    let start = has_start.then_some(start);
    let out: Vec<(usize, bool)> = match mode {
        "E" => chain_entities(&entities, start, reversed),
        "G" => chain_paths(
            &entities
                .iter()
                .map(|e| (e.first, e.last))
                .collect::<Vec<_>>(),
            start,
        ),
        _ => {
            let pls: Vec<Polyline> = entities
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    vec![
                        e.first,
                        Point::new(i as i64, -1),
                        Point::new(i as i64, -2),
                        e.last,
                    ]
                })
                .collect();
            chain_polylines(pls, start)
                .iter()
                .map(|pl| (pl[1].x as usize, pl[1].y == -2))
                .collect()
        }
    };
    out.iter()
        .map(|(i, r)| format!("{i}:{} ", u8::from(*r)))
        .collect()
}

/// Compares against reference output produced by the C++ harness built from PrusaSlicer's own
/// ShortestPath.cpp, KDTreeIndirect.hpp and MutablePriorityQueue.hpp:
/// `MICA_SHORTEST_PATH_CASES=cases.txt MICA_SHORTEST_PATH_EXPECTED=expected.txt cargo test`.
#[test]
#[ignore = "needs the C++ harness output: set MICA_SHORTEST_PATH_CASES and MICA_SHORTEST_PATH_EXPECTED, cargo test -- --ignored"]
fn reference_cases_from_env() {
    let (Ok(cases), Ok(expected)) = (
        std::env::var("MICA_SHORTEST_PATH_CASES"),
        std::env::var("MICA_SHORTEST_PATH_EXPECTED"),
    ) else {
        panic!("set MICA_SHORTEST_PATH_CASES and MICA_SHORTEST_PATH_EXPECTED");
    };
    let cases = std::fs::read_to_string(cases).unwrap();
    let expected = std::fs::read_to_string(expected).unwrap();
    let mut bad = 0;
    for (k, (case, want)) in cases.lines().zip(expected.lines()).enumerate() {
        let got = run_case(case);
        if got.trim_end() != want.trim_end() {
            bad += 1;
            if bad < 5 {
                eprintln!("case {k}: {case}\n  want {want}\n  got  {got}");
            }
        }
    }
    assert_eq!(bad, 0, "{bad} cases differ");
}
const GOLDEN: &[(&str, &str)] = &[
    (
        "E 1 3 1 0 8 0 0 2 0 1 0 1 1 1 1 1 0 3 1 2 0 1 0 0 0 2 0 1 0 0 0 0 0 1 0 2 0 0 0 1 0 2 0 0 0 1 0 0 0 0 0 1 0",
        "2:0 0:1 3:0 5:0 4:1 7:0 6:1 1:0",
    ),
    (
        "E 1 10 10 0 8 20 0 20 0 1 1 0 0 0 0 1 0 0 0 0 0 1 1 20 0 10 10 1 0 0 0 30 10 0 0 20 0 10 10 1 0 20 0 0 0 0 0 0 0 0 0 0 0",
        "3:1 5:0 6:0 4:0 0:0 2:0 7:0 1:1",
    ),
    (
        "E 1 200000 0 0 8 200000 0 200000 0 0 0 300000 100000 300000 100000 1 0 300000 100000 0 0 0 0 200000 0 0 0 0 0 200000 0 200000 0 0 0 0 0 0 0 0 0 300000 100000 100000 100000 0 0 300000 100000 200000 0 1 0",
        "7:1 1:0 6:0 3:0 5:0 4:0 0:0 2:0",
    ),
    (
        "E 1 3000 1000 0 8 2000 0 3000 1000 1 0 0 0 1000 1000 1 0 0 0 1000 1000 0 0 2000 0 1000 1000 0 0 1000 1000 0 0 1 0 1000 1000 2000 0 1 0 1000 1000 0 0 0 0 2000 0 1000 1000 1 0",
        "0:1 5:1 1:1 4:1 7:1 3:0 6:0 2:0",
    ),
    (
        "E 0 0 0 0 8 0 0 0 0 0 0 1 1 1 1 0 1 2 0 2 0 0 0 3 1 3 1 1 0 0 0 0 0 1 0 3 1 0 0 0 0 1 1 1 1 0 0 0 0 2 0 1 0",
        "0:0 7:0 2:0 3:1 5:0 4:1 1:0 6:0",
    ),
    (
        "E 1 6 0 0 8 3 3 9 3 1 0 9 3 6 0 1 0 9 3 6 0 1 0 9 3 3 3 1 0 0 0 0 0 1 0 3 3 9 3 1 0 0 0 3 3 1 0 6 0 9 3 1 0",
        "2:1 5:1 3:1 7:1 1:1 0:1 6:1 4:1",
    ),
    (
        "G 1 3 1 0 8 0 0 3 1 1 0 1 1 1 1 1 0 2 0 1 1 1 0 1 1 2 0 1 0 0 0 0 0 1 0 1 1 1 1 1 0 2 0 0 0 1 0 2 0 2 0 1 0",
        "0:1 4:1 6:1 7:1 3:1 1:0 5:0 2:1",
    ),
    (
        "G 0 0 0 0 8 1 1 0 0 1 0 0 0 0 0 1 0 1 1 2 0 1 0 1 1 1 1 1 0 2 0 3 1 1 0 2 0 1 1 1 0 2 0 2 0 1 0 1 1 1 1 1 0",
        "4:1 6:0 2:0 5:0 7:0 3:0 0:0 1:1",
    ),
    (
        "G 1 300000 100000 0 8 100000 100000 100000 100000 1 0 200000 0 200000 0 1 0 300000 100000 0 0 1 0 300000 100000 300000 100000 1 0 100000 100000 200000 0 1 0 100000 100000 100000 100000 1 0 100000 100000 100000 100000 1 0 300000 100000 200000 0 1 0",
        "3:1 5:1 0:0 6:1 4:0 1:1 7:1 2:0",
    ),
    (
        "G 1 9 3 0 8 0 0 0 0 1 0 0 0 6 0 1 0 0 0 0 0 1 0 3 3 6 0 1 0 3 3 3 3 1 0 3 3 3 3 1 0 6 0 6 0 1 0 6 0 3 3 1 0",
        "7:0 4:0 5:0 3:0 6:0 1:1 2:0 0:0",
    ),
    (
        "P 0 3000 1000 0 8 2000 0 3000 1000 1 0 2000 0 3000 1000 1 0 3000 1000 3000 1000 1 0 2000 0 2000 0 1 0 2000 0 3000 1000 1 0 3000 1000 2000 0 1 0 2000 0 1000 1000 1 0 2000 0 3000 1000 1 0",
        "1:1 3:1 0:0 7:1 5:1 2:0 4:1 6:0",
    ),
    (
        "P 0 30 10 0 8 10 10 10 10 1 0 10 10 10 10 1 0 10 10 10 10 1 0 20 0 30 10 1 0 10 10 0 0 1 0 20 0 10 10 1 0 10 10 10 10 1 0 20 0 20 0 1 0",
        "3:1 7:0 5:0 6:1 1:1 0:1 2:0 4:0",
    ),
    (
        "P 0 6 2 0 8 6 2 6 2 1 0 6 2 2 2 1 0 6 2 6 2 1 0 0 0 6 2 1 0 2 2 0 0 1 0 0 0 0 0 1 0 2 2 2 2 1 0 2 2 4 0 1 0",
        "4:0 5:1 3:0 2:0 0:0 1:0 6:0 7:0",
    ),
    (
        "P 0 0 7 0 8 4 7 0 7 1 0 2 7 -2 7 1 0 -3 7 -5 7 1 0 -3 7 -3 7 1 0 4 7 -3 7 1 0 5 7 -3 7 1 0 5 7 4 7 1 0 4 7 -3 7 1 0",
        "1:0 0:1 6:1 5:0 3:0 4:1 7:0 2:0",
    ),
    (
        "P 0 4 0 0 8 6 2 2 2 1 0 2 2 2 2 1 0 2 2 4 0 1 0 0 0 0 0 1 0 6 2 0 0 1 0 2 2 6 2 1 0 2 2 2 2 1 0 0 0 0 0 1 0",
        "7:0 3:0 4:1 5:1 1:0 2:1 6:1 0:1",
    ),
    (
        "P 0 30 10 0 8 20 0 10 10 1 0 10 10 10 10 1 0 20 0 20 0 1 0 30 10 20 0 1 0 20 0 20 0 1 0 10 10 30 10 1 0 20 0 0 0 1 0 20 0 0 0 1 0",
        "0:0 1:1 5:0 3:0 4:1 7:0 6:1 2:0",
    ),
    (
        "E 1 0 0 0 4 -79361589 116255891 -79361589 116255891 1 0 116255891 -79361589 116255891 -79361589 1 0 89673101 -66252221 89673101 -66252221 1 0 -66252221 89673101 -66252221 89673101 1 0",
        "2:0 1:1 3:0 0:0",
    ),
    (
        "E 1 0 0 0 4 133517017 68427393 133517017 68427393 1 0 68427393 133517017 68427393 133517017 1 0 -126627625 -131366283 -126627625 -131366283 1 0 -131366283 -126627625 -131366283 -126627625 1 0",
        "1:1 0:1 2:1 3:1",
    ),
    (
        "P 0 2 2 0 10 2 2 0 0 1 0 0 0 4 0 1 0 4 0 0 0 1 0 2 2 4 0 1 0 4 0 6 2 1 0 0 0 2 2 1 0 0 0 2 2 1 0 4 0 6 2 1 0 0 0 4 0 1 0 4 0 4 0 1 0",
        "0:0 5:0 3:0 1:1 8:0 9:0 7:0 4:1 2:0 6:0",
    ),
    (
        "P 0 158 26 0 10 -70 178 -70 178 1 0 192 196 192 272 1 0 -28 -150 36 -34 1 0 96 -82 96 -82 1 0 196 188 196 244 1 0 -68 42 136 -20 1 0 146 -16 146 68 1 0 -28 -60 -28 100 1 0 70 -84 70 78 1 0 30 -150 30 10 1 0",
        "3:1 8:0 9:1 2:0 7:0 0:1 5:0 6:0 4:0 1:1",
    ),
    (
        "P 0 -10191020 -5446088 0 10 44738182 -140174579 44738182 -136074579 1 0 -122908964 -82967366 -122908964 -80067366 1 0 122326732 65943105 122326732 75343105 1 0 12792627 131364467 12792627 138564467 1 0 -33249854 1232235 -33249854 4732235 1 0 -117155842 -110780845 -88664058 -148508927 1 0 34675784 136939897 34675784 139539897 1 0 -2144339 61408967 -32088985 -31906487 1 0 -47589478 14327957 -20110038 -52130324 1 0 -125353070 117756819 -125353070 126656819 1 0",
        "1:1 5:0 0:0 8:1 4:1 7:1 2:0 6:0 3:1 9:1",
    ),
    (
        "P 0 -87000 1000 0 17 11000 1000 -86000 1000 1 0 -59000 0 -81000 1000 1 0 -65000 -1000 92000 0 1 0 -20000 0 -71000 1000 1 0 -99000 0 15000 0 1 0 -56000 0 46000 1000 1 0 37000 0 58000 0 1 0 76000 0 -71000 1000 1 0 -72000 -1000 -1000 -1000 1 0 62000 -1000 98000 0 1 0 73000 1000 3000 -1000 1 0 26000 0 -73000 1000 1 0 9000 0 37000 0 1 0 -100000 1000 -11000 0 1 0 96000 1000 -31000 1000 1 0 -9000 -1000 -57000 0 1 0 51000 0 11000 0 1 0",
        "11:0 8:0 10:1 7:0 3:1 14:1 9:1 6:1 12:1 16:1 5:1 15:1 13:1 4:0 0:0 1:1 2:0",
    ),
];

/// Small cases (duplicated and collinear points, segments that may not reverse, loops, chain
/// flipping and equal connection lengths) with the order the C++ reference harness printed for them.
#[test]
fn golden_cases_from_reference_harness() {
    for (case, want) in GOLDEN {
        assert_eq!(run_case(case).trim_end(), *want, "{case}");
    }
}

#[test]
fn entities_that_cannot_reverse_keep_their_direction() {
    let e = |a: (i64, i64), b: (i64, i64), can_reverse| Entity {
        first: Point::new(a.0, a.1),
        last: Point::new(b.0, b.1),
        is_loop: false,
        can_reverse,
    };
    // Going right to left would be shorter, but the middle entity may only run left to right.
    let entities = [
        e((30, 0), (20, 0), true),
        e((0, 0), (10, 0), false),
        e((60, 0), (50, 0), true),
    ];
    let out = chain_entities(&entities, Some(Point::new(100, 0)), false);
    assert!(out.iter().all(|&(i, rev)| i != 1 || !rev));
    assert_eq!(out.len(), 3);
}

#[test]
fn islands_are_chained_from_the_first_contour_point() {
    let square = |x: i64| ExPolygon {
        contour: vec![
            Point::new(x, 0),
            Point::new(x + 10, 0),
            Point::new(x + 10, 10),
            Point::new(x, 10),
        ],
        holes: Vec::new(),
    };
    let order = chain_expolygons(&[square(100), square(0), square(50)]);
    assert!(
        order == vec![1, 2, 0] || order == vec![0, 2, 1],
        "{order:?}"
    );
}
