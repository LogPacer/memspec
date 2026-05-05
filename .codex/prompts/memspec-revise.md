---
description: "Use the memspec-synthesize skill to append a new inline revision to a .memspec file capturing the semantic changes since the last revision. Source-rewriting; append-only; replay-validated."
argument-hint: "<file.memspec> [--reason \"<concrete reason>\"] [--author <name>]"
---

Use the memspec-synthesize skill for this workflow and follow its rules exactly.

Treat any text after the prompt name as the target `.memspec` path and optional revision metadata. Pass a concrete `--reason` describing the semantic change.
