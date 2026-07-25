# GPU acceleration backends

Minutes transcribes with whisper.cpp, which can run on the CPU (default) or on a
GPU through one of several backends. GPU backends are opt-in Cargo features.

| Feature | Backend | Platforms | Notes |
|---|---|---|---|
| *(none)* | CPU | all | Default. Works everywhere, no toolchain needed. |
| `metal` | Apple Metal | macOS (Apple Silicon) | Fast on M-series; needs full Xcode for the shader compiler. |
| `cuda` | NVIDIA CUDA | Linux, Windows | Needs the CUDA Toolkit + a supported GPU/driver. See the Pascal + Windows caveat below. |
| `vulkan` | Vulkan | Linux, Windows | GPU acceleration with **no CUDA Toolkit**. The most portable NVIDIA path on Windows. Needs the Vulkan SDK (LunarG). |
| `hipblas` | AMD ROCm/HIP | Linux | For AMD GPUs. |

Install with a single feature, for example:

```bash
cargo install minutes-cli --no-default-features --features vulkan
```

## Choosing a backend

- **Apple Silicon:** use `metal`.
- **NVIDIA on Linux:** `cuda` if your Toolkit + GPU generation are supported, otherwise `vulkan`.
- **NVIDIA on Windows:** prefer **`vulkan`**. It avoids the CUDA/nvcc/MSVC toolchain entirely, which is where most Windows build failures live (see below).
- **AMD on Linux:** `hipblas`.
- **Unsure, or the GPU build fights you:** the default CPU build always works. GPU is a speed optimization, not a requirement.

## Known limitation: CUDA on Pascal (GTX 10-series) + Windows

Building `--features cuda` on Windows for a **Pascal** GPU (compute capability
6.1 / `sm_61`, e.g. GTX 1050/1060/1070/1080) is not viable with current
toolchains, and there is no minutes-side fix because the failure is inside
NVIDIA's proprietary compiler, before any Minutes or whisper.cpp source
compiles:

- **CUDA 13.x** dropped Pascal support outright (`nvcc fatal: Unsupported gpu
  architecture 'compute_61'`).
- **CUDA 12.6** (the last Toolkit with full Pascal support) crashes during
  CMake's compiler-identification step: `nvcc error: 'cudafe++' died with status
  0xC0000005 (ACCESS_VIOLATION)`. `cudafe++` is NVIDIA's closed-source CUDA
  frontend; the crash is on the exact invocation CMake constructs, and nothing
  in the Minutes build controls it.
- **CUDA 11.8** fails to compile CMake's probe against the modern MSVC STL
  (`STL1002` and residual parser errors from nvcc 11.8's older frontend).

This is a dead toolchain combination, not a bug we can patch — `cudafe++` is
not open source, and the crash precedes whisper.cpp entirely, so neither a
CMake nor a whisper.cpp change would fix it.

**Do this instead — use Vulkan:**

```powershell
# Install the Vulkan SDK (LunarG) first, then:
cargo install minutes-cli --no-default-features --features vulkan --force
```

Vulkan gives you GPU acceleration on the same Pascal card with no CUDA Toolkit
and no nvcc, sidestepping every failure above. If you don't need GPU at all, the
default CPU build works out of the box.

Reference: issue [#531](https://github.com/silverstein/minutes/issues/531).
