//! Named-slot children of a component and the `$$slot_def` lowering.
//! Mirrors `htmlxtojsx_v2/nodes/slot.ts` and `Let.ts`.

use crate::ast::template::{
    Attribute, AttributeValue, AttributeValuePart, Component, Fragment, LetDirective,
    RegularElement, SvelteElement, TemplateNode,
};
use crate::svelte2tsx::magic_string::MagicString;
use crate::svelte2tsx::svelte2tsx::{Svelte2TsxOptions, slice_src};

use crate::svelte2tsx::template::attributes::attribute::format_attribute_node;
use crate::svelte2tsx::template::attributes::binding::format_bind_directive;
use crate::svelte2tsx::template::attributes::class_style::build_class_style_directive_suffix_segments;
use crate::svelte2tsx::template::attributes::event_handler::format_on_directive;
use crate::svelte2tsx::template::attributes::let_::{
    build_let_destructure_string, get_let_directives,
};
use crate::svelte2tsx::template::attributes::spread::format_spread_attribute;
use crate::svelte2tsx::template::attributes::transition::format_transition_directive;
use crate::svelte2tsx::template::ctx::{Counter, TemplateNodeExt};
use crate::svelte2tsx::template::segs::segs_to_string;
use crate::svelte2tsx::template::utils::source::{find_closing_tag_start, find_opening_tag_end};
use crate::svelte2tsx::template::walk::{process_fragment_inplace, process_node_inplace};

use super::inline_component::handle_component;
use super::slot_element::get_slot_attr_value;
use crate::svelte2tsx::template::attributes::action::format_use_directive;

/// True if `attributes` contains a `slot` attribute whose value is anything
/// other than the static string `"default"` — i.e. a *non-default* slot target.
///
/// Mirrors official `handleImplicitChildren`'s skip condition:
/// `a.name === 'slot' && a.value[0]?.data !== 'default'`. A dynamic
/// `slot={foo}` (no static `.data`) counts as non-default, as does any static
/// `slot="name"` except `slot="default"`.
pub(crate) fn has_non_default_slot_attr(attributes: &[Attribute], _source: &str) -> bool {
    for attr in attributes {
        if let Attribute::Attribute(node) = attr
            && node.name == "slot"
        {
            // Read the static text data of the first value part, if any.
            let value0_data: Option<String> = match &node.value {
                AttributeValue::Sequence(parts) => match parts.first() {
                    Some(AttributeValuePart::Text(text)) => Some(text.raw.to_string()),
                    _ => None,
                },
                _ => None,
            };
            return value0_data.as_deref() != Some("default");
        }
    }
    false
}

const HAS_DEFAULT_CONTENT: u8 = 1 << 0;
const HAS_NAMED_SLOT: u8 = 1 << 1;
const HAS_DEFAULT_SLOT_LET: u8 = 1 << 2;
const HAS_DIRECT_SNIPPET: u8 = 1 << 3;
const ALL_SLOT_FACTS: u8 =
    HAS_DEFAULT_CONTENT | HAS_NAMED_SLOT | HAS_DEFAULT_SLOT_LET | HAS_DIRECT_SNIPPET;

#[derive(Clone, Copy, Default)]
pub(crate) struct ComponentSlotFacts {
    bits: u8,
    #[cfg(test)]
    nodes_visited: usize,
}

impl ComponentSlotFacts {
    pub(crate) fn collect(fragment: &Fragment) -> Self {
        let mut facts = Self::default();
        facts.collect_direct(fragment);
        facts
    }

    #[inline]
    pub(crate) fn has_default_content(self) -> bool {
        self.bits & HAS_DEFAULT_CONTENT != 0
    }

    #[inline]
    pub(crate) fn has_named_slot(self) -> bool {
        self.bits & HAS_NAMED_SLOT != 0
    }

    #[inline]
    pub(crate) fn has_default_slot_let(self) -> bool {
        self.bits & HAS_DEFAULT_SLOT_LET != 0
    }

