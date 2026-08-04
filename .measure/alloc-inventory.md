# 分散型コスト 実弾候補インベントリ

入力: `.measure/alloc-callers-by-line.txt`（flowbite serial client, 1296 files,
2,573,618 allocations = 1986/file, stacks 1-in-128, `debug = "line-tables-only"`
による file:line 集計）。

## 0. シェアの換算について（重要）

このテーブルの 100% は **アロケーション回数**であって壁時計ではない。
team-lead から渡された「string alloc 5.42% / hash 5.75%」は samply の**時間**シェア。
両者を突き合わせると:

- string カテゴリ: alloc 23.44% ↔ 時間 5.42% → **alloc シェア 1pt ≈ 時間 0.231pt ≈ 0.52µs/file**
- hash/map カテゴリ: alloc 5.16% ↔ 時間 5.75% → 比が 1 を超える。hash の時間は
  アロケーションではなくハッシュ計算とプローブが主。**単純比例換算は不可**。

したがって下表の µs は **string 側のみ** 0.52µs/pt で換算し、hash 側は
「別途カウンタで測る」とだけ書く。ここを比例で埋めるのは推測になる。

## 1. 判定一覧（上位サイト、機械的列挙）

| サイト | alloc share | 判定 |
|---|---|---|
| `client/expression_utils.rs:2182` / `:2203` | 1.72 + 1.72 = **3.44** | **構造的に不要**。全文字位置で 5 文字 `String` を作って `"async"`/`"await"` と比較しているだけ。スライス比較で足りる。しかも :2182 側の分岐は本体がコメントのみの **dead code** |
| `2_analyze` 名前 String クラスタ（下記 §2） | **合計 ~4.5** | **構造的に不要**。識別子名が `String` で 4〜5 本の並列インデックスに深いコピーされている。`CompactString` で ≤24B はインライン化 |
| `ast/js.rs:230` `Expression::Typed(Box::new(TypedExpr::new(..)))` | 3.59 | **本質**（現表現では）。Expression 1 個 = Box 1 個。除去は #15 のアーキ側 |
| `to_oxc.rs:1344` `other => Some(vec![self.stmt(other)?])` | 2.52 | **構造的に不要**。1 要素 `Vec` を文ごとにヒープに作る。`SmallVec<[_;1]>` か enum 返しで消える |
| `2_analyze/scope.rs:444` `parse_json_field` | 1.24 + 0.61 + 1.01 = **2.86** | **構造的に不要だが N4 で no-go 済み**。`Binding.initial` は scope_builder が JSON 文字列を手組み → ここで再パース。往復そのものが無駄（§3） |
| esrap `printer.rs:2913/2802/715/708/2037/1912/827/1905` | 1.94+1.21+1.13+1.05+0.92+0.58+0.56+0.38 = **7.77** | **本質**。measure→emit 二パスのレイアウトアルゴリズムが要求する per-node scratch Vec / render クロージャ。T3 で置換 no-go、pool 移設も +2.4% 悪化で棄却済み |
| `ast/typed_expr.rs` Serialize クラスタ（:817-822, :750, :855-861, :958-979, :1431/1441, :2416） | **~7.3** | **本質**（JSON 表現を持つ限り）。除去 = JSON 表現の除去そのもの |
| `1_parse/read/expression.rs:6398` `self.insert(key.to_string(), v)` | 0.48 + 0.53 + 1.03 = **2.04** | **本質**。`serde_json::Map<String, Value>` がキー所有を要求。JSON 表現を残す限り不可避 |
| `ast/template.rs:209/:211` (`Text.raw`/`Text.data`) | 0.73 + 0.73 = **1.46** | **本質**。derive(Serialize) の生成コードが field 宣言行に帰属する = Text ノードの JSON 直列化。パース時の `Cow` は借用のまま |
| `3_transform/utils.rs:120` / `:133` | 0.30 + 0.32 = **0.62** | **構造的に不要**。`replace_{leading,trailing}_whitespace` は「置換不要」の共通パスでも `s.to_string()` を返す。`Cow<str>` にすれば no-op パスは 0 alloc |
| `2_analyze/mod.rs:4806` `out.insert(name.to_string())` | 0.59 + 0.11 = **0.70** | **構造的に不要**。`name` は `CompactString` だが `out: &mut FxHashSet<String>`（確認済み）。加えて `if !out.contains(..) { out.insert(..) }` で**同じキーを 2 回ハッシュ**している（`insert` の戻り値だけで足りる） |
| `client/visitors/shared/component.rs:286/:287` `"default".to_string()` | 0.50 + 0.42 = **0.92** | **構造的に不要**。子ごとに固定文字列を heap 化。`Cow::Borrowed("default")` か `&'static str` |
| `2_analyze/visitors/shared/component.rs:434/:448` `format!("Attribute{}", ..)` | 0.46 + 0.24 = **0.70** | **構造的に不要**。重複検出のためだけに前置詞付き String を作って `HashSet<String>` に入れている。`(u8, &str)` タプルキーで format も alloc も不要 |
| `to_oxc.rs:773` 一時 `Vec<Statement>` → `ArenaVec` | 0.73 | **構造的に不要**。`ArenaVec::with_capacity_in` に直接積めばヒープ往復が消える |
| `to_oxc.rs:617` `self.str(&format!("'{source}'"))` | 0.42 | **構造的に不要**。import source ごとにヒープ String を作ってアリーナへコピー。アリーナ上で直接組める |
| `client/transform_template/types.rs:10` `IndexMap<String, Option<String>>` | 0.19 + 0.33 = **0.52** | **構造的に不要**（要設計）。属性は数個。`Vec<(CompactString, Option<String>)>` で十分 |
| `client/transform_template/types.rs:23` `Vec<Text<'static>>` | 0.52 | **本質**。IR が parse source より長生きするため owned 化が必要 |
| `2_analyze/store_subscriptions.rs:750` `Vec<char>` | 0.29 + 0.50 = **0.79** | **構造的に不要**。`char` 展開は 4 バイト/文字。ただし 2 パスで共有する意図的なキャッシュなので、除去は走査側の書き換えとセット |
| `2_analyze/types.rs:1813` `.map(\|n\| format!("${n}")).collect()` | 0.35 | **構造的に不要**。直後に同じ文字列を `FxHashSet<&str>` に借用し直している。`strip_prefix('$')` 比較で Vec ごと消える |
| `client/formatting.rs:932/:936`, `client/mod.rs:3325`, `transform_template/template.rs:*` | 各 0.13〜0.39 | **本質 or テキスト後段パスのエピック側**。出力テキストの行分割・再結合は print 段の統合（既知の大エピック）でしか消えない |
| `client/types.rs:3011/:3022`（unique name 生成） | 0.62 + 0.61 = 1.23 | **本質**。生成名は集合が所有する必要がある。既に `with_capacity` + itoa まで詰めてある |
| `client/types.rs:2683` `scope_root.conflicts.clone()` | 0.59 | **構造的に不要**。transform ごとに conflicts 集合を丸ごとディープコピー。基底は immutable、追加分だけローカルに持てばよい（§2 と同一クラスタ） |
| `3_transform/utils.rs:695` | 0.50 | **軽微に不要**。`hoisted` は多くの場合空なのに常に容量 8 を確保。遅延化で消える |

