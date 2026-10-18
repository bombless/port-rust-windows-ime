# Third-party notices

## `data/pinyin_simp.dict.yaml`

Embedded into the DLL at build time. It is the dictionary file of
[`port-golang-windows-ime`](https://github.com/bombless/port-golang-windows-ime)
(`tip/pinyin_simp.dict.yaml`), copied **byte for byte**; `tools/differential/run.sh`
fails if the two ever differ.

The file's own header states its origin:

> Rime dictionary … A minimal Pinyin dictionary for simplified Chinese script.
> Derived from android open source project:
> http://android.git.kernel.org/?p=platform/packages/inputmethods/PinyinIME.git

That header is kept intact. The Go repository ships no licence or NOTICE file for the
dictionary, so this port cannot say more than the header does. Android Open Source
Project code is generally distributed under the Apache License 2.0, but that has not
been confirmed for this file. **The maintainer should confirm the dictionary's licence
and add the matching notice before this crate or its DLL is distributed.**

## Crates

The DLL links the Microsoft [`windows`](https://crates.io/crates/windows) family of
crates (MIT OR Apache-2.0) and nothing else; `Cargo.lock` pins the exact versions.