    #[inline]
    pub(crate) fn has_direct_snippet(self) -> bool {
        self.bits & HAS_DIRECT_SNIPPET != 0
    }

    fn collect_direct(&mut self, fragment: &Fragment) {
        for node in &fragment.nodes {
            self.record_visit();

            if matches!(node, TemplateNode::SnippetBlock(_)) {
                self.bits |= HAS_DIRECT_SNIPPET;
            }

            let (attributes, filters_default_content, can_be_named, can_have_default_let) =
                match node {
                    TemplateNode::RegularElement(el) => (Some(&el.attributes), true, true, true),
                    TemplateNode::Component(el) => (Some(&el.attributes), true, true, false),
                    TemplateNode::SvelteFragment(el) => (Some(&el.attributes), true, true, true),
                    TemplateNode::SvelteElement(el) => (Some(&el.attributes), true, true, true),
                    TemplateNode::SlotElement(el) => (Some(&el.attributes), false, true, false),
                    TemplateNode::SvelteSelf(el) => (Some(&el.attributes), true, false, false),
                    TemplateNode::SvelteComponent(el) => (Some(&el.attributes), true, false, false),
                    _ => (None, false, false, false),
                };
            let attr_facts = attributes.map_or_else(SlotAttributeFacts::default, |attributes| {
                collect_slot_attribute_facts(attributes)
            });

            if can_be_named && attr_facts.has_named_slot {
                self.bits |= HAS_NAMED_SLOT;
            }
            if can_have_default_let && attr_facts.has_let {
                self.bits |= HAS_DEFAULT_SLOT_LET;
            }

            let has_default_content = match node {
                TemplateNode::Text(text) => text.data.chars().any(|c| !c.is_whitespace()),
                TemplateNode::SnippetBlock(_)
                | TemplateNode::Comment(_)
                | TemplateNode::SlotElement(_) => false,
                _ if filters_default_content && attr_facts.has_non_default_slot => false,
                _ => true,
            };
            if has_default_content {
                self.bits |= HAS_DEFAULT_CONTENT;
            }

            if self.bits & HAS_NAMED_SLOT == 0 {
                self.collect_named_slots_in_control_flow(node);
            }
            if self.bits == ALL_SLOT_FACTS {
                return;
            }
        }
    }

    fn collect_named_slots(&mut self, fragment: &Fragment) {
        for node in &fragment.nodes {
            self.record_visit();
            let attributes = match node {
                TemplateNode::RegularElement(el) => Some(&el.attributes),
                TemplateNode::Component(el) => Some(&el.attributes),
                TemplateNode::SvelteFragment(el) => Some(&el.attributes),
                TemplateNode::SlotElement(el) => Some(&el.attributes),
                TemplateNode::SvelteElement(el) => Some(&el.attributes),
                _ => None,
            };
            if attributes.is_some_and(|attributes| has_named_slot_attr(attributes)) {
                self.bits |= HAS_NAMED_SLOT;
                return;
            }
            self.collect_named_slots_in_control_flow(node);
            if self.bits & HAS_NAMED_SLOT != 0 {
                return;
            }
        }
    }

    fn collect_named_slots_in_control_flow(&mut self, node: &TemplateNode) {
        match node {
            TemplateNode::IfBlock(block) => {
                self.collect_named_slots(&block.consequent);
                if self.bits & HAS_NAMED_SLOT == 0
                    && let Some(alternate) = &block.alternate
                {
                    self.collect_named_slots(alternate);
                }
            }
            TemplateNode::EachBlock(block) => {
                self.collect_named_slots(&block.body);
                if self.bits & HAS_NAMED_SLOT == 0
                    && let Some(fallback) = &block.fallback
                {
                    self.collect_named_slots(fallback);
                }
            }
            TemplateNode::AwaitBlock(block) => {
                if let Some(pending) = &block.pending {
                    self.collect_named_slots(pending);
                }
                if self.bits & HAS_NAMED_SLOT == 0
                    && let Some(then) = &block.then
                {
                    self.collect_named_slots(then);
                }
                if self.bits & HAS_NAMED_SLOT == 0
                    && let Some(catch) = &block.catch
                {
                    self.collect_named_slots(catch);
                }
            }
            TemplateNode::KeyBlock(block) => self.collect_named_slots(&block.fragment),
            _ => {}
        }
    }

