#!/usr/bin/env python3
"""Seeded test-case generator for the Go/Rust differential test.

usage: gen_cases.py DICT.yaml OUTDIR [N_CANDS] [N_KEYS] [SEED]
writes OUTDIR/cands.txt (one composition per line) and OUTDIR/keys.txt (one key
sequence per line, hex virtual-key codes).
"""
import random
import sys

dict_path, outdir = sys.argv[1], sys.argv[2]
n_cands = int(sys.argv[3]) if len(sys.argv) > 3 else 3000
n_keys = int(sys.argv[4]) if len(sys.argv) > 4 else 400
seed = int(sys.argv[5]) if len(sys.argv) > 5 else 7
rng = random.Random(seed)

codes, in_table = set(), False
for line in open(dict_path, encoding="utf-8").read().split("\n"):
    line = line.rstrip("\r")
    if line.strip() == "...":
        in_table = True
        continue
    if not in_table or not line or line.startswith("#"):
        continue
    cols = line.split("\t")
    if len(cols) >= 2:
        codes.add(cols[1].strip())
codes = sorted(codes)
syllables = [c for c in codes if len(c) <= 6]
letters = "abcdefghijklmnopqrstuvwxyz"

cands = list(rng.sample(codes, min(len(codes), n_cands // 6)))                  # exact entries
for c in rng.sample(codes, min(len(codes), n_cands // 6)):                      # prefixes (fallback stage)
    cands += [c[:k] for k in range(1, len(c))]
for _ in range(n_cands // 3):                                                   # concatenations (segmentation)
    cands.append("".join(rng.choice(syllables) for _ in range(rng.choice([2, 2, 3, 3, 4, 5]))))
for _ in range(n_cands // 8):                                                   # concatenation + dangling tail
    cands.append("".join(rng.choice(syllables) for _ in range(rng.choice([1, 2, 3]))) + rng.choice(letters))
for _ in range(n_cands // 8):                                                   # garbage
    cands.append("".join(rng.choice(letters) for _ in range(rng.randint(1, 12))))
for c in rng.sample(codes, min(len(codes), n_cands // 12)):                     # case and separators
    cands += [c.upper(), c[: len(c) // 2] + "'" + c[len(c) // 2:], " " + c + " - "]
cands += ["", " ", "'", "-", "ni\u597d", "\u597d", "NI HAO", "xi'an", "n\u01d0", "a1b", "a\tb"]
rng.shuffle(cands)

# Key sequences: mostly letters so compositions get long enough to be interesting,
# with every other key the reducer knows, and keys it does not know.
VK = {"back": 0x08, "tab": 0x09, "enter": 0x0D, "esc": 0x1B, "space": 0x20, "left": 0x25, "up": 0x26,
      "right": 0x27, "down": 0x28, "del": 0x2E, "f1": 0x70}
others = list(VK.values())
keys = []
for _ in range(n_keys):
    seq = []
    word = "".join(rng.choice(syllables) for _ in range(rng.choice([1, 1, 2, 3])))
    for _ in range(rng.randint(0, 3)):
        seq += [ord(ch.upper()) for ch in word]
        for _ in range(rng.randint(0, 4)):
            r = rng.random()
            if r < 0.35:
                seq.append(rng.choice([0x25, 0x26, 0x27, 0x28]))
            elif r < 0.55:
                seq.append(ord(rng.choice("0123456789")))
            else:
                seq.append(rng.choice(others))
        word = "".join(rng.choice(syllables) for _ in range(rng.choice([1, 2])))
    keys.append(" ".join("%x" % vk for vk in seq))

open(outdir + "/cands.txt", "w", encoding="utf-8").write("\n".join(cands) + "\n")
open(outdir + "/keys.txt", "w", encoding="utf-8").write("\n".join(keys) + "\n")
print(f"{len(cands)} composition cases, {len(keys)} key sequences")
