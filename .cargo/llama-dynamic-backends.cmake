# llama-cpp-sys 0.1.151 sets GGML_CPU_ARM_ARCH=armv8-a for Linux ARM64 and
# GGML_CPU_ALL_VARIANTS=ON for dynamic backends. The latter already selects
# portable ARM variants, and upstream CMake rejects both selectors together.
if(GGML_CPU_ALL_VARIANTS AND GGML_CPU_ARM_ARCH)
  set(GGML_CPU_ARM_ARCH "" CACHE STRING "ggml: CPU architecture for ARM" FORCE)
endif()
