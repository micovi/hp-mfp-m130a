# Read-only check of the printer plugged into this computer by USB: does it answer IPP and eSCL?
probe:
    python probe/probe.py

test:
    pytest -q

lint:
    ruff check .
    ruff format --check .
    ty check probe tests
