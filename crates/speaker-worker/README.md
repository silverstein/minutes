# Opt-in full speaker refinement worker

This separately built CPU helper runs the pinned Speakrs reconstruction/PLDA/VBx
pipeline. It is outside the main workspace and is not downloaded, auto-selected,
or bundled in Minutes releases. Existing engines and live transcription retain
their current behavior. This is an experimental anonymous-label engine;
production default promotion and named voice enrollment remain unqualified.

Build with the repository's Rust 1.95.0 toolchain:

On Linux, install your distribution's OpenBLAS development package first
(`libopenblas-dev` on Debian/Ubuntu); the resulting helper also needs the system
OpenBLAS shared library at run time. This enables [ndarray's BLAS
matrix-multiplication path](https://docs.rs/ndarray/0.17.2/ndarray/#crate-feature-flags)
in the standalone helper. [OpenBLAS](https://github.com/OpenMathLib/OpenBLAS)
provides the optimized system CPU library. macOS uses Accelerate, including the
legacy CBLAS path on systems without upstream's newer BLAS thread API.
These BLAS dependencies belong to the standalone helper.

Windows uses statically linked OpenBLAS so the supervisor can continue binding
and launching one immutable executable. Build the pinned vcpkg manifest from a
Developer PowerShell with C++ build tools, CMake and Ninja available:

```powershell
git clone https://github.com/microsoft/vcpkg C:/src/minutes-vcpkg
git -C C:/src/minutes-vcpkg checkout 664933b8d385dfc01f4e0cdff4c7c74e71f6a740
& C:/src/minutes-vcpkg/bootstrap-vcpkg.bat -disableMetrics
& C:/src/minutes-vcpkg/vcpkg.exe install --x-manifest-root=crates/speaker-worker --x-install-root=C:/src/minutes-vcpkg/installed --triplet=x64-windows-static-md-release --disable-metrics
$env:VCPKG_ROOT = 'C:/src/minutes-vcpkg'
$env:VCPKGRS_TRIPLET = 'x64-windows-static-md-release'
```

Do not set `VCPKGRS_DYNAMIC`: the helper requires static BLAS linkage. The
[vcpkg Rust build helper](https://docs.rs/vcpkg/0.2.15/vcpkg/) discovers this
installation. The pinned Microsoft port builds OpenBLAS 0.3.34 from source;
its license notices remain in the vcpkg installation. No prebuilt OpenBLAS DLL
is downloaded or added to the runtime executable search path.

```bash
RUSTFLAGS="-C target-cpu=native" cargo build --release --locked --manifest-path crates/speaker-worker/Cargo.toml
```

In PowerShell, set `$env:RUSTFLAGS = '-C target-cpu=native'` before the Cargo
command. Build on the machine that will run the helper: Rust's
[`target-cpu=native`](https://doc.rust-lang.org/rustc/codegen-options/index.html#target-cpu)
enables that CPU's instructions. Do not distribute this executable or its
compiled cache to other machines. This flag applies only to the separate helper;
build the Minutes host with its ordinary portable flags (in PowerShell, first
run `Remove-Item Env:RUSTFLAGS`). The native regression
workflow does not restore compiled helper artifacts across runners. CPU-specific
compilation is an optimization under evaluation, not a claim that Intel runtime
qualification has passed.

Code: Apache-2.0 Speakrs at `af8013f95a53a37a554d8abc6e4ae2ab9a614be6`;
Minutes wrapper MIT. Default upstream features are disabled; only `cpu` is
compiled. The helper never downloads models. Its separate Cargo.lock pins the
dependency graph without adding Speakrs to the desktop/CLI dependency graph.

Download and verify the explicit public bundle using the repository evaluation
fetcher, after reviewing model attribution and upstream terms:

```bash
python3 tests/eval/diarization/fetch_full_pipeline.py \
  --manifest tests/eval/diarization/full-native-ami.json --directory /owned/full-native
```

This also fetches public AMI evaluation audio and baseline models. The helper
uses only `/owned/full-native/candidate-models`. The converted model card has
no separate converter license declaration and points to upstream model terms;
this wrapper does not redistribute weights.

Opt in by editing your configuration deliberately (all paths must be absolute):

The released Minutes 0.30.4 host does not implement this bridge. Build the
Minutes CLI or desktop host from the same source revision as this helper, with
the `diarize` feature enabled, before selecting this engine. Building the helper
alone does not upgrade an installed app. For an offline check without changing
an installed app or your configuration, use the probe below.

```toml
[diarization]
engine = "speakrs"
speaker_worker_path = "/absolute/minutes/crates/speaker-worker/target/release/minutes-speaker-worker"
model_path = "/owned/full-native/candidate-models"
```

`minutes setup --diarization` refuses to overwrite this bundle with legacy ONNX
models. Switch `engine` back to `auto` and restore the ordinary ONNX model path
to return to the previous configuration. Neither configuration is changed by
building, fetching, or running the probe below.

```bash
cargo build --release --locked -p minutes-core --no-default-features \
  --features diarize --example full_diarize_file
target/release/examples/full_diarize_file \
  /absolute/minutes-speaker-worker /owned/full-native/candidate-models /owned/meeting.wav
```

The current bridge supports ordinary WAV recordings, up to one hour, resampled
by Minutes to mono 16 kHz. Compressed and anonymous private-capability inputs are
refused rather than weakening their transport rules. Transcription remains
available when the optional speaker worker fails. The worker receives canonical
PCM through stdin and no audio filename. It returns only anonymous activity,
with a digest and sample count binding the response to the input. No embeddings,
names, transcript text, or enrollment writes cross the protocol. Existing
profiles stay in their own namespaces; enrollment for this engine is refused.

The host uses its existing immutable executable binding and process-tree
supervisor: finite deadline, bounded stdout/stderr, explicit cancellation and
Unix process-group / Windows Job retirement. Linux has a 4 GiB virtual-address
ceiling; Windows process/job committed memory is limited to 4 GiB before resume.
macOS installs a 4 GiB virtual-address growth allowance over its dyld baseline
before reading input or constructing models. These memory measures differ;
none is an RSS measurement. A worker-local watchdog also bounds parent loss.
Thread environment settings and Unix nice priority are scheduling hints, not
proof that concurrent capture is unaffected. Model copies are hashed before
inference reopens their private snapshot. Duration, observation, speaker,
segment and output bounds precede acceptance of results.

The helper observes only the pinned pipeline's segmentation and embedding
completion events. It emits fixed stage labels and cumulative elapsed
milliseconds from inference start, bounded by the existing 900-second budget.
The host discards arbitrary tracing fields, paths and messages; diagnostic
receipts retain only the closed labels and numeric timings. These observations
do not change inference parameters or qualify a failed runtime gate.

Preserving overlap in model output does not resolve which overlapping speaker
said an ASR word. Existing conservative attribution remains in place. Real ASR
alignment, multilingual/short-turn/similar-voice/headphone cases, named false
positives, installed-app parity and concurrent live latency remain rollout gates.
