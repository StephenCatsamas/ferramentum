// Permission/compatibility smoke fixture only. Its runtime is not a study timing.
#include <cuda_runtime.h>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <vector>

static void check(cudaError_t status) {
    if (status != cudaSuccess) {
        std::fprintf(stderr, "CUDA error: %s\n", cudaGetErrorString(status));
        std::exit(1);
    }
}

__global__ void counter_probe(const std::uint32_t* input,
                              std::uint32_t* output, std::uint32_t size) {
    const std::uint32_t i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < size) output[i] = input[i] + 1u;
}

int main() {
    constexpr std::uint32_t size = 1u << 20;
    constexpr std::size_t bytes = size * sizeof(std::uint32_t);
    std::vector<std::uint32_t> input(size), output(size);
    for (std::uint32_t i = 0; i < size; ++i) input[i] = i ^ 0xffffffffu;
    check(cudaSetDevice(0));
    cudaDeviceProp device{};
    check(cudaGetDeviceProperties(&device, 0));
    std::fprintf(stderr, "GPU: %s; SM %d.%d; fixture bytes: %zu\n",
                 device.name, device.major, device.minor, 2 * bytes);
    std::fprintf(stderr, "GPU UUID: ");
    for (unsigned char byte : device.uuid.bytes) std::fprintf(stderr, "%02x", byte);
    std::fprintf(stderr, "\n");
    std::uint32_t* device_input = nullptr;
    std::uint32_t* device_output = nullptr;
    check(cudaMalloc(reinterpret_cast<void**>(&device_input), bytes));
    check(cudaMalloc(reinterpret_cast<void**>(&device_output), bytes));
    check(cudaMemcpy(device_input, input.data(), bytes, cudaMemcpyHostToDevice));
    counter_probe<<<size / 256, 256>>>(device_input, device_output, size);
    check(cudaGetLastError());
    check(cudaDeviceSynchronize());
    check(cudaMemcpy(output.data(), device_output, bytes, cudaMemcpyDeviceToHost));
    for (std::uint32_t i = 0; i < size; ++i) {
        if (output[i] != input[i] + 1u) {
            std::fprintf(stderr, "Incorrect output at %u\n", i);
            return 2;
        }
    }
    check(cudaFree(device_output));
    check(cudaFree(device_input));
    std::fprintf(stderr, "PASS: all %u unsigned outputs checked\n", size);
    return 0;
}
