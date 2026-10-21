// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
// Source (original): Mica exact-input memoization, written for this workspace.
// Changes: Bounded per-job Clipper results shared by scoped Rayon workers.

//! Exact, ordered Clipper input memoization. [`with_job`] owns the cache and worker pool;
//! returning (including unwinding) joins the workers and releases all cached geometry.
//! Calls outside that scope compute normally. No process-global geometry is retained.

use super::ExPolygon;
use clipper2_rust::Paths64;
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};

const SHARDS: usize = 16;
const BUDGET: usize = 64 * 1024 * 1024;
const SHARD_BYTES: usize = BUDGET / SHARDS;
const MAX_ENTRY_BYTES: usize = 1024 * 1024;

thread_local! {
    static CURRENT: RefCell<Option<Arc<Cache>>> = const { RefCell::new(None) };
}

/// All operation parameters, including the bits of floating-point parameters. The wrappers
/// use distinct tags for tree booleans and offsets followed by a positive tree union.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Operation(pub [u64; 7]);

struct Entry {
    op: Operation,
    subject: Paths64,
    clip: Paths64,
    result: Vec<ExPolygon>,
    bytes: usize,
}

#[derive(Default)]
struct Shard {
    entries: HashMap<u64, Arc<Entry>>,
    order: VecDeque<u64>,
    bytes: usize,
}

impl Shard {
    /// Inserts `entry` and returns the evicted entries, for the caller to release after
    /// unlocking the shard (freeing their geometry need not block other workers).
    fn insert(&mut self, hash: u64, entry: Arc<Entry>, budget: usize) -> Vec<Arc<Entry>> {
        // A hash collision is a miss, and may simply prevent insertion. Equality, never
        // the hash, decides reuse. FIFO has exactly one record per map entry.
        let mut evicted = Vec::new();
        if entry.bytes > MAX_ENTRY_BYTES || entry.bytes > budget || self.entries.contains_key(&hash)
        {
            evicted.push(entry);
            return evicted;
        }
        while self.bytes + entry.bytes > budget {
            let oldest = self.order.pop_front().unwrap();
            let old = self.entries.remove(&oldest).unwrap();
            self.bytes -= old.bytes;
            evicted.push(old);
        }
        self.bytes += entry.bytes;
        self.entries.insert(hash, entry);
        self.order.push_back(hash);
        evicted
    }
}

struct Cache([Mutex<Shard>; SHARDS]);
impl Default for Cache {
    fn default() -> Self {
        Self(std::array::from_fn(|_| Mutex::default()))
    }
}

/// Run one slice job with a 64 MiB cache. A scoped pool installs the same cache on every
/// worker, including nested parallel work, and isolates concurrently running jobs.
/// `MICA_CLIPPER_CACHE=0` disables reuse for output/performance comparisons.
/// Nested scopes reuse the outer job. Thread count follows Rayon's normal configuration.
pub fn with_job<T: Send>(work: impl FnOnce() -> T + Send) -> Result<T, String> {
    if CURRENT.with(|c| c.borrow().is_some())
        || std::env::var_os("MICA_CLIPPER_CACHE").is_some_and(|v| v == "0")
    {
        return Ok(work());
    }
    let cache = Arc::new(Cache::default());
    rayon::ThreadPoolBuilder::new()
        .build_scoped(
            |thread| {
                CURRENT.with(|c| *c.borrow_mut() = Some(Arc::clone(&cache)));
                thread.run();
                CURRENT.with(|c| c.borrow_mut().take());
            },
            |pool| pool.install(work),
        )
        .map_err(|e| format!("could not start slice workers: {e}"))
}

/// Run `work` on a second pool of the job's size that shares the job's cache. A worker that
/// waits in a join runs other queued work meanwhile, possibly a long unrelated job; a stage that
/// is a long chain of short parallel steps (tree support pathing runs one per layer) then waits
/// on such jobs at every step. On its own pool the stage only waits on its own work, while the
/// main pool keeps the rest of the slice busy. Falls back to the current pool when the
/// threads cannot be started.
pub fn on_side_pool<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    let cache = CURRENT.with(|c| c.borrow().clone());
    let mut work = Some(work);
    let slot = &mut work;
    let result = rayon::ThreadPoolBuilder::new()
        .num_threads(rayon::current_num_threads())
        .build_scoped(
            |thread| {
                CURRENT.with(|c| *c.borrow_mut() = cache.clone());
                thread.run();
                CURRENT.with(|c| c.borrow_mut().take());
            },
            |pool| pool.install(|| slot.take().map(|work| work())),
        );
    match result {
        Ok(Some(value)) => value,
        _ => (work.take().expect("side pool work not started"))(),
    }
}

