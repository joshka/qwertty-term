# Draft issue: terminal: highlight.Flattened.init doesn't compile (dead code, two field bugs)

<!-- DRAFT ONLY — do not file as-is. Review, edit, and file manually.
     Per Ghostty's AI policy, the disclosure line at the bottom must be kept
     (edit it to reflect your actual review). -->

## Title

`terminal: highlight.Flattened.init doesn't compile when referenced`

## Body

`src/terminal/highlight.zig`'s `Flattened.init` has no in-tree callers (the search
subsystem constructs `Flattened` via `.empty` and manual appends), so Zig's lazy analysis
never type-checks its body. If anything ever calls it, it fails to compile, for two
reasons:

1. Line 146 builds the chunk list as `std.MultiArrayList(PageChunk)` where `PageChunk` is
   `PageList.PageIterator.Chunk` (fields `node`/`start`/`end`). The append at line 151
   writes `.serial = chunk.node.serial`, which that struct doesn't have. It should be
   `std.MultiArrayList(Chunk)` using the `Flattened.Chunk` declared just above (which
   exists precisely to add `serial`), and that is also the type the `chunks` field
   requires.
2. Line 158 constructs the result with `.end_x = end.x`, but the struct field is `bot_x`
   (`top_x`/`bot_x` everywhere else in the type — `clone`, `endPin`, and `untracked` all
   read the end pin's x from `bot_x`).

### Reproduction

On `main` (`c41c6b81a464`), force analysis of the function body:

```zig
// in src/terminal/highlight.zig, inside Flattened:
test "Flattened.init compiles" {
    _ = &Flattened.init;
}
```

```
$ zig build test -Dtest-filter="Flattened.init compiles"
src/terminal/highlight.zig:151:14: error: no field named 'serial' in struct 'terminal.PageList.PageIterator.Chunk'
            .serial = chunk.node.serial,
             ^~~~~~
```

Fixing bug 1 then surfaces bug 2 (`no field named 'end_x'`).

### Expected

`Flattened.init` compiles; probably also worth a small unit test or a
`refAllDecls`-style guard so dead public API stays compilable
(`terminal/main.zig`'s existing `refAllDecls` references the type but doesn't analyze
function bodies).

### Suggested fix

```zig
-        var result: std.MultiArrayList(PageChunk) = .empty;
+        var result: std.MultiArrayList(Chunk) = .empty;
...
-            .end_x = end.x,
+            .bot_x = end.x,
```

### Version

- Commit: `c41c6b81a464` (also present at `2da015cd6`)
- Zig 0.15.2, macOS aarch64

---

*AI disclosure: this defect was found while porting the module with AI assistance
(Claude Code); the reproduction and this report were AI-drafted and human-reviewed and
edited before filing.*
