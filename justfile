# Read-only check of the printer plugged into this computer by USB: does it answer IPP and eSCL?
probe:
    python probe/probe.py

test:
    pytest -q

lint:
    ruff check .
    ruff format --check .
    ty check probe tests

# Host unit tests for the firmware's hardware-independent modules.
fw-test:
    cmake -S firmware/tests -B build/fw-tests -G Ninja
    cmake --build build/fw-tests
    ctest --test-dir build/fw-tests --output-on-failure

# Firmware C limits: function length, complexity, parameters (lizard) and 100-char lines (grep).
fw-lint:
    lizard --warnings_only -C 8 -a 5 -L 100 firmware/src firmware/tests
    grep -nE '^.{101,}' firmware/src/*.[ch] firmware/tests/*.[ch]; test $? -eq 1

# Cross-compile the firmware: build/fw/m130a_bridge.{elf,uf2}.
fw-build:
    cmake -S firmware -B build/fw -G Ninja
    cmake --build build/fw
    arm-none-eabi-size build/fw/m130a_bridge.elf

# Flash through the Debug Probe's SWD port and restart the Pico.
flash: fw-build
    probe-rs download --chip RP235x build/fw/m130a_bridge.elf
    probe-rs reset --chip RP235x

# The Pico's UART0 log, through the Debug Probe. Quit with ctrl-t q.
logs:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob
    ports=(/dev/cu.usbmodem*)
    if [ ${#ports[@]} -eq 0 ]; then
        echo "No /dev/cu.usbmodem* serial port: connect the Debug Probe's USB cable" >&2
        exit 1
    fi
    tio --baudrate 115200 "${ports[0]}"
