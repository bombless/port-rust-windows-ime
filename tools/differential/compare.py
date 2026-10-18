#!/usr/bin/env python3
"""Compares Go and Rust outputs line by line (as parsed JSON). Exit 1 on any difference."""
import json
import sys

cases, go, rust = (open(p, encoding="utf-8").read().split("\n")[:-1] for p in sys.argv[1:4])
if not (len(cases) == len(go) == len(rust)):
    sys.exit(f"line counts differ: cases={len(cases)} go={len(go)} rust={len(rust)}")
bad = [i for i in range(len(cases)) if json.loads(go[i]) != json.loads(rust[i])]
print(f"{len(cases)} cases, {len(cases) - len(bad)} identical, {len(bad)} different")
for i in bad[:10]:
    print("  case:", repr(cases[i]), "\n    go:  ", go[i][:240], "\n    rust:", rust[i][:240])
sys.exit(1 if bad else 0)
