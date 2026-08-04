# server module 経路（`transform_server_module`）の既知の穴

`.svelte.js` / `.svelte.ts` を `compileModule` でコンパイルすると、SSR 出力は
`3_transform/server/mod.rs:74 transform_server_module` が作る。これはコンポーネントの
SSR 経路（`server/ast/`、pure-AST + `rsvelte_esrap`）とは**別物の純テキスト経路**で、
`parts: Vec<String>` を組み立てて `parts.join("\n")` するだけ。`server/ast/` には一度も入らない。

このドキュメントは、その経路を AST 化する調査の過程で見つかった
**「テストが検出しないので、記録が無いと存在しないことになる」問題**を残すためのもの。
AST 化そのものは HOLD（下記「なぜ HOLD か」）。

---

## 1. テストが構造的に検出できない盲点

### 1.1 `$state<T>(0)` — AST 化すると壊れるが corpus は永久に緑

今日の経路は TS ジェネリクスを剥がせている:

`server/mod.rs:119` → `client::transform_module_source_for_module`
（`client/mod.rs:375`）→ `transform_module_script_runes`（`client/mod.rs:3747`）
→ `client/mod.rs:3768` `strip_rune_generics_ast::strip_rune_generic_params_ast(&result, is_ts)`。
`is_ts` は `analysis.filename` の拡張子から決まり（`client/mod.rs:3766`）、
`analysis.filename` は `options.filename`（`compiler/mod.rs:924`）なので、
実ファイルが `.svelte.ts` なら真になる。

**`server/ast/` は `strip_rune_generic_params_ast` を一度も呼ばない**
（production の呼び出し元は `client/mod.rs:3768` のみ。他のヒットは同ファイルの
`#[cfg(test)]`）。したがって module 経路を `server/ast/` に載せ替えると、
`let x = $state<number>(0)` は「`$state` と `number` の比較演算」として解釈され、
誤って lowering される。

**そして corpus はこれを永久に検出しない。**
`scripts/compat-corpus/compile.mjs:75-81 prepareSource` が `.svelte.ts` を esbuild で
TS-strip してから**両方のコンパイラに渡す**ため、`$state<T>(...)` の `<T>` は
Svelte コンパイラに到達する前に消えている。つまり corpus の `.svelte.ts` 入力には
ジェネリクスが 1 つも含まれない。これは corpus 側のバグではなく意図した設計
（実運用では Vite がバンドラ側で strip する）だが、結果としてこの回帰は
**corpus・ratchet のどちらからも見えない**。

AST 化する場合、`strip_rune_generic_params_ast` 相当を server 経路にも通すことと、
corpus では検出できないので**専用のユニットテストを先に置くこと**が必須。

### 1.2 パース失敗が空出力になる（`server/ast/script.rs:378-381`）

```rust
let ret = oxc_parser::Parser::new(&alloc, owned, oxc_span::SourceType::mjs()).parse();
if !ret.diagnostics.is_empty() {
    return Vec::new();
}
```

oxc の diagnostic が 1 件でも出ると、**本体を黙って捨てて空を返す**。
エラーにも警告にもならない。加えてここは `SourceType::mjs()` で再パースしており、
`compile_module` 本体が使ったパーサ設定とは別物。

module 経路をここに載せると、`compile_module` 側が受理したソースでも
このガードに引っかかった瞬間に「import 行だけの空モジュール」が出る。
今のテキスト経路は本体を逐語で通すのでこの失敗モードが無い。
AST 化の前提として、この `Vec::new()` は診断付きの失敗に変える必要がある。

---

## 2. AST 化を待たずに単独で直せそうな実バグ 2 件

### 2.1 `server/mod.rs:115` — 生の `String::replace` が字句ガードを通っていない

```rust
let source_without_effects = source_without_effects.replace("$effect.tracking()", "false");
```

同じファイルの `code_match_positions`（`server/mod.rs:188`）は、まさにこの事故を
防ぐために存在する。その doc コメント（L184-187）が
「`const a = "$effect.root()"` のような文字列/コメント内の rune 呼び出し形の部分文字列を
書き換えてリテラルを壊す（issue #447, H-029）」と明記しているとおり、
L115 は**同じクラスのバグを踏んだままの唯一の残り**。
`const s = "$effect.tracking()"` は `const s = "false"` に化ける。

**単独修正の見立て: 小。** `code_match_positions(src, b"$effect.tracking()")` で位置を採り、
右→左で `"false"` に splice するだけ。関数は同一ファイル内の private `fn` なので
可視性変更すら不要。retro fixture は 未確認（既存の corpus/fixture にこの形が
含まれるかは調べていない）。

### 2.2 `transform_script.rs:4870 transform_class_fields_server` — brace カウンタが文字列/コメントを飛ばさない

