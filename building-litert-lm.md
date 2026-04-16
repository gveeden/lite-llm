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
  //litert_lm:libengine.so
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
  //litert_lm:libengine.so
```

### 4. Copy output

```bash
mkdir -p ~/system/litert-lm-libs
cp bazel-bin/litert_lm/libengine.so ~/system/litert-lm-libs/
find bazel-bin -name '*.so' -not -path '*/\.*' \
  | xargs -I{} cp {} ~/system/litert-lm-libs/ 2>/dev/null || true
```

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
