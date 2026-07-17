# llama-cpp-sys 0.1.151 sets GGML_CPU_ARM_ARCH=armv8-a for Linux ARM64 while
# dynamic backends also request every upstream CPU variant. That combination is
# rejected, and its newest SME variants exceed the pinned GCC 13 toolchain.
# Keep the portable ARM baseline as a runtime-loaded CPU backend beside Vulkan.
if(GGML_CPU_ALL_VARIANTS AND GGML_CPU_ARM_ARCH)
  set(GGML_CPU_ALL_VARIANTS OFF CACHE BOOL "ggml: build all variants of the CPU backend (requires GGML_BACKEND_DL)" FORCE)
endif()