2 箇所とも字句非対応:

- `transform_script.rs:4881` `memmem::find(script_bytes, b"class ")` — ソース中
  **最初の** `class ` を拾う。コメント内や文字列内の `class ` でもヒットする。
- `transform_script.rs:4896-4908` — クラス本体の終端を求める `{` / `}` 深さ計算が
  生の `char_indices()` で、文字列・テンプレート・コメントを一切スキップしない。
  本体中に `"}"` を含む文字列が 1 つあれば class body の境界がずれる。

さらに L4910 以降のメンバ走査は行ベースなので、上 2 つを直しても
「文字列/コメントに影響されない」保証にはならない。

**単独修正の見立て: 中、かつ部分的。** L4881 / L4896-4908 の入口 2 箇所だけなら
`helpers.rs:316 skip_string_literal`（`pub(crate)`）を使って字句ガードできる。
ただし `code_match_positions` は `server/mod.rs` の private `fn` なので、
そのまま流用するには `pub(crate)` への可視性変更が要る。
行ベースのメンバ走査まで含めた完全な解決は AST 化と同じ工数になるので、
**入口だけの部分修正**として切るのが現実的。

---

## 3. なぜ module AST 化を HOLD したか

`server/ast/` のコメント持ち回りは、**トップレベル文と文の間の source gap しか**
登録しない（`server/ast/script.rs:309 register_leading_comments`、呼び出しは L388 / L2617）。
加えて `server/ast/mod.rs:700-708 reparse_statement` は `ret.program.comments` を**捨てる**ため、
文の内側のコメントは復元不能。

既存の生成物だけで測った現状（コンパイル未実行、`compatibility/expected` と `_actual` の読み取りのみ）:

| | 桁 0（トップレベル） | インデント済み（文の内側） |
|---|---|---|
| rsvelte `_actual`（module 出力） | 11 個 / 7 エントリ | **624 個 / 約 100 エントリ** |
| 公式 `expected` | 2 個 | **422 個** |

つまり今のまま AST 化すると、**最大 624 個のコメントが落ち、うち 422 個は公式も保持している位置**。
代わりに消えるのは `// prettier-ignore` の extra 42 件だけで、差し引きでは公式から遠ざかる。

そして**この劣化を検出する仕組みが一つも無い**: corpus gate は
`CommentPolicy::Ignore` + oxfmt 正規化 + AST 等価で比較し、
`known-failures.server.json` は空配列。ratchet は緑のまま、
ユーザに見える出力品質だけが下がる。

前提条件は `reparse_statement` が捨てている `ret.program.comments` を側テーブルに拾う機構。
これが入るまで module AST 化は着手しない。

---

## 4. 配線側の調査結果（前提条件が揃ったらそのまま使える）

- 載せ替え先は `server/ast/script.rs:3608 transform_module`。今日の唯一の呼び出し元は
  `server/ast/mod.rs:1014`（コンポーネント経路）で、`<script module>` を持つ
  全コンポーネントが既にこの関数を通っている。
- `ServerTransformState::new`（`server/ast/mod.rs:265-313`）は `analysis` / `options` /
  `source` / `arena` / `allocator` だけを要求し、**fragment を必要としない**。
- `compile_module`（`compiler/mod.rs:834-1015`）は既に消費可能な形の `Root` を組み立てている
  （L902 `instance: None`、L903-913 synthetic module `Script`、L928 `runes: Some(true)`）。
  span は `source` にオフセット 0 から直接インデックスする。
- `instance_scope_index` は `2_analyze/scope_builder.rs:164-169` が
  `ast.instance` がある場合にしか代入しないので、`instance: None` では 0（module scope）のまま。
  `ServerTransformState::new` の初期化はこの前提で既に正しい。
- 削除できるのは `server/mod.rs` L74-894（821 行、`transform_server` L42-73 は
  L178 以降のヘルパを一切使っていない）と、唯一の呼び出し元が module 経路である
  `transform_script::strip_snapshot_declarator_init_module` /
  `client::transform_module_source_for_module` / `client::extract_imports_str` の 3 関数のみ。
  `transform_script.rs`（7607 行）/ `transform_legacy.rs`（2257 行）/ `transform_store.rs`（1128 行）は
  **消えない** — `server/ast/visitors/declaration_tag.rs:346` が
  `transform_script_content_with_imports_and_derived` を呼んでおり、AST コンポーネント経路から生きている。
- `sourcemaps_gate` の `EXPECTED_IDENTICAL_OUTPUTS = 56`（`tests/sourcemaps_gate.rs:203`）は
  この作業の影響を受けない。gate は `<sample>/input.svelte` しか読まず（L591）
  `compile()` しか呼ばない（L613）。ファイル中に `compile_module` は 1 ヒットも無い。
