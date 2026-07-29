use rsvelte_projection::svelte2tsx::{Svelte2TsxOptions, svelte2tsx};

#[test]
fn combined_component_slot_facts_keep_exact_output() {
    let source = "<C>{#if ok}<div slot=\"named\" let:item>{item}</div>{/if}{#snippet row()}x{/snippet}<span let:value>{value}</span></C>";
    let output = svelte2tsx(
        source,
        Svelte2TsxOptions {
            filename: "Component.svelte".to_string(),
            is_ts_file: true,
            ..Default::default()
        },
    )
    .expect("svelte2tsx")
    .code;

    let expected = concat!(
        "///<reference types=\"svelte\" />\n",
        ";function $$render() {\n",
        "async () => { { const $$_C0C = __sveltets_2_ensureComponent(C); const $$_C0 = new $$_C0C({ target: __sveltets_2_any(), props: {children:() => { return __sveltets_2_any(0); },}});if(ok){{const {/*Ωignore_startΩ*/$$_$$/*Ωignore_endΩ*/,item,} = $$_C0.$$slot_def[\"named\"];$$_$$;{ svelteHTML.createElement(\"div\", {});item; }}} const row/*Ωignore_positionΩ*/ = ()/*Ωignore_startΩ*/: ReturnType<import('svelte').Snippet>/*Ωignore_endΩ*/ => { async ()/*Ωignore_positionΩ*/ => { };return __sveltets_2_any(0)};{const {/*Ωignore_startΩ*/$$_$$/*Ωignore_endΩ*/,value,} = $$_C0.$$slot_def.default;$$_$$; { svelteHTML.createElement(\"span\", {});value; }} C}};\n",
        "return { props: {} as Record<string, never>, exports: {}, bindings: \"\", slots: {}, events: {} }}\n",
        "const Component__SvelteComponent_ = __sveltets_2_isomorphic_component(__sveltets_2_with_any_event($$render()));\n",
        "/*Ωignore_startΩ*/type Component__SvelteComponent_ = InstanceType<typeof Component__SvelteComponent_>;\n",
        "/*Ωignore_endΩ*/export default Component__SvelteComponent_;",
    );
    assert_eq!(output, expected);
}
