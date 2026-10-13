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