    #[inline]
    fn record_visit(&mut self) {
        #[cfg(test)]
        {
            self.nodes_visited += 1;
        }
    }
}

#[derive(Clone, Copy, Default)]
struct SlotAttributeFacts {
    has_named_slot: bool,
    has_non_default_slot: bool,
    has_let: bool,
}

fn collect_slot_attribute_facts(attributes: &[Attribute]) -> SlotAttributeFacts {
    let mut facts = SlotAttributeFacts::default();
    let mut found_slot = false;
    for attribute in attributes {
        match attribute {
            Attribute::Attribute(attribute) if attribute.name == "slot" => {
                if !found_slot {
                    facts.has_non_default_slot = match &attribute.value {
                        AttributeValue::Sequence(parts) => !matches!(
                            parts.first(),
                            Some(AttributeValuePart::Text(text)) if text.raw == "default"
                        ),
                        _ => true,
                    };
                    found_slot = true;
                }
                facts.has_named_slot |= match &attribute.value {
                    AttributeValue::Sequence(parts) => parts.iter().any(
                        |part| matches!(part, AttributeValuePart::Text(text) if !text.raw.is_empty()),
                    ),
                    _ => false,
                };
            }
            Attribute::LetDirective(_) => facts.has_let = true,
            _ => {}
        }
        if found_slot && facts.has_named_slot && facts.has_let {
            break;
        }
    }
    facts
}

#[inline]
fn has_named_slot_attr(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        matches!(
            attribute,
            Attribute::Attribute(attribute)
                if attribute.name == "slot"
                    && matches!(
                        &attribute.value,
                        AttributeValue::Sequence(parts)
                            if parts.iter().any(|part| matches!(
                                part,
                                AttributeValuePart::Text(text) if !text.raw.is_empty()
                            ))
                    )
        )
    })
}

/// Check if a component's fragment has meaningful children for slot purposes.
///
/// Returns true if the component has any non-text children, or text children
/// with non-whitespace content.
pub(crate) fn has_component_slot_children(fragment: &Fragment, source: &str) -> bool {
    for node in &fragment.nodes {
        match node {
            TemplateNode::Text(text) => {
                // Use the DECODED `text.data` (HTML entities resolved), not the
                // raw source: `&nbsp;` decodes to U+00A0 which IS whitespace, so
                // `<Component>&nbsp;</Component>` has no meaningful default-slot
                // content and must not get a synthetic `children` prop. Mirrors
                // upstream `handleImplicitChildren`'s `node.data` check.
                if text.data.chars().any(|c| !c.is_whitespace()) {
                    return true;
                }
            }
            // `{#snippet}` blocks are passed as implicit *props*, not as
            // default-slot content, so they must not trigger the synthetic
            // `children` prop (which would otherwise produce a false
            // `'children' does not exist in type '$$ComponentProps'`).
            // Comments are likewise ignorable. Mirrors upstream
            // `handleImplicitChildren`, which skips `SnippetBlock` / `Comment`
            // and only fakes a `children` prop for a real default-slot child.
            TemplateNode::SnippetBlock(_) | TemplateNode::Comment(_) => {}
            // A `<slot>` child never contributes default-slot content — official
            // `handleImplicitChildren` skips every `child.type === 'Slot'`
            // unconditionally (it forwards a slot, it isn't slotted content).
            TemplateNode::SlotElement(_) => {}
            // Non-default-slot children (`<el slot="name">`, `slot={dynamic}`,
            // `<svelte:fragment slot="name">`, etc.) populate their slot, NOT
            // the default `children` prop, so they must not trigger the
            // synthetic `children`. Only default-slot content (no `slot=`, or
            // `slot="default"`) counts. Mirrors upstream `handleImplicitChildren`
            // which skips any child whose `slot` value isn't `"default"`.
            TemplateNode::RegularElement(el)
                if has_non_default_slot_attr(&el.attributes, source) => {}
            TemplateNode::Component(c) if has_non_default_slot_attr(&c.attributes, source) => {}
            TemplateNode::SvelteFragment(f) if has_non_default_slot_attr(&f.attributes, source) => {
            }
            TemplateNode::SvelteElement(e) if has_non_default_slot_attr(&e.attributes, source) => {}
            TemplateNode::SvelteSelf(s) if has_non_default_slot_attr(&s.attributes, source) => {}
            TemplateNode::SvelteComponent(sc)
                if has_non_default_slot_attr(&sc.attributes, source) => {}
            _ => return true,
        }
    }
    false
}

