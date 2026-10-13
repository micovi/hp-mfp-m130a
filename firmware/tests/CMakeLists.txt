# Host-side unit tests for the firmware's hardware-independent modules.
cmake_minimum_required(VERSION 3.20)
project(m130a_bridge_tests C)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)
set(SRC ${CMAKE_CURRENT_LIST_DIR}/../src)
# UBSan only: ASan's runtime hangs at start-up with nixpkgs' clang on macOS.
add_compile_options(-Wall -Wextra -Werror -fsanitize=undefined -fno-sanitize-recover=undefined)
add_link_options(-fsanitize=undefined)

enable_testing()
foreach(module http_framer usb_desc http_request)
    add_executable(test_${module} test_${module}.c ${SRC}/${module}.c)
    target_include_directories(test_${module} PRIVATE ${SRC})
    target_compile_definitions(test_${module} PRIVATE
        FIXTURE_DIR="${CMAKE_CURRENT_LIST_DIR}/fixtures")
    add_test(NAME ${module} COMMAND test_${module})
    set_tests_properties(${module} PROPERTIES TIMEOUT 10)
endforeach()
