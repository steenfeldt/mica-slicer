// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Ordered batch work for pipeline adapters, using the engine's existing Rayon pool.
//! Callers retain cross-item state outside these operations. No private pool or new threads.

use rayon::prelude::*;

pub fn map<T: Sync, R: Send>(items: &[T], f: impl Fn(usize, &T) -> R + Sync + Send) -> Vec<R> {
    items
        .par_iter()
        .enumerate()
        .map(|(i, item)| f(i, item))
        .collect()
}

pub fn for_each_mut<T: Send>(items: &mut [T], f: impl Fn(&mut T) + Sync + Send) {
    items.par_iter_mut().for_each(f);
}

/// Runs `a` and `b` side by side on the pool (`rayon::join`) and returns both results.
pub fn join<A: Send, B: Send>(
    a: impl FnOnce() -> A + Send,
    b: impl FnOnce() -> B + Send,
) -> (A, B) {
    rayon::join(a, b)
}