/// Process component children with slot awareness.
///
/// This handles:
/// - Default slot wrapping with `let:` destructuring
/// - Named slot wrapping with `slot="name"` children
pub(crate) fn process_component_children_with_slots(
    comp: &Component,
    inst_var: &str,
    let_directives: &[&LetDirective],
    source: &str,
    options: &Svelte2TsxOptions,
    str: &mut MagicString,
    counter: &mut Counter,
    depth: u32,
) {
    let has_lets = !let_directives.is_empty();

    // Build the default slot destructuring if needed
    let let_destructure = build_let_destructure_string(let_directives, source);

    // Group children into default slot and named slots
    // For each child, determine if it belongs to a named slot or the default slot
    // Named slot children get their own $$slot_def blocks
    // Default slot children are wrapped in a single block with the component's let: destructuring

    // We need to track which children are named slots and process them specially.
    // The approach: iterate over children, and for each named-slot child, emit
    // a separate $$slot_def block. Non-named-slot children are part of the default slot.
    //
    // The default slot block is opened before the first default slot child and closed
    // after the last one (or before the first named slot child).

    let mut default_slot_opened = false;
    let mut prev_end: Option<u32> = None;

    // If there are let: directives, we need to open the default slot block
    // before any children (including text nodes).
    if has_lets {
        // We'll open the default slot block at the position of the first child
        // or immediately after the opening tag
        let block_open = format!(
            "{{const {{/*\u{03A9}ignore_start\u{03A9}*/$$_$$/*\u{03A9}ignore_end\u{03A9}*/,{}}} = {}.$$slot_def.default;$$_$$;",
            let_destructure, inst_var
        );

        // Find where to insert the block open
        if let Some(first_node) = comp.fragment.nodes.first() {
            let first_start = first_node.start();
            // Insert the block opening before the first child
            str.append_left(first_start, &block_open);
        }
        default_slot_opened = true;
    }

    for (i, node) in comp.fragment.nodes.iter().enumerate() {
        let is_named_slot = match node {
            TemplateNode::RegularElement(el) => {
                get_slot_attr_value(&el.attributes, source).is_some()
            }
            TemplateNode::Component(child_comp) => {
                get_slot_attr_value(&child_comp.attributes, source).is_some()
            }
            TemplateNode::SvelteFragment(el) => {
                get_slot_attr_value(&el.attributes, source).is_some()
            }
            _ => false,
        };

        if is_named_slot {
            // The default slot's `$$slot_def.default` block stays open
            // through all children. Each named slot child carries its
            // own inner `$$slot_def["..."]` block (handled by the
            // dedicated handlers below); they're nested inside the
            // outer default block.

            // Process the named slot child (children of the parent component are at depth+1)
            match node {
                TemplateNode::RegularElement(el) => {
                    handle_named_slot_element(el, inst_var, source, options, str, counter, depth);
                }
                TemplateNode::Component(child_comp) => {
                    handle_named_slot_component(
                        child_comp, inst_var, source, options, str, counter, depth,
                    );
                }
                TemplateNode::SvelteFragment(el) => {
                    handle_named_slot_svelte_fragment(
                        el, inst_var, source, options, str, counter, depth,
                    );
                }
                _ => {
                    process_node_inplace(node, source, options, str, counter, depth);
                }
            }

            // Re-open default slot block after this named slot child if needed
            if has_lets {
                // Check if there are more non-named-slot children after this
                let _has_more_default = comp.fragment.nodes[i + 1..].iter().any(|n| match n {
                    TemplateNode::RegularElement(el) => {
                        get_slot_attr_value(&el.attributes, source).is_none()
                    }
                    TemplateNode::Component(c) => {
                        get_slot_attr_value(&c.attributes, source).is_none()
                    }
                    TemplateNode::SvelteFragment(el) => {
                        get_slot_attr_value(&el.attributes, source).is_none()
                    }
                    TemplateNode::Text(_) => true,
                    _ => true,
                });

                // Don't re-open if there are no more default slot children
                // Actually, we should re-open for any remaining children
                // We'll handle this below
            }
        } else {
            // Default slot child - process normally
            // If the default slot block was closed for a named slot, re-open it
            if has_lets && !default_slot_opened {
                let block_open = format!(
                    "{{const {{/*\u{03A9}ignore_start\u{03A9}*/$$_$$/*\u{03A9}ignore_end\u{03A9}*/,{}}} = {}.$$slot_def.default;$$_$$;",
                    let_destructure, inst_var
                );
                str.append_left(node.start(), &block_open);
                default_slot_opened = true;
            }
            // A default-slot child (`<svelte:fragment let:foo>`, `<div let:foo>`)
            // with no `slot=` but its OWN `let:` directives needs a
            // `$$slot_def.default` destructure block referencing the ENCLOSING
            // component — JS reference's Element.performTransformation emits one
            // whenever the default-slot child has `let:` directives. Wrap the
            // child so the `let:` bindings are scoped to its body.
            //
            // A COMPONENT child (`<Child let:foo>`) is excluded: its `let:foo`
            // binds from `Child`'s OWN `$$slot_def.default`, which its own
            // `handle_component` already emits. Routing it through the parent
            // here would wrongly duplicate the destructure onto the parent
            // instance (#1232).
            let fragment_lets: Option<Vec<&LetDirective>> = match node {
                TemplateNode::SvelteFragment(el) => {
                    let lets = get_let_directives(&el.attributes);
                    if lets.is_empty() { None } else { Some(lets) }
                }
                TemplateNode::RegularElement(el) => {
                    let lets = get_let_directives(&el.attributes);
                    if lets.is_empty() { None } else { Some(lets) }
                }
                _ => None,
            };
            let fragment_block_open = if let Some(ref lets) = fragment_lets {
                let destructure = build_let_destructure_string(lets, source);
                let block = format!(
                    "{{const {{/*\u{03A9}ignore_start\u{03A9}*/$$_$$/*\u{03A9}ignore_end\u{03A9}*/,{}}} = {}.$$slot_def.default;$$_$$;",
                    destructure, inst_var
                );
                str.append_left(node.start(), &block);
                true
            } else {
                false
            };
            // Mark the component slot context so a `slot="…"` element nested
            // inside this default-slot child's control-flow blocks (`{#if}` /
            // `{#each}` / …) is lowered to the named-slot form referencing this
            // component instance. A nested element/component clears it (each
            // owns its own slot scope) via `handle_regular_element`'s `take()`.
            let prev_slot = counter.slot_inst.replace(inst_var.to_string());
            process_node_inplace(node, source, options, str, counter, depth);
            counter.slot_inst = prev_slot;
            if fragment_block_open {
                str.append_left(node.end(), "}");
            }
        }

        prev_end = Some(node.end());
    }

    // Close the default slot block if still open
    if default_slot_opened && has_lets {
        // Find the position to close: after the last node, before the closing tag
        if let Some(end) = prev_end {
            let closing_tag_start = find_closing_tag_start(source, comp.end);
            if closing_tag_start < comp.end {
                str.append_left(closing_tag_start, "}");
            } else {
                str.append_left(end, "}");
            }
        }
    }
}

