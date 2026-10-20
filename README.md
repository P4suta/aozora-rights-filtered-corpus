# Aozora rights-filtered corpus

A pinned collection of 4,417 Aozora Bunko editions for reproducible text-processing tests.
The snapshot uses reference date `2026-08-02` and publication cutoff `1930`.

## Verify

Install the Rust toolchain in `rust-toolchain.toml` and typos-cli `1.51.1`, then run:

```sh
cargo xtask check
```

This checks formatting, spelling, compilation, Clippy, tests, and the committed corpus.
Snapshot updates are prepared locally with `cargo xtask refresh` and reviewed before adoption.
Refresh requires the upstream mirror configured in [corpus.toml](corpus.toml).

## Data and rights

The [manifest](corpus/manifest.json) includes only editions whose work and contributors have copyright flag `なし`, whose publication years satisfy the cutoff, and whose official source URLs, lossless decoding, and hashes can be verified.
Rejected editions appear in [quarantine](corpus/quarantine.json).
Work-specific allowlists are unsupported.

Sources are the [official extended CSV](https://www.aozora.gr.jp/index_pages/list_person_all_extended_utf8.zip) and [Aozora Bunko site mirror](https://github.com/aozorabunko/aozorabunko).
This unofficial filter targets display in Japan and hosting in the United States; it does not guarantee rights elsewhere.
Review the [Aozora Bunko file-handling policy](https://www.aozora.gr.jp/guide/kijyunn.html) and each edition's rights evidence before using its text.

Generator and maintenance code is [MIT-licensed](LICENSE).
That license does not relicense source texts or bibliographic data.
