---
'@rsvelte/compiler': patch
---

Stop decoding raw bytes as characters when classifying identifier boundaries in
the client store transforms. `u8 as char` is a Latin-1 decode, so a UTF-8
continuation byte became a character absent from the source and the boundary
tests silently inverted. All eight sites in `client/store_transforms.rs` are
fixed — six now take the real character, and the `(`/`)` depth scan compares
bytes so nothing is decoded at all.

Two user-visible effects, plus one corrected predicate. A store name that is
only a suffix of a longer identifier was rewritten as a standalone store read,
so `$count(1) + 名$count(1)` emitted `名$count()(1)` and called a local as a
getter. A non-ASCII character before a call's paren could halt the byte cursor
mid-character, so the `function` keyword lookback sliced on a non-char-boundary
and the compiler panicked with "byte index is not a char boundary". And
`is_function_parameter_in_statement` reported `名$state` as a `$state`
parameter, which would suppress rune transforms on that line; that path is
reached from the rune callers with no identifier pre-filter in front of it.
