use rsvelte_projection::svelte2tsx::{Svelte2TsxOptions, SvelteVersion, svelte2tsx};

fn component_name_lines(source: &str, options: Svelte2TsxOptions) -> String {
    let code = svelte2tsx(source, options).expect("svelte2tsx").code;
    code.lines()
        .filter(|line| line.contains("ExportShape__SvelteComponent_"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn component_export_name_is_exact_across_export_shapes() {
    let legacy_js = component_name_lines(
        "<script>export let value</script>{value}",
        Svelte2TsxOptions {
            filename: "ExportShape.svelte".into(),
            version: SvelteVersion::V4,
            ..Default::default()
        },
    );
    assert_eq!(
        legacy_js,
        "export default class ExportShape__SvelteComponent_ extends __sveltets_2_createSvelte2TsxComponent(__sveltets_2_partial(__sveltets_2_with_any_event($$render()))) {"
    );

    let legacy_ts_with_events_and_slots = component_name_lines(
        r#"<script lang="ts">
            import { createEventDispatcher } from 'svelte';
            export let value: string;
            const dispatch = createEventDispatcher<{save: string}>();
        </script>
        <button on:click={() => dispatch('save', value)}><slot /></button>"#,
        Svelte2TsxOptions {
            filename: "ExportShape.svelte".into(),
            is_ts_file: true,
            ..Default::default()
        },
    );
    assert_eq!(
        legacy_ts_with_events_and_slots,
        "const ExportShape__SvelteComponent_ = __sveltets_2_isomorphic_component_slots(__sveltets_2_with_any_event($$render()));\n/*Ωignore_startΩ*/type ExportShape__SvelteComponent_ = InstanceType<typeof ExportShape__SvelteComponent_>;\n/*Ωignore_endΩ*/export default ExportShape__SvelteComponent_;"
    );

    let runes_js_with_top_level_await = component_name_lines(
        "<script>const value = await Promise.resolve(1)</script>{value}",
        Svelte2TsxOptions {
            filename: "ExportShape.svelte".into(),
            emit_jsdoc: true,
            ..Default::default()
        },
    );
    assert_eq!(
        runes_js_with_top_level_await,
        "export const ExportShape__SvelteComponent_ = __sveltets_2_fn_component($$$render);\n/*Ωignore_startΩ*//** @typedef {ReturnType<typeof ExportShape__SvelteComponent_>} ExportShape__SvelteComponent_ */\n/*Ωignore_endΩ*/export default ExportShape__SvelteComponent_;"
    );

    let runes_ts_generics = component_name_lines(
        r#"<script lang="ts" generics="T extends string">
            let { value }: { value: T } = $props();
        </script>
        <button on:click><slot />{value}</button>"#,
        Svelte2TsxOptions {
            filename: "ExportShape.svelte".into(),
            is_ts_file: true,
            ..Default::default()
        },
    );
    assert_eq!(
        runes_ts_generics,
        "const ExportShape__SvelteComponent_: $$IsomorphicComponent = null as any;\n/*Ωignore_startΩ*/type ExportShape__SvelteComponent_<T extends string> = InstanceType<typeof ExportShape__SvelteComponent_<T>>;\n/*Ωignore_endΩ*/export default ExportShape__SvelteComponent_;"
    );
}