/// Handle a regular element child with `slot="name"` attribute inside a component.
///
/// Wraps the element in a `$$slot_def["name"]` destructuring block.
pub(crate) fn handle_named_slot_element(
    el: &RegularElement,
    inst_var: &str,
    source: &str,
    options: &Svelte2TsxOptions,
    str: &mut MagicString,
    counter: &mut Counter,
    depth: u32,
) {
    let slot_name = get_slot_attr_value(&el.attributes, source).unwrap_or_default();
    let let_directives = get_let_directives(&el.attributes);
    let let_destructure = build_let_destructure_string(&let_directives.to_vec(), source);

    // Build the slot def block opener
    let block_open = format!(
        "{{const {{/*\u{03A9}ignore_start\u{03A9}*/$$_$$/*\u{03A9}ignore_end\u{03A9}*/,{}}} = {}.$$slot_def[\"{}\"];$$_$$;",
        let_destructure, inst_var, slot_name
    );

    // Build attributes string excluding `slot` and `let:` directives
    let attrs_str = build_named_slot_element_attrs(&el.attributes, source);

    let opening_tag_end = find_opening_tag_end(source, el.start, el.end);

    // class:/style: directives lower to statements after createElement
    // (`class:bar` → ` bar;`), same as a regular element. The `let:` binding
    // itself is consumed by the `$$slot_def[…]` destructure above (and any use
    // in the body emits its own reference), so it is NOT re-emitted here.
    let class_style_suffix = segs_to_string(
        &build_class_style_directive_suffix_segments(&el.attributes, source),
        source,
    );

    // NOTE: the `let:foo={bar}` binding is reflected purely via the slot-def
    // destructure (`{ …, foo: bar } = …$$slot_def["…"]`); official emits NO
    // separate `bar;` reflection statement (that would duplicate the `{bar}`
    // content expression).
    let opener = format!(
        "{}{{ svelteHTML.createElement(\"{}\", {{{}}});{}",
        block_open, el.name, attrs_str, class_style_suffix
    );
    str.overwrite(el.start, opening_tag_end, &opener);

    // This named-slot element is a RegularElement — its children are at depth+1.
    process_fragment_inplace(&el.fragment, source, options, str, counter, depth + 1);

    // Void elements (`<input slot="x">`) and source-self-closing tags have no
    // `</tag>`; calling `find_closing_tag_start` would scan backward and match
    // an unrelated earlier `</…>` (e.g. `</script>`), overwriting everything in
    // between. Append the closing braces at `el.end` instead. Mirrors
    // `handle_regular_element`.
    let is_self_closing_source = slice_src(source, el.start as usize, el.end as usize)
        .trim_end()
        .ends_with("/>");
    let is_void = crate::compiler::utils::is_void_element(&el.name);
    if is_void || is_self_closing_source {
        str.append_left(el.end, " }}");
    } else {
        let closing_tag_start = find_closing_tag_start(source, el.end);
        if closing_tag_start < el.end {
            str.overwrite(closing_tag_start, el.end, " }}");
        } else {
            str.append_left(el.end, " }}");
        }
    }
}

