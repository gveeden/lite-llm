FROM rust:slim-trixie AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    cmake \
    clang \
    libclang-dev \
    curl \
    libc++-dev \
    libc++abi-dev \
    git \
    libvulkan-dev \
    glslang-tools \
    glslc \
    && rm -rf /var/lib/apt/lists/*

# The libvulkan-dev headers on Bookworm (1.3.239) are too old for llama-cpp-2
# We install newer headers directly from KhronosGroup to fix missing VK_EXT_layer_settings types
RUN git clone --depth 1 -b v1.3.280 https://github.com/KhronosGroup/Vulkan-Headers.git /tmp/Vulkan-Headers && \
    cd /tmp/Vulkan-Headers && cmake -S . -B build && cmake --install build && \
    rm -rf /tmp/Vulkan-Headers

# llama-cpp-2 requires rustfmt and Vulkan headers/glslc for the Vulkan feature
RUN rustup component add rustfmt

WORKDIR /usr/src/lite-llm

# Download pre-built LiteRT-LM libraries directly to the build environment
# (These will be linked during the build)
RUN mkdir -p /litert-lm-libs && \
    curl -L https://github.com/gveeden/lite-llm/releases/download/v0.1.0/libengine.so -o /litert-lm-libs/libengine.so && \
    curl -L https://github.com/gveeden/lite-llm/releases/download/v0.1.0/libGemmaModelConstraintProvider.so -o /litert-lm-libs/libGemmaModelConstraintProvider.so && \
    curl -L https://github.com/gveeden/lite-llm/releases/download/v0.1.0/libLiteRt.so -o /litert-lm-libs/libLiteRt.so && \
    curl -L https://github.com/gveeden/lite-llm/releases/download/v0.1.0/libLiteRtTopKWebGpuSampler.so -o /litert-lm-libs/libLiteRtTopKWebGpuSampler.so && \
    curl -L https://github.com/gveeden/lite-llm/releases/download/v0.1.0/libLiteRtWebGpuAccelerator.so -o /litert-lm-libs/libLiteRtWebGpuAccelerator.so

# Set the lib path so build.rs links it correctly and bakes it into RPATH
ENV LITERT_LM_LIB_PATH=/litert-lm-libs

COPY . .

# Force the use of clang instead of GCC for compiling C/C++ dependencies like llama.cpp
ENV CC=clang
ENV CXX=clang++
ENV LLAMA_NATIVE=OFF

# Build the release binary
RUN cargo build --release

# -------------------------
# Final runtime stage
# -------------------------
FROM debian:trixie-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libc++1 \
    libc++abi1 \
    libssl3 \
    sqlite3 \
    libvulkan1 \
    && rm -rf /var/lib/apt/lists/*

# Create a non-root user matching the pod's securityContext (uid 1000, gid 1000)
RUN groupadd -g 1000 appgroup && \
    useradd -u 1000 -g 1000 -s /bin/sh -m appuser

WORKDIR /app

# Copy the pre-built LiteRT-LM libraries from builder
COPY --from=builder /litert-lm-libs/*.so /litert-lm-libs/

# Ensure the system dynamic linker can find them
RUN echo "/litert-lm-libs" > /etc/ld.so.conf.d/litert.conf && ldconfig

# Copy the compiled binary and configuration
COPY --from=builder /usr/src/lite-llm/target/release/lite-llm /app/lite-llm
COPY --from=builder /usr/src/lite-llm/config.toml /app/config.toml

# We need to listen on 0.0.0.0 in Docker instead of 127.0.0.1
RUN sed -i 's/host = "127.0.0.1"/host = "0.0.0.0"/g' /app/config.toml

# Change ownership to the non-root user
RUN chown -R appuser:appgroup /app /litert-lm-libs

# Switch to the non-root user
USER 1000:1000

ENV RUST_LOG=info
EXPOSE 8080

CMD ["./lite-llm"]