# Building LiteRT-LM Native Libraries

Pre-built libraries for supported platforms are available on the [Releases page](https://github.com/gveeden/lite-llm/releases). Download them if you can — building from source takes 20–40 minutes.

---

## Platform notes

| Platform | Architecture | Supported |
|---|---|---|
| Asahi Linux (Apple M-series) | aarch64 | ✓ |
| Raspberry Pi 4/5 — 64-bit OS | aarch64 | ✓ |
| Raspberry Pi 3/4 — 32-bit OS | armv7l | ✗ |
| Linux x86_64 | x86_64 | Build untested |

Raspberry Pi must be running **64-bit Raspberry Pi OS** (Bookworm or later). Check with `uname -m` — you need `aarch64`, not `armv7l`.

---

## Build on Raspberry Pi 4/5

### 1. Flash 64-bit Raspberry Pi OS

Use Raspberry Pi Imager and choose **Raspberry Pi OS (64-bit)**. Verify after boot:

```bash
uname -m   # must print: aarch64
```

### 2. Install Bazel

Bazel is not in the Pi OS apt repos. Install via Bazelisk:

```bash
sudo apt update
sudo apt install -y curl openjdk-17-jdk python3 python3-dev build-essential zip unzip git

# Install Bazelisk as `bazel`
curl -Lo /usr/local/bin/bazel \
  https://github.com/bazelbuild/bazelisk/releases/latest/download/bazelisk-linux-arm64
chmod +x /usr/local/bin/bazel
bazel version   # downloads the correct Bazel version on first run
```

### 3. Install C++ build dependencies

```bash
sudo apt install -y clang libc++-dev libc++abi-dev
```

### 4. Clone LiteRT-LM

```bash
git clone https://github.com/google-ai-edge/LiteRT-LM.git
cd LiteRT-LM
```

### 5. Build libengine.so

```bash
bazel build \
  --config=linux \
  --jobs=$(nproc) \
  //c:libengine.so
```

This takes 20–40 minutes on a Pi 4. A Pi 5 is noticeably faster.

### 6. Copy the output libraries

```bash
mkdir -p ~/litert-lm-libs
cp bazel-bin/litert_lm/libengine.so ~/litert-lm-libs/

# Copy any additional runtime .so files Bazel produced
find bazel-bin -name '*.so' -not -path '*/\.*' \
  | xargs -I{} cp {} ~/litert-lm-libs/ 2>/dev/null || true
```

---

## Build on Asahi Linux (Apple M-series)

### 1. Install Bazel

```bash
# Arch / Asahi
sudo pacman -S bazel   # or use Bazelisk (see Pi instructions above)
```

### 2. Install dependencies

```bash
sudo pacman -S clang libc++ git python
```

### 3. Clone and build

```bash
git clone https://github.com/google-ai-edge/LiteRT-LM.git
cd LiteRT-LM

bazel build \
  --config=linux \
  --jobs=$(nproc) \
  //c:libengine.so
```

### 4. Copy output

```bash
mkdir -p ~/system/litert-lm-libs
cp bazel-bin/litert_lm/libengine.so ~/system/litert-lm-libs/
find bazel-bin -name '*.so' -not -path '*/\.*' \
  | xargs -I{} cp {} ~/system/litert-lm-libs/ 2>/dev/null || true
```

---

## GPU build on Asahi Linux (Honeykrisp / Vulkan)

The upstream LiteRT-LM BUILD files have GPU stubs that are intentionally empty. This section documents how to patch them to enable GPU acceleration via the LiteRT OpenCL delegate, which on Asahi runs through Mesa's rusticl → Honeykrisp Vulkan → Apple GPU.

> **Status:** experimental. Confirmed working on Apple M1 Max (OpenCL 3.0 via Mesa rusticl + Honeykrisp). Inference falls back to CPU if OpenCL device initialisation fails at runtime.

### How it works

```
LiteRT GPU delegate → OpenCL API → Mesa rusticl → Honeykrisp Vulkan driver → Apple GPU
```

The LiteRT GPU delegate uses OpenCL on Linux. On Asahi, Mesa's rusticl implements OpenCL on top of the Vulkan driver, so Honeykrisp still does the compute.

### 1. Install system dependencies

```bash
sudo pacman -S opencl-icd-loader ocl-icd-loader opencl-clhpp-headers clinfo
```

or 

```bash
# ICD loader, headers, and the Mesa rusticl OpenCL implementation
sudo dnf install OpenCL-ICD-Loader opencl-headers clinfo mesa-libOpenCL
```

Verify Mesa exposes an OpenCL device for the Apple GPU:

```bash
RUSTICL_ENABLE=asahi clinfo | grep -E 'Device Name|OpenCL C'
```

You should see the Apple GPU listed. If it shows nothing, check that `mesa-libOpenCL` is installed (`dnf list installed mesa-libOpenCL`) and that `/etc/OpenCL/vendors/` contains an ICD file pointing to the Mesa rusticl library.

### 2. Patch the LiteRT-LM BUILD files

These two targets are empty stubs in the upstream repo. Fill them in:

**`runtime/executor/BUILD`** — find `default_static_gpu_accelerator` and replace:

```python
# Before:
cc_library(
    name = "default_static_gpu_accelerator",
    deps = select({
        "@litert//litert:litert_link_capi_so": [],
        "//conditions:default": [],
    }) + select({
        "//conditions:default": [],
    }),
)

# After:
cc_library(
    name = "default_static_gpu_accelerator",
    deps = select({
        "@litert//litert:litert_link_capi_so": [],
        "//conditions:default": [
            "@litert//tflite/delegates/gpu:delegate",
        ],
    }),
)
```

**`runtime/components/BUILD`** — find `default_static_gpu_samplers` and replace:

```python
# Before:
cc_library(
    name = "default_static_gpu_samplers",
    deps = select({
        "@litert//litert:litert_link_capi_so": [],
        "//conditions:default": [],
    }) + select({
        "//conditions:default": [],
    }),
)

# After:
cc_library(
    name = "default_static_gpu_samplers",
    deps = select({
        "@litert//litert:litert_link_capi_so": [],
        "//conditions:default": [
            "@litert//tflite/delegates/gpu:delegate",
        ],
    }),
)
```

### 3. Build with OpenCL / no-GL flags

The GPU delegate on Linux needs `CL_DELEGATE_NO_GL` to avoid pulling in EGL/OpenGL display dependencies (there is no display server requirement for inference):

```bash
bazel build \
  --config=linux \
  --jobs=$(nproc) \
  --copt=-DCL_DELEGATE_NO_GL \
  --copt=-DTFLITE_GPU_BINARY_RELEASE \
  //c:libengine.so
```

`TFLITE_GPU_BINARY_RELEASE` trims some debug/testing deps that can cause link issues outside Google's internal build environment.

### 4. Copy output and set up the runtime environment

```bash
mkdir -p ~/system/litert-lm-libs
cp bazel-bin/litert_lm/libengine.so ~/system/litert-lm-libs/
find bazel-bin -name '*.so' -not -path '*/\.*' \
  | xargs -I{} cp {} ~/system/litert-lm-libs/ 2>/dev/null || true
```

At runtime, rusticl needs to know to expose the Asahi GPU as an OpenCL device:

```bash
export RUSTICL_ENABLE=asahi
./lite-llm
```

Or add it to your `~/.config/lite-llm/env` or a systemd unit `Environment=` line.

### 5. Verify GPU is being used

If the GPU delegate initialises successfully you will see a log line like:

```
Created OpenCL-based gpu delegate
```

in stderr at startup. If it falls back to CPU you will see nothing or a warning about OpenCL device not found — inference still works, just CPU-only.

---

## Publishing a release

Once you have libraries built for a platform, attach them to a GitHub release:

```bash
# Tag and create the release
git tag v0.1.0
git push origin v0.1.0

# Upload the .so files (run once per platform)
gh release create v0.1.0 ~/system/litert-lm-libs/*.so \
  --title "v0.1.0" \
  --notes "See release notes."

# Add Pi libs to the same release
gh release upload v0.1.0 ~/litert-lm-libs/*.so
```

Name the assets clearly: `libengine-linux-aarch64.so`, `libengine-linux-aarch64-pi.so`, etc. if they differ.
