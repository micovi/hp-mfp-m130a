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

# Function length, complexity and parameter limits for the firmware C code.
fw-lint:
    lizard --warnings_only -C 8 -a 5 -L 100 firmware/src firmware/tests
