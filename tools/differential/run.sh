#!/usr/bin/env bash
# Differential test: the original Go engine against this crate's engine.
#
# usage: tools/differential/run.sh [N_CANDS] [N_KEYS] [SEED]
#
# Clones port-golang-windows-ime at a pinned commit, builds its strategy.go into a
# small harness, feeds both implementations the same generated inputs and requires
# identical output. Needs git, go, python3 and cargo.
#
# One line of the reference is patched: the prefix-fallback stage there iterates a
# Go map and uses an unstable sort, so ties come out in random order. The patch
# makes it alphabetical, which is what the Rust engine documents, so that every
# case can be required to match exactly.
set -euo pipefail

REF_URL="https://github.com/bombless/port-golang-windows-ime.git"
REF_COMMIT="5048d274e9f0624b0b8627be38be3c5a2a233909"
N_CANDS="${1:-3000}"
N_KEYS="${2:-400}"
SEED="${3:-7}"

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
work="${DIFF_WORKDIR:-$(mktemp -d)}"
mkdir -p "$work"
echo "work dir: $work"

if [ ! -d "$work/ref/.git" ]; then
  git clone --quiet "$REF_URL" "$work/ref"
fi
git -C "$work/ref" checkout --quiet "$REF_COMMIT"

# the reference and this crate must ship the same dictionary
cmp "$work/ref/tip/pinyin_simp.dict.yaml" "$root/data/pinyin_simp.dict.yaml"

rm -rf "$work/harness" && mkdir -p "$work/harness"
cp "$here"/go_harness/*.go "$here"/go_harness/go.mod "$work/harness/"
cp "$work/ref/tip/strategy.go" "$work/ref/tip/pinyin_simp.dict.yaml" "$work/harness/"
# pin the one nondeterministic tie order (see the header comment)
before="$(grep -c 'sort\.Slice(codes, func' "$work/harness/strategy.go")"
[ "$before" = "1" ] || { echo "reference changed: expected exactly one sort.Slice(codes, ...)"; exit 1; }
sed -i 's/^\tsort\.Slice(codes, func(i, j int) bool {$/\tsort.Strings(codes)\n\tsort.SliceStable(codes, func(i, j int) bool {/' "$work/harness/strategy.go"
grep -q 'sort.SliceStable(codes' "$work/harness/strategy.go" || { echo "tie-order patch did not apply"; exit 1; }
(cd "$work/harness" && go build -o godiff .)

python3 "$here/gen_cases.py" "$root/data/pinyin_simp.dict.yaml" "$work" "$N_CANDS" "$N_KEYS" "$SEED"

(cd "$root" && cargo build --quiet --release --examples)
ex="$root/target/release/examples"

echo "== candidate generation"
"$work/harness/godiff" cands < "$work/cands.txt" > "$work/cands.go.txt"
"$ex/candidates_dump" < "$work/cands.txt" > "$work/cands.rust.txt"
python3 "$here/compare.py" "$work/cands.txt" "$work/cands.go.txt" "$work/cands.rust.txt"

echo "== key reducer"
"$work/harness/godiff" keys < "$work/keys.txt" > "$work/keys.go.txt"
"$ex/reduce_dump" < "$work/keys.txt" > "$work/keys.rust.txt"
python3 "$here/compare.py" "$work/keys.txt" "$work/keys.go.txt" "$work/keys.rust.txt"
echo "OK: the Rust engine matches the Go original"
