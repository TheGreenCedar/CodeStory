ARG GLSLC_IMAGE=ubuntu:24.04
ARG BUILD_IMAGE=rust:1.95.0-bullseye

FROM ${GLSLC_IMAGE} AS glslc

ARG DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
      glslc=2023.8-1build1 \
      libvulkan-dev=1.3.275.0-1build1 \
      spirv-headers=1.6.1+1.4.309.0-1~ubuntu0.24.04.2 \
    && install -d /opt/glslc-root/usr/bin \
    && cp /usr/bin/glslc /opt/glslc-root/usr/bin/glslc \
    && ldd /usr/bin/glslc \
      | awk '/=> \// { print $3 } /^\// { print $1 }' \
      | xargs -r cp --dereference --parents --target-directory=/opt/glslc-root \
    && cp --dereference --parents /lib64/ld-linux-x86-64.so.2 /opt/glslc-root \
    && rm -rf /var/lib/apt/lists/*

FROM ${BUILD_IMAGE}

ARG DEBIAN_FRONTEND=noninteractive

# Bullseye preserves the glibc 2.31 floor but has no usable glslc package.
# Keep its linker/runtime and run the pinned shader compiler under its own loader.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
      clang-13=1:13.0.1-6~deb11u1 \
      libclang-13-dev=1:13.0.1-6~deb11u1 \
      libvulkan-dev=1.2.162.0-1 \
      pkg-config=0.29.2-1 \
    && rm -rf /var/lib/apt/lists/*

ENV CC=clang-13 \
    CXX=clang++-13 \
    LIBCLANG_PATH=/usr/lib/llvm-13/lib

# llama.cpp compiles dispatch variants newer than Bullseye's GCC 10. Keep the
# Bullseye ABI floor while proving the pinned compiler accepts every required
# x86 feature flag before the release build starts.
RUN printf 'int main() { return 0; }\n' \
      | "${CXX}" -x c++ -std=c++17 \
          -mavxvnni -mavx512bf16 -mamx-tile -mamx-int8 \
          -c -o /tmp/codestory-compiler-probe.o - \
    && rm /tmp/codestory-compiler-probe.o

ARG CMAKE_VERSION=3.28.3
ARG CMAKE_SHA256=804d231460ab3c8b556a42d2660af4ac7a0e21c98a7f8ee3318a74b4a9a187a6
RUN archive="cmake-${CMAKE_VERSION}-linux-x86_64.tar.gz" \
    && curl -fsSLo "/tmp/${archive}" \
      "https://github.com/Kitware/CMake/releases/download/v${CMAKE_VERSION}/${archive}" \
    && printf '%s  %s\n' "${CMAKE_SHA256}" "/tmp/${archive}" | sha256sum -c - \
    && install -d /opt/cmake \
    && tar -xzf "/tmp/${archive}" --strip-components=1 -C /opt/cmake \
    && ln -s /opt/cmake/bin/cmake /usr/local/bin/cmake \
    && rm "/tmp/${archive}" \
    && cmake --version

COPY --from=glslc /opt/glslc-root /opt/glslc-root
COPY --from=glslc /usr/include/spirv /usr/include/spirv
COPY --from=glslc /usr/include/vulkan /usr/include/vulkan
COPY --from=glslc /usr/include/vk_video /usr/include/vk_video
COPY --from=glslc /usr/share/cmake/SPIRV-Headers /usr/share/cmake/SPIRV-Headers
COPY glslc /usr/local/bin/glslc

RUN chmod 0755 /usr/local/bin/glslc \
    && glslc --version \
    && pkg-config --exists vulkan
