cmake_minimum_required(VERSION 3.20)

project(tav-cbor-builder-poc CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

get_filename_component(TAV_REPO_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/../../.." ABSOLUTE)
set(TAV_LIB "${TAV_REPO_ROOT}/target/debug/libtee_attestation_verification_ffi.a")

add_custom_target(tav_cbor_abi ALL
    COMMAND cargo build --manifest-path "${TAV_REPO_ROOT}/ffi/Cargo.toml" --features crypto_openssl
    WORKING_DIRECTORY "${TAV_REPO_ROOT}"
    BYPRODUCTS "${TAV_LIB}"
    VERBATIM)

add_executable(tav-cbor-builder-poc main.cpp cbor_builder.cpp)
add_dependencies(tav-cbor-builder-poc tav_cbor_abi)
target_include_directories(tav-cbor-builder-poc PRIVATE
    "${TAV_REPO_ROOT}/ffi/tests/c-consumer/vendor"
    "${TAV_REPO_ROOT}/ffi/include")
target_link_libraries(tav-cbor-builder-poc PRIVATE "${TAV_LIB}" ssl crypto pthread dl m)
target_compile_options(tav-cbor-builder-poc PRIVATE -Wall -Wextra)

enable_testing()
add_test(NAME cbor-builder-poc COMMAND tav-cbor-builder-poc)
