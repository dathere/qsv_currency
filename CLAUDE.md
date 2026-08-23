# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this crate is

`qsv_currency` is a fork of [Tahler/currency-rs](https://github.com/Tahler/currency-rs), created for
the [qsv](https://github.com/jqnatividad/qsv) CSV toolkit. The fork exists to support **multi-character
currency strings** ("USD", "EUR") rather than only single-char symbols ("$", "€"), plus serde support,
`num` 0.4, and `is_iso_currency()`.

Note the naming mismatch: the crate is `qsv_currency`, but `repository` in Cargo.toml points at
`dathere/currency-rs`.

## Commands

```sh
cargo test                    # 16 unit tests + 10 doctests
cargo test test_from_str      # single test (all tests are inline in `mod tests`; no tests/ dir)
cargo test --doc              # doctests only — the public API is documented almost entirely by them
cargo clippy --all-targets
cargo fmt --check
```

`cargo clippy --all-targets` and `cargo fmt --check` are both currently clean — treat any warning as
something you introduced.

## Architecture

Everything lives in `src/lib.rs` (~1500 lines). There is no module structure to learn — the
non-obvious parts are the invariants below.

**Representation.** `Currency { symbol: String, coin: BigInt }`. `coin` is in 1/100 units, so
`Currency::from(1000, '$')` is `$10.00`. `DECIMAL_PLACES` is hardcoded to 2, so 0-decimal (JPY) and
3-decimal (KWD) currencies pass `is_iso_currency()` but are still stored and formatted with 2 decimals.

**Arithmetic panics on symbol mismatch.** Add/Sub/Div between two `Currency` values with different
symbols is a `panic!`, not an `Err`. This is the single biggest gotcha in the API.

**Operators are macro-generated.** To add or change one, edit the macro and its invocation list
rather than writing impls by hand:

| Macro | Covers |
|---|---|
| `impl_all_trait_combinations_for_currency!` | Add/Sub between two `Currency` (all 4 owned/borrowed combos). `Mul` is deliberately commented out. |
| `impl_all_trait_combinations_for_currency_into_bigint!` | Mul/Div by `BigUint`, `u8`..`usize`, `i8`..`isize` |
| `impl_all_trait_combinations_for_currency_conv_bigint!` | Mul/Div by `f32`/`f64` via `from_f32`/`from_f64` |

`Currency / Currency` (4 impls) and `Neg` are hand-written, not macro-generated.

**`from_str` is a hand-rolled char scanner**, not a regex or grammar. It splits leading non-digit
chars into the symbol, then decides decimal placement from a heuristic: the *last* delimiter seen
(`.` or `,`) plus the length of the trailing digit streak.

- streak == 3 → treated as a thousands separator, no decimals (so `"£1.000"` parses as 1000.00)
- streak < 2 → pad with zeros
- streak > 2 → rounded by going through `f64`

It also accepts accounting-style negatives: `(1.00)` and `-1.00`.

**Formatting.** `{}` (`Display`) produces comma grouping with a `.` decimal; `{:e}` (`LowerExp`)
produces the European form by running `Display` output through a three-step `,`↔`.` placeholder swap.

**`is_iso_currency`** checks `iso_currency::Currency::from_code()` and falls back to a
`OnceLock<HashSet<String>>` of every ISO symbol built from `iso_currency::Currency::iter()`. The
`iterator` feature on `iso_currency` is load-bearing — removing it breaks the build. Code lookup is
case-sensitive ("USd" is not ISO), and crypto symbols (Ð, Ξ) are correctly rejected.

**Serde is hand-written, not derived.** `Serialize` emits `to_string()`; `Deserialize` runs
`from_str`. Because `Deserialize` goes through `String::deserialize`, the wire form must be a JSON
*string* (`{"amount": "-$12,000.99"}`) — a bare JSON number fails. `serde_json` and `serde_derive`
are `#[cfg(test)]`-only dev-dependencies.

**Ordering** is a `#[derive(PartialOrd)]` on `(symbol, coin)`, so cross-symbol comparison sorts
lexicographically by symbol first — this is derived behavior, not a designed contract, and the tests
only ever compare same-symbol values.

## Known-shaky areas

The file opens with `// TODO issues with precision. truncation all over the place`. Multiplication,
division, and over-precise parses all truncate or round through `f64`. Treat precision as a known
open problem rather than a bug to fix incidentally.