/// Handle a `<svelte:fragment slot="name" let:foo>` child inside a parent
/// component. `<svelte:fragment>` itself doesn't render to HTML — it's a
/// virtual element used to distribute children into a named slot. The JS
/// reference still emits a `svelteHTML.createElement("svelte:fragment", { })`
/// (with `slot` and `let:` attributes stripped), wrapped in the slot let
/// destructure block.
pub(crate) fn handle_named_slot_svelte_fragment(
    el: &SvelteElement,
    inst_var: &str,
    source: &str,
    options: &Svelte2TsxOptions,
    str: &mut MagicString,
    counter: &mut Counter,
    depth: u32,
) {
    let slot_name = get_slot_attr_value(&el.attributes, source).unwrap_or_default();
    let let_directives = get_let_directives(&el.attributes);
    let let_destructure = build_let_destructure_string(&let_directives.to_vec(), source);

    // Leading ` ` matches the JS reference, which produces
    // `\t {const ... ;{ svelteHTML.createElement(...)` after the tab indent
    // is preserved.
    let block_open = format!(
        " {{const {{/*\u{03A9}ignore_start\u{03A9}*/$$_$$/*\u{03A9}ignore_end\u{03A9}*/,{}}} = {}.$$slot_def[\"{}\"];$$_$$;",
        let_destructure, inst_var, slot_name
    );

    let opening_tag_end = find_opening_tag_end(source, el.start, el.end);
    let closing_tag_start = find_closing_tag_start(source, el.end);
    let has_closing_tag = closing_tag_start < el.end;

    // Emit the slot-def block + a `svelteHTML.createElement("svelte:fragment", {  })`
    // with the `slot` / `let:` attributes stripped. The JS reference's
    // position-preserving emission leaves one space per stripped attribute
    // visible inside the empty `{}` (so `slot="x" let:y` → 2 spaces,
    // `slot="x" let:y let:z` → 3 spaces, etc.).
    let attrs_str = build_named_slot_element_attrs(&el.attributes, source);
    let inner = if attrs_str.is_empty() {
        let stripped_count = el
            .attributes
            .iter()
            .filter(|a| {
                matches!(
                    a,
                    Attribute::Attribute(node)
                        if node.name == "slot"
                ) || matches!(a, Attribute::LetDirective(_))
            })
            .count();
        " ".repeat(stripped_count.max(1))
    } else {
        attrs_str
    };
    let opener = format!(
        "{}{{ svelteHTML.createElement(\"svelte:fragment\", {{{}}});",
        block_open, inner
    );

    if !has_closing_tag {
        // Self-closing `<svelte:fragment slot="x" />` — body has no nodes.
        let combined = format!("{} }}}}", opener);
        str.overwrite(el.start, el.end, &combined);
        return;
    }

    str.overwrite(el.start, opening_tag_end, &opener);
    // `<svelte:fragment slot=…>` emits its own `createElement("svelte:fragment")`,
    // so it is an element nesting level — children (their `$$_<name><depth>`
    // instance vars) are at depth + 1.
    process_fragment_inplace(&el.fragment, source, options, str, counter, depth + 1);
    str.overwrite(closing_tag_start, el.end, " }}");
}

