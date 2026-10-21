// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
// Benchmark driver only; uses the Boost.Polygon API used by PrusaSlicer.
// clang++ -O3 -DNDEBUG -std=c++17 -I<existing Boost include root> scripts/bench-voronoi.cpp -o /tmp/bench-voronoi
#include <boost/polygon/voronoi.hpp>
#include <boost/polygon/segment_data.hpp>
#include <chrono>
#include <cstdint>
#include <fstream>
#include <iostream>
#include <stdexcept>
#include <vector>

static std::uint32_t read_u32(std::istream& input) {
    unsigned char bytes[4];
    if (!input.read(reinterpret_cast<char*>(bytes), 4))
        throw std::runtime_error("truncated capture");
    return std::uint32_t(bytes[0]) | (std::uint32_t(bytes[1]) << 8) |
           (std::uint32_t(bytes[2]) << 16) | (std::uint32_t(bytes[3]) << 24);
}

int main(int argc, char** argv) {
    if (argc < 2) { std::cerr << "usage: bench-voronoi CAPTURE [REPEATS]\n"; return 2; }
    std::ifstream input(argv[1], std::ios::binary);
    if (!input) throw std::runtime_error("cannot open capture");
    using Segment = boost::polygon::segment_data<std::int32_t>;
    using Point = boost::polygon::point_data<std::int32_t>;
    std::vector<std::vector<Segment>> inputs;
    std::size_t segments = 0, vertices = 0;
    while (input.peek() != std::char_traits<char>::eof()) {
        const auto count = read_u32(input);
        std::vector<Segment> lines;
        lines.reserve(count);
        for (std::uint32_t i = 0; i < count; ++i) {
            const auto ax = static_cast<std::int32_t>(read_u32(input));
            const auto ay = static_cast<std::int32_t>(read_u32(input));
            const auto bx = static_cast<std::int32_t>(read_u32(input));
            const auto by = static_cast<std::int32_t>(read_u32(input));
            lines.emplace_back(Point(ax, ay), Point(bx, by));
        }
        segments += count;
        inputs.push_back(std::move(lines));
    }
    const auto repeats = argc > 2 ? std::stoul(argv[2]) : 1;
    const auto start = std::chrono::steady_clock::now();
    for (unsigned long repeat = 0; repeat < repeats; ++repeat) {
        for (const auto& lines : inputs) {
            boost::polygon::voronoi_diagram<double> diagram;
            boost::polygon::construct_voronoi(lines.begin(), lines.end(), &diagram);
            vertices += diagram.num_vertices();
        }
    }
    const std::chrono::duration<double> duration = std::chrono::steady_clock::now() - start;
    std::cout << "diagrams=" << inputs.size() << " segments=" << segments
              << " repeats=" << repeats << " vertices=" << vertices
              << " sweep_s=" << duration.count() << '\n';
}
