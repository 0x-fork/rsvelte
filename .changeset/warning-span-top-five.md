---
'@rsvelte/compiler': patch
---

fix(analyze): attach the node's span to `event_directive_deprecated`, `element_invalid_self_closing_tag`, `export_let_unused`, `non_reactive_update` and `options_missing_custom_element`, which previously reported no position at all
