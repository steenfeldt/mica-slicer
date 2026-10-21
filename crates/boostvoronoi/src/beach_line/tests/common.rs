// SPDX-License-Identifier: BSL-1.0
// Provenance: ported
// Source: boostvoronoi@4060736209b06919a08590f37480d88b429599ba src/beach_line/tests/common.rs :: beach line test parsing
// Changes: Vendored from boostvoronoi 0.12.1 (eadf's Rust port of Boost.Polygon's Voronoi, BSL-1.0);
// Changes: unchanged.

use regex::Regex;
use std::sync::LazyLock;

pub(super) static RE_PREDICATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?P<node1>L.+), (?P<node2>L.+)\)==(?P<result>.+)").unwrap());

pub(super) static RE_NODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"L:(?P<left>.+),R:(?P<right>.+)").unwrap());

pub(super) static RE_NODE_SINGLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"#(?P<si>\d+)\((?P<x>-?\d+),(?P<y>-?\d+)\),ii:(?P<ii>-?\d+),f:(?P<f>\d+)").unwrap()
});

pub(super) static RE_NODE_DOUBLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"#(?P<si>\d+)\((?P<x1>-?\d+),(?P<y1>-?\d+)\)-\((?P<x2>-?\d+),(?P<y2>-?\d+)\),ii:(?P<ii>-?\d+),f:(?P<f>\d+)").unwrap()
});

pub(super) static RE_NODE_INVERS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"#(?P<si>\d+)\((?P<x1>-?\d+),(?P<y1>-?\d+)\)¿\((?P<x2>-?\d+),(?P<y2>-?\d+)\),ii:(?P<ii>-?\d+),f:(?P<f>\d+)").unwrap()
});
