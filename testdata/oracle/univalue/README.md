# UniValue vectors (MG-04b D7)

`univalue_vectors.json` records how Dash Core v24.0.0-rc.3's own UniValue
reads and writes each input in `gen_vectors.py`'s `INPUTS`: `read`'s verdict,
`write()`, `write(2)`, `write(2)` of dash-qt's `WithoutFingerprint`, dash-qt's
`EnvelopeFingerprint` (the shared-masternode message code), and
`getInt<int64_t|uint64_t|int|uint32_t>` of each array element. Cases nested
deeper than 20 keep only `read`'s verdict (`read_only`): their pretty rendering
is quadratic in the depth. The inputs include object keys that need escapes
(`"`, `\`, `\u0001`, `\u0000`, non-ASCII raw and as `\u` escapes with a
surrogate pair), duplicate keys (also spelled with different escapes),
`fingerprint` written with an escape or nested, and malformed keys (a raw tab
or newline, a bad escape).

The document is `{"source", "count", "cases"}`; `count` is the number of
cases. A case holds `input` and `ok`; `read_only` (deep cases only) replaces
everything else; otherwise an ok case also holds `write0`, `write2` and
`without_fp`, plus `fingerprint` for an object and `ints` for an array.

`rust/crates/dw-protx/tests/oracle_univalue.rs` checks
`dw-protx::shared::json` and `shared::envelope` against every case, and the
structure above: the exact case count, exactly these fields per case, and the
exact set of `read_only` cases. A second test corrupts the vectors in memory
(a byte of a result, `fingerprint` or `ints` removed from every case, every
case `read_only`, truncation) and requires the check to fail. Adding or
removing an input means updating `EXPECTED_CASES` (and `EXPECTED_READ_ONLY`)
there. Tests and CI read only the JSON; nothing compiles C++.

| File | What it is |
|---|---|
| `univalue_oracle.cpp` | The driver: reads hex cases on stdin, prints the JSON. `WithoutFingerprint` and `EnvelopeFingerprint` are copied from rc.3 `src/qt/mnsharesession.cpp:46-57,225-232` (Qt strings replaced by `std::string`). |
| `gen_vectors.py` | The inputs, the build and the run. |
| `univalue_vectors.json` | The output, committed. |

## Regenerating

Needs a C++20 compiler (`CXX`, default `c++`) and a Dash Core git checkout
with the tag. The script reads Core only through `git archive <tag>`
(`src/univalue`, `src/crypto/sha256.*`, `src/crypto/common.h`, `src/compat`),
builds in a temporary directory, and overwrites the JSON:

```sh
python3 -I testdata/oracle/univalue/gen_vectors.py --dash-repo <dash checkout> [--tag v24.0.0-rc.3]
cd rust && cargo test -p dw-protx --test oracle_univalue
```

To add a case, append it to `INPUTS` (a `str` for UTF-8 text, `bytes` for
anything else) and regenerate. A change in a later Core tag shows up as a diff
of the JSON.
