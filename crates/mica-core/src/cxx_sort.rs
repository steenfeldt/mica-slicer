// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/AvoidCrossingPerimeters.cpp :: avoid_perimeters_inner, get_closest_lines_in_radius
// Changes: PrusaSlicer orders equal keys with the C++ standard library's unstable std::sort. The
// Changes: reference macOS binary instantiates libc++'s introsort (LLVM, Apache-2.0 WITH LLVM-exception;
// Changes: SDK header __algorithm/sort.h): sorting networks for 2-5 elements, insertion sort below 24,
// Changes: median-of-3 / Tukey ninther pivots, equal-on-right/left partitions, incomplete insertion
// Changes: sort after a perfect partition and the heap-sort fallback. Re-implemented on slices.

//! Element order of libc++ `std::sort`, for upstream sorts whose keys can tie.
//!
//! Rust's `sort_by` is stable, so ties keep their input order; libc++ does not. Where an upstream
//! decision depends on which of two equal-cost candidates comes first (travel intersections at the
//! same distance, for example), only the same algorithm reproduces the reference output.

/// Sorts `v` with a strict-weak-ordering `less`, placing equal elements exactly as libc++'s
/// `std::sort` does for a non-arithmetic element type.
pub fn sort_by<T: Clone>(v: &mut [T], mut less: impl FnMut(&T, &T) -> bool) {
    if v.len() < 2 {
        return;
    }
    let depth = 2 * (usize::BITS - 1 - v.len().leading_zeros()) as usize;
    introsort(v, 0, v.len(), &mut less, depth, true);
}

fn sort3<T>(v: &mut [T], x: usize, y: usize, z: usize, c: &mut impl FnMut(&T, &T) -> bool) -> bool {
    if !c(&v[y], &v[x]) {
        if !c(&v[z], &v[y]) {
            return false;
        }
        v.swap(y, z);
        if c(&v[y], &v[x]) {
            v.swap(x, y);
        }
        return true;
    }
    if c(&v[z], &v[y]) {
        v.swap(x, z);
        return true;
    }
    v.swap(x, y);
    if c(&v[z], &v[y]) {
        v.swap(y, z);
    }
    true
}

fn sort4<T>(v: &mut [T], a: [usize; 4], c: &mut impl FnMut(&T, &T) -> bool) {
    sort3(v, a[0], a[1], a[2], c);
    if c(&v[a[3]], &v[a[2]]) {
        v.swap(a[2], a[3]);
        if c(&v[a[2]], &v[a[1]]) {
            v.swap(a[1], a[2]);
            if c(&v[a[1]], &v[a[0]]) {
                v.swap(a[0], a[1]);
            }
        }
    }
}

fn sort5<T>(v: &mut [T], a: [usize; 5], c: &mut impl FnMut(&T, &T) -> bool) {
    sort4(v, [a[0], a[1], a[2], a[3]], c);
    if c(&v[a[4]], &v[a[3]]) {
        v.swap(a[3], a[4]);
        if c(&v[a[3]], &v[a[2]]) {
            v.swap(a[2], a[3]);
            if c(&v[a[2]], &v[a[1]]) {
                v.swap(a[1], a[2]);
                if c(&v[a[1]], &v[a[0]]) {
                    v.swap(a[0], a[1]);
                }
            }
        }
    }
}

/// Networks for 0..=5 elements; returns false when the range is longer.
fn small<T>(v: &mut [T], f: usize, l: usize, c: &mut impl FnMut(&T, &T) -> bool) -> bool {
    match l - f {
        0 | 1 => {}
        2 => {
            if c(&v[f + 1], &v[f]) {
                v.swap(f, f + 1);
            }
        }
        3 => {
            sort3(v, f, f + 1, f + 2, c);
        }
        4 => sort4(v, [f, f + 1, f + 2, f + 3], c),
        5 => sort5(v, [f, f + 1, f + 2, f + 3, f + 4], c),
        _ => return false,
    }
    true
}

fn insertion_sort<T: Clone>(
    v: &mut [T],
    f: usize,
    l: usize,
    c: &mut impl FnMut(&T, &T) -> bool,
    guarded: bool,
) {
    if f == l {
        return;
    }
    for i in f + 1..l {
        let mut j = i - 1;
        if c(&v[i], &v[j]) {
            let t = v[i].clone();
            let mut k = j;
            j = i;
            loop {
                v[j] = v[k].clone();
                j = k;
                if guarded && j == f {
                    break;
                }
                if !c(&t, &v[k - 1]) {
                    break;
                }
                k -= 1;
            }
            v[j] = t;
        }
    }
}