## 2. 最大のテーマ: 識別子名 `String` の 4〜5 重コピー

同じ識別子名が、名前ごとに別々のヒープ確保として最低 4 回複製されている。

| 行 | 内容 | share |
|---|---|---|
| `scope.rs:229` | `declarations: FxHashMap<String, usize>` | 0.54 |
| `scope_builder.rs:253` | root スコープ統合で `name.clone()` | 0.54 |
| `scope_builder.rs:300` | `conflicts.insert(name.clone())` | 0.38 |
| `scope_builder.rs:305` | `conflicts.insert(binding.name.clone())` | 0.48 |
| `scope_builder.rs:574` | `Binding::with_declaration_kind(name.clone(), ..)` | 0.47 |
| `scope_builder.rs:599` | `bindings_by_name.entry(binding.name.clone())` | 0.57 + 0.10 |
| `client/types.rs:2683` | `scope_root.conflicts.clone()`（5 本目の複製） | 0.59 |
| `2_analyze/mod.rs:4806` | `out.insert(name.to_string())` | 0.59 + 0.11 |
| `scope.rs:265` / `scope.rs:96` / `scope_builder.rs:362` | 付随 | 0.38 |
| | **合計** | **~4.75** |

`compact_str` は既に `rsvelte_core` の直接依存（`Cargo.toml:47`、serde feature 付き）で、
`JsNode` の文字列フィールドは既に `CompactString`。**≤24 バイトはインライン**なので、
実用上ほぼ全ての識別子でこれらの clone がヒープ確保 0 になる。

注意: これはアロケーションを消すだけで、**ハッシュ計算は消えない**。
hash 5.75% への寄与は「ハッシュ関数の入れ替え」でも「容量ヒント」でもないが、
効果は string 側に限定して見積もるべき。

## 3. `Binding.initial` の JSON 文字列往復（参考・N4 no-go 済み）

`scope_builder.rs:1260-1278` が import 用の JSON を**文字列連結で手組み**し、
`scope.rs:444` の `parse_json_field` が `serde_json::from_str` で**再パース**する。
合計 2.86 pt。表現としては明確に無駄（serialize→string→parse の往復）だが、
team-lead 判断で N4（`Binding.initial` enum 化）は no-go 済みのため候補から外す。
enum 化ではなく `Option<Box<Value>>` を直接持つ案は未検討。

## 4. ランク表（不要と判定したサイトのみ）