fn same_paths(a: &Paths64, b: &Paths64) -> bool {
    // Clipper's Point equality deliberately ignores Z; the key must compare it too.
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|(a, b)| (a.x, a.y, a.z) == (b.x, b.y, b.z))
        })
}

fn digest(op: Operation, subject: &Paths64, clip: &Paths64) -> u64 {
    // Word-at-a-time non-cryptographic hash; collisions only reduce cache effectiveness.
    let mut h = 0x517c_c1b7_2722_0a95u64;
    let mut mix = |v: u64| {
        h = (h ^ v).rotate_left(27).wrapping_mul(0x9e37_79b1_85eb_ca87);
    };
    for v in op.0 {
        mix(v);
    }
    for paths in [subject, clip] {
        mix(paths.len() as u64);
        for path in paths {
            mix(path.len() as u64);
            for p in path {
                mix(p.x as u64);
                mix(p.y as u64);
                mix(p.z as u64);
            }
        }
    }
    h
}

fn paths_bytes(paths: &Paths64) -> usize {
    paths.capacity() * std::mem::size_of::<clipper2_rust::Path64>()
        + paths
            .iter()
            .map(|p| p.capacity() * std::mem::size_of::<clipper2_rust::Point64>())
            .sum::<usize>()
}

fn result_bytes(result: &[ExPolygon], capacity: usize) -> usize {
    capacity * std::mem::size_of::<ExPolygon>()
        + result
            .iter()
            .map(|e| {
                e.contour.capacity() * std::mem::size_of::<super::Point>()
                    + e.holes.capacity() * std::mem::size_of::<super::Polygon>()
                    + e.holes
                        .iter()
                        .map(|h| h.capacity() * std::mem::size_of::<super::Point>())
                        .sum::<usize>()
            })
            .sum::<usize>()
}