fn insertion_sort_incomplete<T: Clone>(
    v: &mut [T],
    f: usize,
    l: usize,
    c: &mut impl FnMut(&T, &T) -> bool,
) -> bool {
    if small(v, f, l, c) {
        return true;
    }
    let mut j = f + 2;
    sort3(v, f, f + 1, j, c);
    let mut count = 0;
    let mut i = j + 1;
    while i != l {
        if c(&v[i], &v[j]) {
            let t = v[i].clone();
            let mut k = j;
            j = i;
            loop {
                v[j] = v[k].clone();
                j = k;
                if j == f || !c(&t, &v[k - 1]) {
                    break;
                }
                k -= 1;
            }
            v[j] = t;
            count += 1;
            if count == 8 {
                return i + 1 == l;
            }
        }
        j = i;
        i += 1;
    }
    true
}

fn partition_equals_on_right<T: Clone>(
    v: &mut [T],
    begin: usize,
    end: usize,
    c: &mut impl FnMut(&T, &T) -> bool,
) -> (usize, bool) {
    let pivot = v[begin].clone();
    let mut first = begin;
    let mut last = end;
    loop {
        first += 1;
        if !c(&v[first], &pivot) {
            break;
        }
    }
    if begin == first - 1 {
        while first < last {
            last -= 1;
            if c(&v[last], &pivot) {
                break;
            }
        }
    } else {
        loop {
            last -= 1;
            if c(&v[last], &pivot) {
                break;
            }
        }
    }
    let already = first >= last;
    while first < last {
        v.swap(first, last);
        loop {
            first += 1;
            if !c(&v[first], &pivot) {
                break;
            }
        }
        loop {
            last -= 1;
            if c(&v[last], &pivot) {
                break;
            }
        }
    }
    let pos = first - 1;
    if begin != pos {
        v[begin] = v[pos].clone();
    }
    v[pos] = pivot;
    (pos, already)
}

fn partition_equals_on_left<T: Clone>(
    v: &mut [T],
    begin: usize,
    end: usize,
    c: &mut impl FnMut(&T, &T) -> bool,
) -> usize {
    let pivot = v[begin].clone();
    let mut first = begin;
    let mut last = end;
    if c(&pivot, &v[last - 1]) {
        loop {
            first += 1;
            if c(&pivot, &v[first]) {
                break;
            }
        }
    } else {
        loop {
            first += 1;
            if first >= last || c(&pivot, &v[first]) {
                break;
            }
        }
    }
    if first < last {
        loop {
            last -= 1;
            if !c(&pivot, &v[last]) {
                break;
            }
        }
    }
    while first < last {
        v.swap(first, last);
        loop {
            first += 1;
            if c(&pivot, &v[first]) {
                break;
            }
        }
        loop {
            last -= 1;
            if !c(&pivot, &v[last]) {
                break;
            }
        }
    }
    let pos = first - 1;
    if begin != pos {
        v[begin] = v[pos].clone();
    }
    v[pos] = pivot;
    first
}

fn sift_down<T: Clone>(
    v: &mut [T],
    f: usize,
    c: &mut impl FnMut(&T, &T) -> bool,
    len: usize,
    mut start: usize,
) {
    let mut child = start - f;
    if len < 2 || (len - 2) / 2 < child {
        return;
    }
    child = 2 * child + 1;
    let mut ci = f + child;
    if child + 1 < len && c(&v[ci], &v[ci + 1]) {
        ci += 1;
        child += 1;
    }
    if c(&v[ci], &v[start]) {
        return;
    }
    let top = v[start].clone();
    loop {
        v[start] = v[ci].clone();
        start = ci;
        if (len - 2) / 2 < child {
            break;
        }
        child = 2 * child + 1;
        ci = f + child;
        if child + 1 < len && c(&v[ci], &v[ci + 1]) {
            ci += 1;
            child += 1;
        }
        if c(&v[ci], &top) {
            break;
        }
    }
    v[start] = top;
}

fn heap_sort<T: Clone>(v: &mut [T], f: usize, l: usize, c: &mut impl FnMut(&T, &T) -> bool) {
    let n = l - f;
    if n > 1 {
        for start in (0..=(n - 2) / 2).rev() {
            sift_down(v, f, c, n, f + start);
        }
    }
    // __sort_heap: repeated __pop_heap with Floyd's sift-down and a final sift-up.
    let mut last = l;
    let mut len = n;
    while len > 1 {
        let top = v[f].clone();
        let mut hole = f;
        let mut ci = f;
        let mut child = 0usize;
        loop {
            ci += child + 1;
            child = 2 * child + 1;
            if child + 1 < len && c(&v[ci], &v[ci + 1]) {
                ci += 1;
                child += 1;
            }
            v[hole] = v[ci].clone();
            hole = ci;
            if child > (len - 2) / 2 {
                break;
            }
        }
        last -= 1;
        if hole == last {
            v[hole] = top;
        } else {
            v[hole] = v[last].clone();
            hole += 1;
            v[last] = top;
            // __sift_up(first, hole, len = hole - first)
            let mut ulen = hole - f;
            if ulen > 1 {
                ulen = (ulen - 2) / 2;
                let mut ptr = f + ulen;
                let mut ul = hole - 1;
                if c(&v[ptr], &v[ul]) {
                    let t = v[ul].clone();
                    loop {
                        v[ul] = v[ptr].clone();
                        ul = ptr;
                        if ulen == 0 {
                            break;
                        }
                        ulen = (ulen - 1) / 2;
                        ptr = f + ulen;
                        if !c(&v[ptr], &t) {
                            break;
                        }
                    }
                    v[ul] = t;
                }
            }
        }
        len -= 1;
    }
}