/// Handle a component child with `slot="name"` attribute inside a parent component.
pub(crate) fn handle_named_slot_component(
    comp: &Component,
    inst_var: &str,
    source: &str,
    options: &Svelte2TsxOptions,
    str: &mut MagicString,
    counter: &mut Counter,
    depth: u32,
) {
    let slot_name = get_slot_attr_value(&comp.attributes, source).unwrap_or_default();
    let let_directives = get_let_directives(&comp.attributes);
    let let_destructure = build_let_destructure_string(&let_directives.to_vec(), source);

    // Build the slot def block opener
    let block_open = format!(
        "{{const {{/*\u{03A9}ignore_start\u{03A9}*/$$_$$/*\u{03A9}ignore_end\u{03A9}*/,{}}} = {}.$$slot_def[\"{}\"];$$_$$;",
        let_destructure, inst_var, slot_name
    );

    // Insert the block opener before the component
    str.append_left(comp.start, &block_open);

    // Process the component normally. Suppress its component-name reference at
    // the close so we can emit it *outside* the component's own block (matching
    // official `endTransformation` order: component-block `}`, then `Name`, then
    // the named-slot-block `}`).
    counter.named_slot_component_close = true;
    counter.suppress_component_lets = true;
    handle_component(comp, source, options, str, counter, depth);

    // Emit the component-name reference (non-self-closing only — official maps
    // `</Name>` to `Name`; self-closing components have no name reference) and
    // close the named-slot block.
    let closing_tag_start = find_closing_tag_start(source, comp.end);
    if closing_tag_start < comp.end {
        str.append_left(comp.end, &format!(" {}}}", comp.name));
    } else {
        str.append_left(comp.end, "}");
    }
}

