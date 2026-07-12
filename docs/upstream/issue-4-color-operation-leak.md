# Draft issue: terminal: OSC color_operation requests list leaks (Parser.reset no-op arm)

<!-- DRAFT ONLY — do not file as-is. Review, edit, and file manually.
     Per Ghostty's AI policy, the disclosure line at the bottom must be kept
     (edit it to reflect your actual review). -->

## Title

`terminal: OSC 4/104/etc. color_operation requests list is never freed (memory leak)`

## Body

`osc.Parser.reset()` (`src/terminal/osc.zig:399`) cleans up `.kitty_color_protocol`'s
allocated list, but `.color_operation` falls into the explicit no-op arm. Its `requests`
list (`std.SegmentedList(Request, 2)`, allocated with `parser.alloc` in
`src/terminal/osc/parsers/color.zig`) is owned by `parser.command` and is never deinited
there — and no downstream consumer frees it either: `stream.zig` forwards it and both
handlers (`src/termio/stream_handler.zig:327`, `src/terminal/stream_terminal.zig:260`)
take `*const List`.

The leak is masked by the SegmentedList's prealloc of 2: sequences with at most two
color operations never touch the heap. Three or more operations in a single OSC allocate
dynamic shelves that leak. Existing tests only send 1-2 operations per OSC through the
`Parser`, so `std.testing.allocator` never trips. Theme-switching scripts that set the
whole palette in one `OSC 4` (16-256 pairs) leak on every invocation, in both the app
and libghostty-vt.

### Reproduction

```zig
test "color_operation requests leak on Parser.reset" {
    const testing = std.testing;
    var p: osc.Parser = .init(testing.allocator);
    defer p.deinit();

    // 3 set requests: one more than the SegmentedList prealloc (2).
    const input = "4;0;rgb:aa/bb/cc;1;rgb:bb/cc/dd;2;rgb:cc/dd/ee";
    for (input) |ch| p.next(ch);
    const cmd = p.end(0x07).?;
    try testing.expect(cmd.* == .color_operation);
    try testing.expectEqual(3, cmd.color_operation.requests.count());
}
```

```console
$ zig build test -Dtest-filter="color_operation requests leak"
[gpa] (err): memory address 0x... leaked:
    lib/std/segmented_list.zig:198:63: in growCapacity
    src/terminal/osc/parsers/color.zig:239:38: in parseGetSetAnsiColor
    ...
1 failed; 1 leaked
```

Equivalent through the stream:
`s.nextSlice("\x1b]4;0;rgb:aa/bb/cc;1;rgb:bb/cc/dd;2;rgb:cc/dd/ee\x1b\\")`.

### Expected vs actual

- Expected: `Parser.reset()`/`deinit()` releases everything the parser allocated, as it
  does for `kitty_color_protocol`.
- Actual: `color_operation.requests`'s heap shelves are dropped; a few hundred bytes leak
  per ≥3-operation color OSC.

### Suggested fix

Handle `.color_operation` next to `.kitty_color_protocol` in `reset()`:

```zig
.color_operation => |*v| color_operation: {
    v.requests.deinit(self.alloc orelse break :color_operation);
},
```

(Consumers only borrow the list during dispatch, so freeing on reset is safe — reset
happens on the next `OscStart` or `deinit`.)

### Version

- Commit: `c41c6b81a464` (also present at `2da015cd6`)
- Zig 0.15.2, macOS aarch64

---

*AI disclosure: this defect was found while porting the module with AI assistance
(Claude Code); the reproduction and this report were AI-drafted and human-reviewed and
edited before filing.*
