# Aozora rights-filtered corpus

This repository pins Aozora Bunko editions that pass a conservative, mechanical rights policy.

## Policy

An edition is identified by its work ID and text ZIP URL. It is included in
`corpus/manifest.json` only when:

- the work and every contributor have the copyright flag `なし`;
- the first-publication field is present, every year can be parsed, and every year is at or before
  `reference year - 96`;
- the work card and text ZIP use official Aozora Bunko HTTPS URLs;
- the pinned ZIP can be extracted safely and converted from its declared encoding to UTF-8 without
  loss; and
- the CSV, ZIP, and UTF-8 text SHA-256 values can be reproduced.

Rejected editions are recorded with reasons in `corpus/quarantine.json`. Work-specific allowlists
are not supported.

## Commands

Install Rust 1.97.1 and typos-cli 1.48.0.

| Command | Purpose |
| --- | --- |
| `cargo xtask doctor` | Check host tools and policy configuration. |
| `cargo xtask spellcheck` | Check spelling with typos-cli. |
| `cargo xtask check` | Run formatting, spelling, compilation, Clippy, tests, and corpus verification. |
| `cargo xtask scan` | Scan the pinned metadata input. |
| `cargo xtask snapshot --jobs 20` | Regenerate the corpus from pinned remote inputs. |
| `cargo xtask snapshot --upstream-root ../aozorabunko --jobs 20` | Regenerate from a matching local mirror checkout. |
| `cargo xtask verify` | Verify policy, ordering, identities, files, and hashes. |
| `cargo xtask refresh --reference-date YYYY-MM-DD --jobs 20` | Prepare a new snapshot and update report. |

## Data and license

Inputs are pinned versions of the
[Aozora Bunko extended CSV](https://www.aozora.gr.jp/index_pages/list_person_all_extended_utf8.zip)
and [Aozora Bunko site mirror](https://github.com/aozorabunko/aozorabunko).

This is not an official Aozora Bunko service. The policy targets display in Japan and hosting in
the United States; it does not guarantee rights in every jurisdiction. Review the
[Aozora Bunko file-handling policy](https://www.aozora.gr.jp/guide/kijyunn.html) and each entry's
source and rights evidence before using a text.

Generator and maintenance code is MIT-licensed. That license does not relicense Aozora Bunko texts
or bibliographic data.