/// Build attribute string for a named slot element, excluding `slot` and `let:` directives.
pub(crate) fn build_named_slot_element_attrs(attributes: &[Attribute], source: &str) -> String {
    let mut parts: Vec<String> = Vec::new();

    for attr in attributes {
        match attr {
            Attribute::Attribute(node) => {
                if node.name == "slot" {
                    continue;
                }
                // Named-slot elements become `svelteHTML.createElement(…)` calls,
                // so they are real DOM elements — apply data-* wrapping.
                if let Some(s) = format_attribute_node(node, source, true) {
                    parts.push(s);
                }
            }
            Attribute::SpreadAttribute(spread) => {
                if let Some(s) = format_spread_attribute(spread, source) {
                    parts.push(s);
                }
            }
            Attribute::BindDirective(bind) => {
                parts.push(format_bind_directive(bind, source));
            }
            Attribute::OnDirective(on) => {
                parts.push(format_on_directive(on, source));
            }
            Attribute::ClassDirective(_) | Attribute::StyleDirective(_) => {
                // class:/style: are not props — they lower to statements after
                // createElement (see the suffix in handle_named_slot_element).
            }
            Attribute::TransitionDirective(transition) => {
                if let Some(s) = format_transition_directive(transition, source) {
                    parts.push(s);
                }
            }
            Attribute::UseDirective(use_dir) => {
                if let Some(s) = format_use_directive(use_dir, source) {
                    parts.push(s);
                }
            }
            // Skip let: directives and animate
            Attribute::AnimateDirective(_) | Attribute::LetDirective(_) => {}
            Attribute::AttachTag(_) => {}
        }
    }

    let result = parts.join("");
    if result.is_empty() {
        result
    } else {
        format!(" {}", result)
    }
}

#[cfg(test)]
mod tests {
    use oxc_allocator::Allocator;

    use crate::ast::template::TemplateNode;
    use crate::compiler::phases::phase1_parse::{ParseOptions, parse};

    use super::ComponentSlotFacts;

    fn first_component(source: &str) -> crate::ast::template::Component<'_> {
        let root = parse(source, &Allocator::default(), ParseOptions::default()).expect("parse");
        let TemplateNode::Component(component) = root
            .fragment
            .nodes
            .into_iter()
            .next()
            .expect("root component")
        else {
            panic!("expected component");
        };
        *component
    }

    #[test]
    fn component_slot_facts_cover_direct_and_transparent_children() {
        let source = "<C>{#if ok}<div slot=\"named\" let:item />{/if}{#snippet row()}x{/snippet}<span let:value>text</span></C>";
        let component = first_component(source);
        let facts = ComponentSlotFacts::collect(&component.fragment);

        assert!(facts.has_default_content());
        assert!(facts.has_named_slot());
        assert!(facts.has_default_slot_let());
        assert!(facts.has_direct_snippet());
    }

    #[test]
    fn component_slot_facts_visit_each_direct_child_once() {
        const CHILDREN: usize = 1_024;
        let source = format!("<C>{}</C>", "<!--ignored-->".repeat(CHILDREN));
        let component = first_component(&source);
        let facts = ComponentSlotFacts::collect(&component.fragment);

        assert_eq!(component.fragment.nodes.len(), CHILDREN);
        assert_eq!(facts.nodes_visited, CHILDREN);
        assert_eq!(facts.bits, 0);
    }
}
