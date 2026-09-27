# JSON5 parser provenance extension

`crates/json5` contains the pinned json5-rs 1.3.1 parser and serializer by Callum Oakley, copied from the published crate with its MIT LICENSE. Its upstream source and deserialization/serialization fixtures are retained. Benchmark-only dependencies and benchmark targets are omitted; runtime dependencies are unchanged.

The local extension is opt-in `Spanned<T>` / `SpannedSeed<S>`. The existing Serde deserializer runs the wrapped seed, then reports its original UTF-8 token bounds, including quotes and collection delimiters and excluding surrounding comments/whitespace. Object keys use the same native key parser, including owned/escaped names. The extension uses a reserved Serde newtype name; ordinary parsing/serialization stays unchanged.

Merman configuration capture consumes these ranges in its existing value visitor, trims only parsed string quote delimiters for scalar/key selections, and carries array indices as numeric path components. Existing textual diagnostic matching cannot traverse array indices. Escaped keys are source-addressable but remain unsafe for direct key rewriting; whole-directive migration safety is unchanged. Original preprocessing edit composition remains responsible for BOM, CRLF and Markdown/source coordinates.

Checks: unchanged upstream JSON5 tests, token/key/error regressions, native configuration and diagnostic tests, and Trace saved-SVG/browser/family acceptance. This is the same owning parser with provenance added, not a parallel configuration parser.
