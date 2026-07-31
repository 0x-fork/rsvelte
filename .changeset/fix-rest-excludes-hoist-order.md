---
"@rsvelte/compiler": patch
---

fix(compiler): keep the hoisted `rest_excludes` set ahead of the template factory in dev, where the factory is wrapped in `$.add_locations(...)`
