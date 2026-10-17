# Third-party dictionary notices

## Rime Luna Pinyin dictionary

- Source: https://github.com/rime/rime-luna-pinyin
- Vendored file: `data/luna_pinyin.dict.yaml`
- Upstream version: 2024.02.10 (as identified in the dictionary header)
- License: GNU Lesser General Public License v3.0 (LGPL-3.0); see `data/RIME-LICENSE`.
- The dictionary includes upstream attribution and source comments. It is used as a local embedded data file; no network access is required at runtime.

The engine keeps a small curated starter list first for predictable common-word ordering, then adds matching entries from the Rime dictionary. This initial integration normalizes pinyin by removing spaces and is not yet a complete implementation of Rime's schema, weighting, OpenCC conversion, user dictionary, or phrase language model.
