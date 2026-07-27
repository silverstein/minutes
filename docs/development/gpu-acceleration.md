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

Install by adding a GPU feature to the defaults, for example:

```bash
cargo install minutes-cli --features vulkan
```

> **Do not pass `--no-default-features` with only a GPU feature.** `whisper` is a
> default feature; dropping the defaults builds a binary with no transcription
> ("Transcription placeholder — whisper feature not enabled"). If you must use
> `--no-default-features`, name `whisper` explicitly:
> `cargo install minutes-cli --no-default-features --features whisper,vulkan`.

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

**Try Vulkan instead — but see the shader-gen caveat below:**

```powershell
# Install the Vulkan SDK (LunarG) first, then:
cargo install minutes-cli --features vulkan --force
```

Vulkan avoids the CUDA Toolkit and nvcc entirely, sidestepping every failure
above. Keep the default features (do not pass `--no-default-features` with only
`vulkan`, or you get a placeholder binary with no transcription — see the note
in the install section above).

### Vulkan also fails on some Windows configs: `vulkan-shaders-gen` hangs

On at least one Windows machine (a dual-GPU laptop: Pascal GTX 1050 + Intel UHD
630, Vulkan SDK 1.4.350), the `vulkan` build hangs indefinitely in whisper.cpp's
`ggml-vulkan` step: `vulkan-shaders-gen.exe` sits at 0% CPU forever (reproducible
serial and parallel, and by running the extracted command manually outside the
build). This is an **upstream ggml/whisper.cpp build-tool hang**, not a Minutes
issue, and it is under investigation upstream (issue #531). A 0%-CPU block
(rather than a spin) points at the tool waiting on a subprocess/device call, and
dual-GPU device enumeration is a suspected factor.

**If both CUDA and Vulkan fail on your machine, use the CPU build** — it always
works, needs no toolkit, and on a low-end GPU like a GTX 1050 the GPU speedup is
modest anyway:

```powershell
cargo install minutes-cli --force
```

Reference: issue [#531](https://github.com/silverstein/minutes/issues/531).
