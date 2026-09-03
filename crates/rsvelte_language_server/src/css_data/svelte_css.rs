//! Svelte's additions to the CSS data, generated — do not edit.
//!
//! Source: `packages/language-server/src/plugins/css/features/svelte-selectors.ts` and `packages/language-server/src/plugins/css/service.ts` of language-tools, read
//! out of their build (MIT).
//!
//!   sha256 aa5b2453647df241c5513bce23017e428b5d5ac62a74b62ac6363a82396ca930 (svelte-selectors.ts)
//!   sha256 01221aeb55125640e7a36e3aaff19af36b2b29762d8f6566cf84ebc2c40c85f8 (service.ts)
//!   sha256 d7c8585de5a81dfea968c3555f6fa46c219bb25a33cd0442532fc8954d0ef681 (the svelte-selectors build read)
//!   sha256 ce55d48d7ee01e4d891b7c14a9255f88565d95b6e5d2f6d1f9e5a19697b7b204 (the service build read)
//!
//! `service.ts` also declares `vector-effect` and `print-color-adjust`, which
//! [`super::web`] already carries — `CSSDataManager.collectData` is first-wins
//! with the built-in provider pushed first, so they are shadowed and contribute
//! nothing. The generator compares the two live services and refuses to write
//! this file if that stops holding.
//!
//! Regenerate with `node scripts/dev/generate-css-data.mjs`.

use super::web::{Entry, Reference};

pub const SVELTE_PSEUDO_CLASSES: &[Entry] = &[Entry {
    name: ":global()",
    description: Some("[svelte] :global modifier\n\nApplying styles to a selector globally"),
    browsers: None,
    references: &[Reference {
        name: "Svelte.dev Reference",
        url: "https://svelte.dev/docs/svelte/global-styles",
    }],
    baseline: None,
    status: None,
}];
