// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
//
// Replays a `MICA_CLIPPER_DUMP` capture (format in crates/clipper2/src/capture.rs) through the C++
// Clipper2 library, timing only the clipper calls, so the same inputs can be compared with
// `cargo run --release -p mica-core --example clipper_bench`. Build with USINGZ to match Mica:
//
//   clang++ -O3 -DNDEBUG -DUSINGZ -std=c++17 -I$CLIPPER2/include scripts/bench-clipper2.cpp \
//     $CLIPPER2/src/clipper.engine.cpp $CLIPPER2/src/clipper.offset.cpp -o /tmp/bench-clipper2
//   /tmp/bench-clipper2 /tmp/cat.clip
//
// Nested records (an offset's own finishing union) are skipped: the offset record times them.

#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <vector>

#include "clipper2/clipper.h"

// OrcaSlicer renames the USINGZ build: pass -DCLIPPER2_NS=Clipper2Lib_Z for its deps_src copy.
#ifndef CLIPPER2_NS
#define CLIPPER2_NS Clipper2Lib
#endif
using namespace CLIPPER2_NS;

namespace {

struct Reader {
  FILE *f;
  bool ok = true;
  template <typename T> T get() {
    T v{};
    if (std::fread(&v, sizeof(T), 1, f) != 1) ok = false;
    return v;
  }
  Paths64 paths() {
    uint32_t n = get<uint32_t>();
    Paths64 out;
    out.reserve(n);
    std::vector<int64_t> raw;
    for (uint32_t i = 0; i < n && ok; ++i) {
      uint32_t m = get<uint32_t>();
      raw.resize(size_t(m) * 3);
      if (m && std::fread(raw.data(), sizeof(int64_t), raw.size(), f) != raw.size()) ok = false;
      Path64 p;
      p.reserve(m);
      for (uint32_t j = 0; j < m; ++j)
#ifdef USINGZ
        p.emplace_back(raw[3 * j], raw[3 * j + 1], raw[3 * j + 2]);
#else
        p.emplace_back(raw[3 * j], raw[3 * j + 1]);
#endif
      out.push_back(std::move(p));
    }
    return out;
  }
};

struct Sum {
  uint64_t points = 0, paths = 0, hash = 1469598103934665603ull, hash_z = hash;
  void add(const Paths64 &ps) {
    for (const auto &p : ps) {
      ++paths;
      for (const auto &pt : p) {
        ++points;
        hash = (hash ^ uint64_t(pt.x)) * 1099511628211ull;
        hash = (hash ^ uint64_t(pt.y)) * 1099511628211ull;
#ifdef USINGZ
        hash_z = (hash_z ^ uint64_t(pt.z)) * 1099511628211ull;
#endif
      }
    }
  }
};

void tree_paths(const PolyPath64 &node, Paths64 &out) {
  for (const auto &child : node) {
    out.push_back(child->Polygon());
    tree_paths(*child, out);
  }
}

}  // namespace

int main(int argc, char **argv) {
  if (argc < 2) {
    std::fprintf(stderr, "usage: %s CAPTURE\n", argv[0]);
    return 2;
  }
  // CLIPPER_BENCH_PARSE_ONLY=1 reads the capture without clipping (process-cost baseline).
  const bool parse_only = std::getenv("CLIPPER_BENCH_PARSE_ONLY") != nullptr;
  Reader r{std::fopen(argv[1], "rb")};
  if (!r.f) return 1;
  const char *names[] = {"boolean intersection", "boolean union", "boolean difference",
                         "offset square", "offset bevel", "offset round", "offset miter"};
  double secs[7] = {0};
  uint64_t counts[7] = {0};
  Sum sum;
  while (true) {
    int c = std::fgetc(r.f);
    if (c == EOF) break;
    uint8_t nested = r.get<uint8_t>();
    Paths64 result;
    std::chrono::steady_clock::time_point t0, t1;
    int slot = -1;
    if (c == 0) {
      uint8_t ct = r.get<uint8_t>(), fr = r.get<uint8_t>(), tree = r.get<uint8_t>();
      uint8_t pc = r.get<uint8_t>(), rev = r.get<uint8_t>();
      Paths64 subj = r.paths(), open = r.paths(), clip = r.paths();
      if (!r.ok) break;
      if (nested || parse_only) continue;
      slot = ct == 1 ? 0 : ct == 2 ? 1 : 2;
      t0 = std::chrono::steady_clock::now();
      Clipper64 cl;
      cl.PreserveCollinear(pc);
      cl.ReverseSolution(rev);
      if (!subj.empty()) cl.AddSubject(subj);
      if (!open.empty()) cl.AddOpenSubject(open);
      if (!clip.empty()) cl.AddClip(clip);
      Paths64 open_out;
      if (tree) {
        PolyTree64 pt;
        cl.Execute(ClipType(ct), FillRule(fr), pt, open_out);
        t1 = std::chrono::steady_clock::now();
        tree_paths(pt, result);
      } else {
        cl.Execute(ClipType(ct), FillRule(fr), result, open_out);
        t1 = std::chrono::steady_clock::now();
      }
      result.insert(result.end(), open_out.begin(), open_out.end());
    } else {
      double delta = r.get<double>(), miter = r.get<double>(), arc = r.get<double>();
      uint8_t pc = r.get<uint8_t>(), rev = r.get<uint8_t>(), tree = r.get<uint8_t>();
      uint32_t groups = r.get<uint32_t>();
      std::vector<std::pair<std::pair<uint8_t, uint8_t>, Paths64>> gs;
      for (uint32_t g = 0; g < groups && r.ok; ++g) {
        uint8_t jt = r.get<uint8_t>(), et = r.get<uint8_t>();
        gs.push_back({{jt, et}, r.paths()});
      }
      if (!r.ok) break;
      if (nested || parse_only) continue;
      slot = 3 + (gs.empty() ? 0 : gs[0].first.first);
      t0 = std::chrono::steady_clock::now();
      ClipperOffset co(miter, arc, pc, rev);
      for (auto &g : gs) co.AddPaths(g.second, JoinType(g.first.first), EndType(g.first.second));
      if (tree) {
        PolyTree64 pt;
        co.Execute(delta, pt);
        t1 = std::chrono::steady_clock::now();
        tree_paths(pt, result);
      } else {
        co.Execute(delta, result);
        t1 = std::chrono::steady_clock::now();
      }
    }
    secs[slot] += std::chrono::duration<double>(t1 - t0).count();
    counts[slot] += 1;
    sum.add(result);
  }
  double total = 0;
  for (int i = 0; i < 7; ++i) {
    if (!counts[i]) continue;
    std::printf("%-22s %9llu calls %9.3f s\n", names[i], (unsigned long long)counts[i], secs[i]);
    total += secs[i];
  }
  std::printf("%-22s %15s %9.3f s\n", "total", "", total);
  std::printf("output: %llu paths, %llu points, xy hash %016llx, z hash %016llx\n",
              (unsigned long long)sum.paths, (unsigned long long)sum.points,
              (unsigned long long)sum.hash, (unsigned long long)sum.hash_z);
}
