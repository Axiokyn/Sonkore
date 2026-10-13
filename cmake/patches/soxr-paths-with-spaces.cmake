# Run in libsoxr's source dir by FetchContent (PATCH_COMMAND).
# Its BUILD_LSR_TESTS option tests `EXISTS ${PROJECT_SOURCE_DIR}/lsr-tests`
# inside a cmake_dependent_option() condition, which splits on spaces and
# fails when the build path has any. The tests are never built here.
file(READ CMakeLists.txt content)
string(REPLACE
    "cmake_dependent_option (BUILD_LSR_TESTS \"Build LSR tests.\" OFF\n  \"UNIX;NOT CMAKE_CROSSCOMPILING;EXISTS \${PROJECT_SOURCE_DIR}/lsr-tests;WITH_LSR_BINDINGS\" OFF)"
    "set (BUILD_LSR_TESTS OFF)"
    patched "${content}")
if(patched STREQUAL content AND NOT content MATCHES "set \\(BUILD_LSR_TESTS OFF\\)")
    message(FATAL_ERROR "soxr patch did not apply")
endif()
file(WRITE CMakeLists.txt "${patched}")
