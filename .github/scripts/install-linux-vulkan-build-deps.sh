#!/usr/bin/env bash
set -euo pipefail

sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  glslc \
  libvulkan-dev \
  pkg-config \
  spirv-headers

glslc --version
pkg-config --exists vulkan
