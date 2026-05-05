---
description: "Use the memspec-revisions skill to generate a revision-1 genesis manifest for an existing .memspec file. Source-preserving (does not modify the file). Run before /memspec-revise on files without an inline revisions block."
argument-hint: "<file.memspec> [--reason \"initial import\"] [--author <name>]"
---

Use the memspec-revisions skill for this workflow and follow its rules exactly.

Treat any text after the prompt name as the target `.memspec` path and optional genesis metadata. The source file must not be modified.
