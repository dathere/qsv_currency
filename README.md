# Rust Currency Library

A very small library, providing a way to represent currencies in Rust.

This is a fork of https://github.com/Tahler/currency-rs, pulling in pending PRs
for currency strings (e.g. "USD, EUR, etc." - not just single character symbols - "$, €, etc.")
and serde support.

It also upgrades the num dependency from 0.1.32 to 0.4.0 and adds a `is_iso_currency` function to check if a currency is an ISO currency.

This fork was primarily created for the [qsv](https://github.com/jqnatividad/qsv) CSV data-wrangling toolkit.