pub(super) fn get_or_build(
    op: Operation,
    subject: Paths64,
    clip: Paths64,
    build: impl FnOnce(&Paths64, &Paths64) -> Vec<ExPolygon>,
) -> Vec<ExPolygon> {
    let cache = CURRENT.with(|c| c.borrow().clone());
    let Some(cache) = cache else {
        return build(&subject, &clip);
    };
    if subject.is_empty() && clip.is_empty() {
        return build(&subject, &clip);
    }
    let hash = digest(op, &subject, &clip);
    let shard = &cache.0[hash as usize % SHARDS];
    let found = shard
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .entries
        .get(&hash)
        .cloned();
    if let Some(found) = found
        && found.op == op
        && same_paths(&found.subject, &subject)
        && same_paths(&found.clip, &clip)
    {
        return found.result.clone();
    }
    // Geometry, full equality and result copies all run outside the shard lock. Racing
    // misses may compute twice; scheduling never affects the returned geometry/order.
    let result = build(&subject, &clip);
    // Include owned vector capacities, Arc/entry storage and a conservative 512 bytes
    // per entry for hash buckets/FIFO/allocator metadata. Shard root storage is reserved.
    let bytes = paths_bytes(&subject)
        + paths_bytes(&clip)
        + result_bytes(&result, result.capacity())
        + std::mem::size_of::<Entry>()
        + 512;
    if bytes <= MAX_ENTRY_BYTES {
        let entry = Arc::new(Entry {
            op,
            subject,
            clip,
            result: result.clone(),
            bytes,
        });
        let evicted = shard.lock().unwrap_or_else(|p| p.into_inner()).insert(
            hash,
            entry,
            SHARD_BYTES - std::mem::size_of::<Shard>(),
        );
        drop(evicted);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use clipper2_rust::Point64;
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const OP: Operation = Operation([0, 2, 1, 0, 0, 0, 0]);
    fn paths() -> Paths64 {
        vec![vec![
            Point64::new(0, 0),
            Point64::new(10, 0),
            Point64::new(10, 10),
        ]]
    }
    fn marker() -> Vec<ExPolygon> {
        vec![ExPolygon {
            contour: vec![super::super::Point::new(17, 23)],
            holes: vec![],
        }]
    }
    fn entry(bytes: usize) -> Arc<Entry> {
        Arc::new(Entry {
            op: OP,
            subject: paths(),
            clip: vec![],
            result: marker(),
            bytes,
        })
    }

    #[test]
    fn collision_is_a_miss_and_every_parameter_and_coordinate_matters() {
        with_job(|| {
            let p = paths();
            let hash = digest(OP, &p, &vec![]);
            let cache = CURRENT.with(|c| c.borrow().clone().unwrap());
            // Force a collision with a different key, not merely a different hash.
            let mut collision = entry(1000);
            Arc::get_mut(&mut collision).unwrap().subject[0][0].z = 1;
            cache.0[hash as usize % SHARDS]
                .lock()
                .unwrap()
                .insert(hash, collision, SHARD_BYTES);
            let result = get_or_build(OP, p.clone(), vec![], |_, _| vec![]);
            assert!(result.is_empty());
            for i in 0..7 {
                let mut other = OP;
                other.0[i] += 1;
                let mut collision = entry(1000);
                Arc::get_mut(&mut collision).unwrap().op = other;
                let mut shard = cache.0[hash as usize % SHARDS].lock().unwrap();
                *shard = Shard::default();
                shard.insert(hash, collision, SHARD_BYTES);
                drop(shard);
                assert!(get_or_build(OP, p.clone(), vec![], |_, _| vec![]).is_empty());
            }
            let mut z = p.clone();
            z[0][0].z = 2;
            assert!(!same_paths(&p, &z));
            let mut reversed = p.clone();
            reversed[0].reverse();
            assert!(!same_paths(&p, &reversed));
            assert!(!same_paths(&p, &vec![p[0].clone(), vec![]]));
            assert!(get_or_build(OP, vec![], p, |_, _| vec![]).is_empty());
        })
        .unwrap();
    }

    #[test]
    fn byte_bound_fifo_and_oversize_bypass() {
        let mut shard = Shard::default();
        for h in 0..20 {
            shard.insert(h, entry(1000), 2500);
            assert!(shard.bytes <= 2500);
            assert_eq!(shard.entries.len(), shard.order.len());
        }
        assert_eq!(shard.entries.len(), 2);
        assert!(!shard.entries.contains_key(&17));
        assert!(shard.entries.contains_key(&18));
        shard.insert(21, entry(MAX_ENTRY_BYTES + 1), usize::MAX);
        assert!(!shard.entries.contains_key(&21));
    }

    #[test]
    fn workers_share_results_copies_are_independent_and_scope_drops() {
        let weak = with_job(|| {
            let cache = CURRENT.with(|c| c.borrow().clone().unwrap());
            let count = AtomicUsize::new(0);
            let mut first = get_or_build(OP, paths(), vec![], |_, _| {
                count.fetch_add(1, Ordering::Relaxed);
                marker()
            });
            first[0].contour.clear();
            (0..100).into_par_iter().for_each(|_| {
                assert_eq!(
                    get_or_build(OP, paths(), vec![], |_, _| {
                        count.fetch_add(1, Ordering::Relaxed);
                        vec![]
                    }),
                    marker()
                );
                with_job(|| {
                    CURRENT.with(|c| assert!(Arc::ptr_eq(c.borrow().as_ref().unwrap(), &cache)));
                })
                .unwrap();
            });
            assert_eq!(count.load(Ordering::Relaxed), 1);
            Arc::downgrade(&cache)
        })
        .unwrap();
        assert!(weak.upgrade().is_none());
        assert!(CURRENT.with(|c| c.borrow().is_none()));
        assert!(get_or_build(OP, paths(), vec![], |_, _| vec![]).is_empty());
    }

    #[test]
    fn concurrent_jobs_are_isolated_and_unwinding_releases_cache() {
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let run = |value| {
                with_job(|| {
                    let p = paths();
                    let expected = vec![ExPolygon {
                        contour: vec![super::super::Point::new(value, 0)],
                        holes: vec![],
                    }];
                    assert_eq!(
                        get_or_build(OP, p.clone(), vec![], |_, _| expected.clone()),
                        expected
                    );
                    barrier.wait();
                    assert_eq!(
                        get_or_build(OP, p, vec![], |_, _| panic!("must hit own job")),
                        expected
                    );
                })
                .unwrap()
            };
            let first = scope.spawn(move || run(1));
            run(2);
            first.join().unwrap();
        });
        let weak = Mutex::new(None);
        assert!(
            std::panic::catch_unwind(|| {
                let _ = with_job(|| {
                    CURRENT.with(|c| {
                        *weak.lock().unwrap() = Some(Arc::downgrade(c.borrow().as_ref().unwrap()))
                    });
                    panic!("job failed");
                });
            })
            .is_err()
        );
        assert!(weak.lock().unwrap().as_ref().unwrap().upgrade().is_none());
    }
}