fn introsort<T: Clone>(
    v: &mut [T],
    mut first: usize,
    mut last: usize,
    c: &mut impl FnMut(&T, &T) -> bool,
    mut depth: usize,
    mut leftmost: bool,
) {
    const LIMIT: usize = 24;
    const NINTHER: usize = 128;
    loop {
        let len = last - first;
        if small(v, first, last, c) {
            return;
        }
        if len < LIMIT {
            insertion_sort(v, first, last, c, leftmost);
            return;
        }
        if depth == 0 {
            heap_sort(v, first, last, c);
            return;
        }
        depth -= 1;
        let half = len / 2;
        if len > NINTHER {
            sort3(v, first, first + half, last - 1, c);
            sort3(v, first + 1, first + half - 1, last - 2, c);
            sort3(v, first + 2, first + half + 1, last - 3, c);
            sort3(v, first + half - 1, first + half, first + half + 1, c);
            v.swap(first, first + half);
        } else {
            sort3(v, first + half, first, last - 1, c);
        }
        if !leftmost && !c(&v[first - 1], &v[first]) {
            first = partition_equals_on_left(v, first, last, c);
            continue;
        }
        let (i, already) = partition_equals_on_right(v, first, last, c);
        if already {
            let fs = insertion_sort_incomplete(v, first, i, c);
            if insertion_sort_incomplete(v, i + 1, last, c) {
                if fs {
                    return;
                }
                last = i;
                continue;
            } else if fs {
                first = i + 1;
                continue;
            }
        }
        introsort(v, first, i, c, depth, leftmost);
        leftmost = false;
        first = i + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1200 arrays of (key, input index), sizes 0..400 with 2, 7 and 1000 distinct keys.
    fn arrays() -> impl Iterator<Item = (usize, u64, Vec<(u32, usize)>)> {
        let mut state = 12345u64;
        (0..400usize).flat_map(move |n| {
            [2u64, 7, 1000]
                .into_iter()
                .map(|modulo| {
                    let v = (0..n)
                        .map(|i| {
                            state = state
                                .wrapping_mul(6364136223846793005)
                                .wrapping_add(1442695040888963407);
                            ((state >> 33) as u32 % modulo as u32, i)
                        })
                        .collect();
                    (n, modulo, v)
                })
                .collect::<Vec<_>>()
        })
    }

    #[test]
    fn element_order_matches_libcxx() {
        // FNV-1a of the lines "<n> <modulo> <ids...>" printed by the same generator in C++,
        // sorted with std::sort and a key-only lambda (Apple clang 21, macOS SDK 26.5 libc++).
        let mut hash = 0xcbf29ce484222325u64;
        for (n, modulo, mut v) in arrays() {
            sort_by(&mut v, |a, b| a.0 < b.0);
            assert!(v.windows(2).all(|w| w[0].0 <= w[1].0));
            let mut ids: Vec<_> = v.iter().map(|e| e.1).collect();
            ids.sort_unstable();
            assert_eq!(ids, (0..n).collect::<Vec<_>>());
            let line = format!("{n} {modulo}")
                + &v.iter().map(|e| format!(" {}", e.1)).collect::<String>()
                + "\n";
            for b in line.bytes() {
                hash = (hash ^ u64::from(b)).wrapping_mul(0x100000001b3);
            }
        }
        assert_eq!(hash, 0xd56cf7f54319ce6f);
    }

    #[test]
    fn heap_sort_fallback_sorts() {
        let mut v: Vec<(u32, usize)> = (0..100).rev().map(|i| (i as u32 % 9, i)).collect();
        let len = v.len();
        introsort(
            &mut v,
            0,
            len,
            &mut |a: &(u32, usize), b: &(u32, usize)| a.0 < b.0,
            0,
            true,
        );
        assert!(v.windows(2).all(|w| w[0].0 <= w[1].0));
    }
}
