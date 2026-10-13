# Firmware for the Pico 2 W. PICO_SDK_PATH and PICO_TINYUSB_PATH come from the Nix dev shell.
cmake_minimum_required(VERSION 3.20)

set(PICO_BOARD pico2_w)
set(PICO_PLATFORM rp2350-arm-s)
include($ENV{PICO_SDK_PATH}/pico_sdk_init.cmake)

project(m130a_bridge C CXX ASM)
set(CMAKE_C_STANDARD 11)
pico_sdk_init()

set(FW_SOURCES
    src/main.c
    src/printer_link.c
    src/http_framer.c
    src/http_request.c
    src/usb_desc.c
)
add_executable(m130a_bridge ${FW_SOURCES})
target_include_directories(m130a_bridge PRIVATE src)
target_link_libraries(m130a_bridge PRIVATE pico_stdlib tinyusb_host)
set_source_files_properties(${FW_SOURCES} PROPERTIES COMPILE_OPTIONS "-Wall;-Wextra;-Werror")

# Logs go to UART0 on GP0/GP1 (the Debug Probe); the USB port is the printer's host port.
pico_enable_stdio_uart(m130a_bridge 1)
pico_enable_stdio_usb(m130a_bridge 0)
pico_add_extra_outputs(m130a_bridge)