換算は string 側 **1 alloc-pt ≈ 0.52µs/file ≈ 0.23% 壁時計**（§0）。
223.3µs/file、1% ≒ 2.233µs/file。

| # | 候補 | alloc pt | 見込み µs/file | 見込み % | 触るファイル | 意味論リスク |
|---|---|---|---|---|---|---|
| 1 | `expression_utils.rs` の `contains_direct_await_in_expression` をスライス比較化 + dead 分岐削除 | 3.44 | ~1.8 | ~0.8% | 1 | **ほぼ無**（同一バイト比較。dead 分岐は本体がコメントのみ） |
| 2 | 名前 String → `CompactString`（§2 クラスタ、4 段階） | 4.75 | ~2.5 | ~1.1% | 段階次第で 3〜15 | 低〜中。型変更は網羅的だがコンパイラが全箇所を検出 |
| 3 | `to_oxc.rs:1344` の 1 要素 `Vec` を `SmallVec<[_;1]>` 化 | 2.52 | ~1.3 | ~0.6% | 1 | 低（戻り値型のみ） |
| 4 | `component.rs:286/287` の `"default".to_string()` を借用化 | 0.92 | ~0.5 | ~0.2% | 1 | **ほぼ無** |
| 5 | `to_oxc.rs:773` 一時 Vec → アリーナ直積み | 0.73 | ~0.4 | ~0.17% | 1 | 低 |
| 6 | `2_analyze/.../component.rs:434/448` の `format!` キー → タプルキー | 0.70 | ~0.4 | ~0.16% | 1 | 低（重複判定の等価性を保つ必要あり） |
| 7 | `3_transform/utils.rs:120/133` を `Cow<str>` 化 | 0.62 | ~0.3 | ~0.14% | 1 + 呼び出し元 | 低 |
| 8 | `transform_template/types.rs:10` の `IndexMap` → `Vec<(..)>` | 0.52 | ~0.3 | ~0.12% | 1 + 参照側 | 中（順序保証と lookup 意味論） |
| 9 | `to_oxc.rs:617` の `format!` をアリーナ直組み | 0.42 | ~0.2 | ~0.10% | 1 | **ほぼ無** |
| 10 | `2_analyze/types.rs:1813` の `format!("${n}")` Vec 除去 | 0.35 | ~0.2 | ~0.08% | 1 | 低 |
| 11 | `3_transform/utils.rs:695` の `hoisted` 遅延確保 | 0.50 | ~0.3 | ~0.11% | 1 | **ほぼ無** |
| | **合計（#1〜#11）** | **15.5** | **~8.2** | **~3.7%** | | |
| 12 | `store_subscriptions.rs:750` の `Vec<char>` 除去 | 0.79 | ~0.4 | ~0.18% | 1（走査側書換とセット） | 中（インデックス意味論が byte 基準に変わる） |

## 5. 証拠の取り方（各候補の決定論的カウンタ設計）

壁時計 A/B では 0.1〜0.8% 帯は判定不能。各候補は「変更前ならやっていた仕事量 vs
実際にやった仕事量」を数える:

- #1: `contains_direct_await_in_expression` の入口で `chars.len()` を累積 →
  変更前の String 生成回数 = Σ(2 × 位置数)。変更後は 0。
- #2: `CompactString::is_heap_allocated()` を clone 地点で数え、
  「変更前なら必ず heap だった回数」vs「実際に heap になった回数」を出す。
  ≤24B 比率がそのまま削減率。
- #3/#5/#9: 到達回数カウンタ（= 除去できた Vec/String 数）。
- #4/#6/#7/#10/#11: 到達回数カウンタ。

## 6. 明示的に候補から外したもの

- ハッシュ関数の入れ替え / 容量ヒント（team-lead 指示により枯渇済み）
- esrap printer の scratch Vec（T3 no-go、pool 移設は +2.4% 悪化で棄却済み）
- `typed_expr.rs` Serialize クラスタと `read/expression.rs:6398`（JSON 表現そのもの）
- `ast/js.rs:230` の `Box<TypedExpr>`（#15 のアーキ側）
- テキスト後段パス（`formatting.rs`, `client/mod.rs:3325`）= print 統合エピック
- `Binding.initial` JSON 往復（N4 no-go 済み）

## 7. 未確認

- §0 の 0.52µs/pt は 2 つのプロファイル（alloc 集計と samply 時間集計）の
  カテゴリ合計を突き合わせた換算であり、サイト単位の時間実測ではない。
- ≤24 バイトに収まる識別子名の比率は未計測（#2 の削減率がこれに直結）。
- 表中の判定は「この表現を保ったまま消せるか」であり、消した後に別コストが
  増えないことは保証していない（map-clone 除去が実測で悪化した前例あり）。
  #1〜#11 はいずれもマージ前にペア A/B とカウンタの両方が要る。
