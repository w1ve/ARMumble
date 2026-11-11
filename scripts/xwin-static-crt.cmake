# Wraps cargo-xwin's clang-cl toolchain and forces the static C runtime (/MT).
# Old CMake projects (libopus) leave policy CMP0091 at OLD, which hard-codes /MD
# into CMAKE_C_FLAGS_RELEASE and makes the exe import the UCRT DLLs.
# Used once to pre-build libopus into ~/RRMumble/opt/opus-win-mt (see
# build-windows-exe.sh):
#   cmake -G Ninja -S <audiopus_sys>/opus -B /tmp/opus-mt-build \
#     -DCMAKE_TOOLCHAIN_FILE=$HOME/RRMumble/opt/cmake/xwin-static-crt.cmake \
#     -DCMAKE_BUILD_TYPE=Release -DCMAKE_POLICY_VERSION_MINIMUM=3.5 \
#     -DCMAKE_POLICY_DEFAULT_CMP0091=NEW -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded \
#     -DOPUS_X86_MAY_HAVE_SSE4_1=OFF -DOPUS_X86_MAY_HAVE_AVX=OFF \
#     -DOPUS_BUILD_PROGRAMS=OFF -DOPUS_BUILD_TESTING=OFF -DCMAKE_INSTALL_PREFIX=$HOME/RRMumble/opt/opus-win-mt
#   cmake --build /tmp/opus-mt-build && cmake --install /tmp/opus-mt-build
include($ENV{HOME}/.cache/cargo-xwin/cmake/clang-cl/x86_64-pc-windows-msvc-toolchain.cmake)
set(CMAKE_POLICY_DEFAULT_CMP0091 NEW)
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded" CACHE STRING "" FORCE)