---
"@rsvelte/language-server": patch
---

Vendor the CSS data the official language server reads, with the provenance discipline `html_data/` already uses: the version comes out of language-tools' `pnpm-lock.yaml`, the resolved package has to agree with it, and the SHA-256 of every file read is recorded in the generated header. `getEntryDescription` is ported rather than wrapped and compared to the function itself on all 3,194 entries in both markup kinds.
