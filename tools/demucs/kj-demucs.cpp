// kj-demucs: takes the voices out of a clip's sound for «Озвучка» («Нейросети» add-on).
// Uses demucs.cpp (https://github.com/sevagh/demucs.cpp, MIT) with the htdemucs 4-source model.
//   kj-demucs <model.bin> <in.f32> <out.f32> <threads>
// in:  raw float32, stereo interleaved, 44100 Hz
// out: the same, everything but the vocals (drums + bass + other)
// Progress goes to stdout as lines "P <0..1>".
#include "model.hpp"
#include "threaded_inference.hpp"
#include <cstdio>
#include <fstream>
#include <iostream>
#include <vector>

int main(int argc, const char **argv) {
    if (argc != 5) { std::cerr << "usage: kj-demucs <model> <in.f32> <out.f32> <threads>\n"; return 2; }
    std::ifstream in(argv[2], std::ios::binary | std::ios::ate);
    size_t bytes = in.tellg(); in.seekg(0);
    std::vector<float> raw(bytes / sizeof(float));
    in.read(reinterpret_cast<char *>(raw.data()), raw.size() * sizeof(float));
    size_t n = raw.size() / 2;
    if (n < 4410) { std::cerr << "too short\n"; return 3; }
    Eigen::MatrixXf audio(2, n);
    for (size_t i = 0; i < n; ++i) { audio(0, i) = raw[2 * i]; audio(1, i) = raw[2 * i + 1]; }

    demucscpp::demucs_model model{};
    if (!demucscpp::load_demucs_model(argv[1], &model)) { std::cerr << "cannot load model\n"; return 4; }
    if (!model.is_4sources) { std::cerr << "need a 4-source model\n"; return 5; }
    int threads = std::max(1, std::atoi(argv[4]));
    Eigen::Tensor3dXf t = demucscppthreaded::threaded_inference(model, audio, threads);

    std::vector<float> out(n * 2);
    for (size_t i = 0; i < n; ++i)
        for (int c = 0; c < 2; ++c) { long k = (long)i; out[2 * i + c] = t(0, c, k) + t(1, c, k) + t(2, c, k); }
    std::ofstream o(argv[3], std::ios::binary);
    o.write(reinterpret_cast<const char *>(out.data()), out.size() * sizeof(float));
    std::cout << "P 1" << std::endl;
    return o.good() ? 0 : 6;
}
