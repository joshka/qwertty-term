//! `Page`, `Row`, `Cell`, and the single-allocation layout. Port of the core of
//! `src/terminal/page.zig`.
//!
//! A page is one contiguous, page-aligned, zero-initialized allocation holding
//! a fixed-capacity grid plus all its side tables, addressed entirely by byte
//! offsets from the base so the block can be relocated without fixups. Layout,
//! in order (each section aligned forward, total aligned to the OS page size):
//!
//! ```text
//! [Rows] [Cells] [Styles] [GraphemeAlloc] [GraphemeMap]
//! [StringAlloc] [HyperlinkSet] [HyperlinkMap]
//! ```
//!
//! **Zero-initialized memory is a valid empty page** — a load-bearing invariant
//! (`Cell`/`Row` zero values are valid, map metadata zero = free, set table
//! zero = no item). `init_buf` only fixes up row->cell offsets and section
//! headers.
//!
//! Backing memory here is `alloc_zeroed` with page-size alignment rather than
//! raw mmap (keeps the module Miri-runnable and platform-independent; the mmap
//! pool is a PageList-chunk concern). See `docs/analysis/page-memory.md`.

use std::alloc::{Layout as AllocLayout, alloc_zeroed, dealloc};

use super::bitmap::BitmapAllocator;
use super::hyperlink::{self, EntryId, HyperlinkContext, HyperlinkSet, PageEntry};
use super::offset_map::OffsetHashMap;
use super::ref_set::{AddError, SetId};
use super::size::{
    CellCountInt, GraphemeBytesInt, HyperlinkCountInt, Offset, OffsetBuf, OffsetInt, OffsetSlice,
    StringBytesInt, StyleCountInt, get_offset,
};
use super::style::{self, Style, StyleContext, StyleSet};

/// Re-exported from the bitmap allocator for the public error surface.
pub use super::bitmap::OutOfMemory;

// ---- allocator tuning (page.zig:84-122) ----

/// Codepoints per grapheme chunk. Skin-tone emoji and combiners usually fit.
const GRAPHEME_CHUNK_LEN: usize = 4;
/// Grapheme chunk size in bytes (`u21` is `u32` here, so 16 bytes).
const GRAPHEME_CHUNK: usize = GRAPHEME_CHUNK_LEN * size_of::<u32>();
type GraphemeAlloc = BitmapAllocator<GRAPHEME_CHUNK>;
/// The grapheme map: cell offset -> codepoint slice.
type GraphemeMap = OffsetHashMap<Offset<Cell>, OffsetSlice<u32>>;

/// Default grapheme bytes (`bitmap_bit_size` chunks).
pub const GRAPHEME_BYTES_DEFAULT: u32 = (64 * GRAPHEME_CHUNK) as u32;

/// Bytes per string chunk (OSC8 IDs/URIs).
const STRING_CHUNK: usize = 32;
type StringAlloc = BitmapAllocator<STRING_CHUNK>;
/// Default string bytes (`bitmap_bit_size` chunks).
pub const STRING_BYTES_DEFAULT: u32 = (64 * STRING_CHUNK) as u32;

/// Default number of hyperlinks supported.
const HYPERLINK_COUNT_DEFAULT: usize = 4;
/// Cells per hyperlink entry for map sizing (a link may span many cells).
const HYPERLINK_CELL_MULTIPLIER: usize = 16;

/// `size_of` a hyperlink set item, used to size `hyperlink_bytes`.
fn hyperlink_item_size() -> usize {
    HyperlinkSet::item_size()
}

const fn align_forward(v: usize, align: usize) -> usize {
    (v + align - 1) & !(align - 1)
}
const fn align_backward(v: usize, align: usize) -> usize {
    v & !(align - 1)
}

/// The OS page size we align the total allocation to. Fixed at 4 KiB — matches
/// `std.heap.page_size_min` on the platforms we target and keeps layout math
/// deterministic across hosts (Miri included).
const PAGE_SIZE_MIN: usize = 4096;

// ---- Row (page.zig:1907) ----

/// A row: an offset to its cells plus per-row flags. Port of `page.zig` `Row`
/// (`packed struct(u64)`); `#[repr(transparent)]` over `u64`, LSB-first bit
/// layout, so zero = valid empty and byte copies survive.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
#[repr(transparent)]
pub struct Row(u64);

/// Semantic prompt state for a row. Port of `Row.SemanticPrompt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SemanticPrompt {
    None = 0,
    Prompt = 1,
    PromptContinuation = 2,
}

impl Row {
    // Bit layout (LSB first): cells:u32, wrap, wrap_continuation, grapheme,
    // styled, hyperlink, semantic_prompt:u2, kitty_virtual_placeholder, dirty,
    // pad:u23.
    const CELLS_MASK: u64 = 0xFFFF_FFFF;
    const WRAP: u64 = 1 << 32;
    const WRAP_CONT: u64 = 1 << 33;
    const GRAPHEME: u64 = 1 << 34;
    const STYLED: u64 = 1 << 35;
    const HYPERLINK: u64 = 1 << 36;
    const SEM_PROMPT_SHIFT: u64 = 37; // 2 bits
    const KITTY_PLACEHOLDER: u64 = 1 << 39;
    const DIRTY: u64 = 1 << 40;

    #[inline]
    fn new_with_cells(cells: Offset<Cell>) -> Self {
        Row(cells.get() as u64)
    }

    #[inline]
    pub fn cells(self) -> Offset<Cell> {
        Offset::new((self.0 & Self::CELLS_MASK) as OffsetInt)
    }
    #[inline]
    fn set_cells(&mut self, cells: Offset<Cell>) {
        self.0 = (self.0 & !Self::CELLS_MASK) | cells.get() as u64;
    }

    #[inline]
    fn flag(self, bit: u64) -> bool {
        self.0 & bit != 0
    }
    #[inline]
    fn set_flag(&mut self, bit: u64, v: bool) {
        if v {
            self.0 |= bit;
        } else {
            self.0 &= !bit;
        }
    }

    #[inline]
    pub fn wrap(self) -> bool {
        self.flag(Self::WRAP)
    }
    #[inline]
    pub fn set_wrap(&mut self, v: bool) {
        self.set_flag(Self::WRAP, v);
    }
    #[inline]
    pub fn wrap_continuation(self) -> bool {
        self.flag(Self::WRAP_CONT)
    }
    #[inline]
    pub fn set_wrap_continuation(&mut self, v: bool) {
        self.set_flag(Self::WRAP_CONT, v);
    }
    #[inline]
    pub fn grapheme(self) -> bool {
        self.flag(Self::GRAPHEME)
    }
    #[inline]
    pub fn set_grapheme(&mut self, v: bool) {
        self.set_flag(Self::GRAPHEME, v);
    }
    #[inline]
    pub fn styled(self) -> bool {
        self.flag(Self::STYLED)
    }
    #[inline]
    pub fn set_styled(&mut self, v: bool) {
        self.set_flag(Self::STYLED, v);
    }
    #[inline]
    pub fn hyperlink(self) -> bool {
        self.flag(Self::HYPERLINK)
    }
    #[inline]
    pub fn set_hyperlink(&mut self, v: bool) {
        self.set_flag(Self::HYPERLINK, v);
    }
    #[inline]
    pub fn kitty_virtual_placeholder(self) -> bool {
        self.flag(Self::KITTY_PLACEHOLDER)
    }
    #[inline]
    pub fn set_kitty_virtual_placeholder(&mut self, v: bool) {
        self.set_flag(Self::KITTY_PLACEHOLDER, v);
    }
    #[inline]
    pub fn dirty(self) -> bool {
        self.flag(Self::DIRTY)
    }
    #[inline]
    pub fn set_dirty(&mut self, v: bool) {
        self.set_flag(Self::DIRTY, v);
    }
    #[inline]
    pub fn semantic_prompt(self) -> SemanticPrompt {
        match (self.0 >> Self::SEM_PROMPT_SHIFT) & 0b11 {
            1 => SemanticPrompt::Prompt,
            2 => SemanticPrompt::PromptContinuation,
            _ => SemanticPrompt::None,
        }
    }
    #[inline]
    pub fn set_semantic_prompt(&mut self, v: SemanticPrompt) {
        self.0 =
            (self.0 & !(0b11 << Self::SEM_PROMPT_SHIFT)) | ((v as u64) << Self::SEM_PROMPT_SHIFT);
    }

    /// C ABI value.
    #[inline]
    pub fn cval(self) -> u64 {
        self.0
    }

    /// True if this row has managed memory (graphemes/styles/hyperlinks). Port
    /// of `Row.managedMemory`.
    #[inline]
    pub fn managed_memory(self) -> bool {
        self.styled() || self.hyperlink() || self.grapheme()
    }
}

// ---- Cell (page.zig:2011) ----

/// The content tag. Port of `Cell.ContentTag`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ContentTag {
    Codepoint = 0,
    CodepointGrapheme = 1,
    BgColorPalette = 2,
    BgColorRgb = 3,
}

/// The wide property. Port of `Cell.Wide`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Wide {
    Narrow = 0,
    Wide = 1,
    SpacerTail = 2,
    SpacerHead = 3,
}

/// The semantic content type. Port of `Cell.SemanticContent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SemanticContent {
    Output = 0,
    Input = 1,
    Prompt = 2,
}

/// A single grid cell. Port of `page.zig` `Cell` (`packed struct(u64)`);
/// `#[repr(transparent)]` over `u64`, LSB-first, zero = valid empty cell.
///
/// Bit layout (LSB first): content_tag:u2, content:u24 (codepoint u21 |
/// palette u8 | rgb 24), style_id:u16, wide:u2, protected:1, hyperlink:1,
/// semantic_content:u2, pad:u16.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
#[repr(transparent)]
pub struct Cell(u64);

impl Cell {
    const TAG_SHIFT: u64 = 0; // 2 bits
    const CONTENT_SHIFT: u64 = 2; // 24 bits
    const STYLE_SHIFT: u64 = 26; // 16 bits
    const WIDE_SHIFT: u64 = 42; // 2 bits
    const PROTECTED: u64 = 1 << 44;
    const HYPERLINK: u64 = 1 << 45;
    const SEM_SHIFT: u64 = 46; // 2 bits

    const CONTENT_MASK: u64 = 0xFF_FFFF; // 24 bits
    const STYLE_MASK: u64 = 0xFFFF;

    /// A codepoint cell. Port of `Cell.init`.
    #[inline]
    pub fn init(cp: u32) -> Self {
        let mut c = Cell(0);
        c.set_content_tag(ContentTag::Codepoint);
        c.set_codepoint(cp);
        c
    }

    #[inline]
    pub fn is_zero(self) -> bool {
        self.0 == 0
    }

    #[inline]
    pub fn cval(self) -> u64 {
        self.0
    }

    /// Reconstruct a cell from its raw `u64` bit representation. The inverse
    /// of [`Cell::cval`]. Used by the batched print fast path
    /// (`Terminal::print_slice`), which builds cell bit patterns from a
    /// template + codepoint shift exactly like upstream `printSliceFill`.
    #[inline]
    pub(crate) const fn from_cval(bits: u64) -> Self {
        Cell(bits)
    }

    /// Bit offset of the 24-bit content field within a cell (`content_tag`
    /// occupies the low 2 bits, content the next 24). Mirrors upstream's
    /// `@bitOffsetOf(Cell, "content")` used to OR a codepoint into a template.
    pub(crate) const CONTENT_BIT_OFFSET: u64 = Self::CONTENT_SHIFT;

    /// The masked-field test for a "simple" cell in the batched print fast
    /// path: content_tag == codepoint, wide == narrow, hyperlink == 0, and the
    /// style_id matching the cursor's. Mirrors upstream's `simple_mask`
    /// (`fieldMask(Cell, {content_tag, style_id, wide, hyperlink})`).
    pub(crate) const SIMPLE_MASK: u64 = (0b11 << Self::TAG_SHIFT)          // content_tag
        | (Self::STYLE_MASK << Self::STYLE_SHIFT)                          // style_id
        | (0b11 << Self::WIDE_SHIFT)                                       // wide
        | Self::HYPERLINK; // hyperlink

    /// The expected masked bits of a simple cell already carrying `style_id`
    /// (so no ref-count churn is needed to overwrite it). Port of
    /// `printSliceCheckExpected`.
    #[inline]
    pub(crate) const fn simple_check_expected(style_id: StyleCountInt) -> u64 {
        (style_id as u64) << Self::STYLE_SHIFT
    }

    #[inline]
    pub fn content_tag(self) -> ContentTag {
        match (self.0 >> Self::TAG_SHIFT) & 0b11 {
            0 => ContentTag::Codepoint,
            1 => ContentTag::CodepointGrapheme,
            2 => ContentTag::BgColorPalette,
            _ => ContentTag::BgColorRgb,
        }
    }
    #[inline]
    pub fn set_content_tag(&mut self, tag: ContentTag) {
        self.0 = (self.0 & !(0b11 << Self::TAG_SHIFT)) | ((tag as u64) << Self::TAG_SHIFT);
    }

    /// The raw 24-bit content field.
    #[inline]
    fn content_raw(self) -> u32 {
        ((self.0 >> Self::CONTENT_SHIFT) & Self::CONTENT_MASK) as u32
    }
    #[inline]
    fn set_content_raw(&mut self, v: u32) {
        self.0 = (self.0 & !(Self::CONTENT_MASK << Self::CONTENT_SHIFT))
            | (((v as u64) & Self::CONTENT_MASK) << Self::CONTENT_SHIFT);
    }

    /// Set the codepoint (low 21 bits of the content union).
    #[inline]
    pub fn set_codepoint(&mut self, cp: u32) {
        self.set_content_raw(cp & 0x1F_FFFF);
    }

    /// The codepoint, or 0 for bg-color-only cells. Port of `Cell.codepoint`.
    #[inline]
    pub fn codepoint(self) -> u32 {
        match self.content_tag() {
            ContentTag::Codepoint | ContentTag::CodepointGrapheme => self.content_raw() & 0x1F_FFFF,
            ContentTag::BgColorPalette | ContentTag::BgColorRgb => 0,
        }
    }

    /// The palette index for a bg-color-palette cell.
    #[inline]
    pub fn color_palette(self) -> u8 {
        self.content_raw() as u8
    }
    #[inline]
    pub fn set_color_palette(&mut self, idx: u8) {
        self.set_content_tag(ContentTag::BgColorPalette);
        self.set_content_raw(idx as u32);
    }

    /// The RGB triple stored in a `BgColorRgb` cell. Port of `Cell.content`'s
    /// `color_rgb` packed field (`RGB{ r, g, b }`, LSB-first: r=bits 0-7,
    /// g=8-15, b=16-23).
    #[inline]
    pub fn color_rgb(self) -> (u8, u8, u8) {
        let raw = self.content_raw();
        (raw as u8, (raw >> 8) as u8, (raw >> 16) as u8)
    }
    #[inline]
    pub fn set_color_rgb(&mut self, r: u8, g: u8, b: u8) {
        self.set_content_tag(ContentTag::BgColorRgb);
        self.set_content_raw((r as u32) | ((g as u32) << 8) | ((b as u32) << 16));
    }

    #[inline]
    pub fn style_id(self) -> StyleCountInt {
        ((self.0 >> Self::STYLE_SHIFT) & Self::STYLE_MASK) as StyleCountInt
    }
    #[inline]
    pub fn set_style_id(&mut self, id: StyleCountInt) {
        self.0 = (self.0 & !(Self::STYLE_MASK << Self::STYLE_SHIFT))
            | ((id as u64) << Self::STYLE_SHIFT);
    }

    #[inline]
    pub fn wide(self) -> Wide {
        match (self.0 >> Self::WIDE_SHIFT) & 0b11 {
            0 => Wide::Narrow,
            1 => Wide::Wide,
            2 => Wide::SpacerTail,
            _ => Wide::SpacerHead,
        }
    }
    #[inline]
    pub fn set_wide(&mut self, w: Wide) {
        self.0 = (self.0 & !(0b11 << Self::WIDE_SHIFT)) | ((w as u64) << Self::WIDE_SHIFT);
    }

    #[inline]
    pub fn protected(self) -> bool {
        self.0 & Self::PROTECTED != 0
    }
    #[inline]
    pub fn set_protected(&mut self, v: bool) {
        if v {
            self.0 |= Self::PROTECTED;
        } else {
            self.0 &= !Self::PROTECTED;
        }
    }

    #[inline]
    pub fn hyperlink(self) -> bool {
        self.0 & Self::HYPERLINK != 0
    }
    #[inline]
    pub fn set_hyperlink(&mut self, v: bool) {
        if v {
            self.0 |= Self::HYPERLINK;
        } else {
            self.0 &= !Self::HYPERLINK;
        }
    }

    #[inline]
    pub fn semantic_content(self) -> SemanticContent {
        match (self.0 >> Self::SEM_SHIFT) & 0b11 {
            1 => SemanticContent::Input,
            2 => SemanticContent::Prompt,
            _ => SemanticContent::Output,
        }
    }
    #[inline]
    pub fn set_semantic_content(&mut self, v: SemanticContent) {
        self.0 = (self.0 & !(0b11 << Self::SEM_SHIFT)) | ((v as u64) << Self::SEM_SHIFT);
    }

    /// True if there's text to render. Port of `Cell.hasText`.
    #[inline]
    pub fn has_text(self) -> bool {
        match self.content_tag() {
            ContentTag::Codepoint | ContentTag::CodepointGrapheme => self.codepoint() != 0,
            ContentTag::BgColorPalette | ContentTag::BgColorRgb => false,
        }
    }

    /// Grid width (1 or 2). Port of `Cell.gridWidth`.
    #[inline]
    pub fn grid_width(self) -> u8 {
        match self.wide() {
            Wide::Narrow | Wide::SpacerHead | Wide::SpacerTail => 1,
            Wide::Wide => 2,
        }
    }

    /// True if styled (non-default style). Port of `Cell.hasStyling`.
    #[inline]
    pub fn has_styling(self) -> bool {
        self.style_id() != style::DEFAULT_ID
    }

    /// True if no text and narrow. Port of `Cell.isEmpty`.
    #[inline]
    pub fn is_empty(self) -> bool {
        match self.content_tag() {
            ContentTag::Codepoint | ContentTag::CodepointGrapheme => {
                !self.has_text() && self.wide() == Wide::Narrow
            }
            ContentTag::BgColorPalette | ContentTag::BgColorRgb => false,
        }
    }

    /// True if the cell holds a multi-codepoint grapheme. Port of `hasGrapheme`.
    #[inline]
    pub fn has_grapheme(self) -> bool {
        self.content_tag() == ContentTag::CodepointGrapheme
    }

    /// True if any cell in the slice has text. Port of `Cell.hasTextAny`
    /// (`page.zig`). Used by PageList's trailing-blank-row trimming.
    #[inline]
    pub fn has_text_any(cells: &[Cell]) -> bool {
        cells.iter().any(|c| c.has_text())
    }
}

// ---- Size / Capacity (page.zig:1791-1905) ----

/// The current dimensions of a page. Port of `page.zig` `Size`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub cols: CellCountInt,
    pub rows: CellCountInt,
}

/// The capacity of a page (fixed at creation). Port of `page.zig` `Capacity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capacity {
    pub cols: CellCountInt,
    pub rows: CellCountInt,
    pub styles: StyleCountInt,
    pub hyperlink_bytes: HyperlinkCountInt,
    pub grapheme_bytes: GraphemeBytesInt,
    pub string_bytes: StringBytesInt,
}

impl Capacity {
    /// A capacity with default side-table sizes. Port of `Capacity`'s field
    /// defaults (styles=16, hyperlink=4 items, grapheme/string bytes default).
    pub fn new(cols: CellCountInt, rows: CellCountInt) -> Self {
        Self {
            cols,
            rows,
            styles: 16,
            hyperlink_bytes: (HYPERLINK_COUNT_DEFAULT * hyperlink_item_size()) as HyperlinkCountInt,
            grapheme_bytes: GRAPHEME_BYTES_DEFAULT,
            string_bytes: STRING_BYTES_DEFAULT,
        }
    }

    /// The standard capacity. Port of `page.zig` `std_capacity`.
    /// `grapheme_bytes` keeps upstream's test/non-test split.
    pub fn std() -> Self {
        Self {
            cols: 215,
            rows: 215,
            styles: 128,
            hyperlink_bytes: (HYPERLINK_COUNT_DEFAULT * hyperlink_item_size()) as HyperlinkCountInt,
            grapheme_bytes: if cfg!(test) { 512 } else { 8192 },
            string_bytes: STRING_BYTES_DEFAULT,
        }
    }

    /// Max columns that still fit at least one row without growing memory. Port
    /// of `Capacity.maxCols`.
    pub fn max_cols(self) -> Option<CellCountInt> {
        let available_bits = self.available_bits_for_grid();
        let row_bits = 64usize; // @bitSizeOf(Row)
        if available_bits <= row_bits {
            return None;
        }
        let remaining = available_bits - row_bits;
        let max = remaining / 64; // @bitSizeOf(Cell)
        Some((CellCountInt::MAX as usize).min(max) as CellCountInt)
    }

    /// Adjust `cols` while holding total size constant, trading rows. Port of
    /// `Capacity.adjust`.
    pub fn adjust_cols(self, cols: CellCountInt) -> Result<Capacity, OutOfMemory> {
        let available_bits = self.available_bits_for_grid();
        let bits_per_row = 64usize + 64usize * cols as usize; // Row + cols*Cell
        let new_rows = available_bits / bits_per_row;
        if new_rows == 0 {
            return Err(OutOfMemory);
        }
        let mut adjusted = self;
        adjusted.cols = cols;
        adjusted.rows = new_rows as CellCountInt;
        Ok(adjusted)
    }

    /// Bits available for rows+cells, found by laying the meta sections
    /// backward from the total size. Port of `availableBitsForGrid`.
    fn available_bits_for_grid(self) -> usize {
        // Requires no gap between rows and cells (Row size % Cell align == 0).
        debug_assert_eq!(size_of::<Row>() % align_of::<Cell>(), 0);
        let l = Layout::compute(self);

        let hyperlink_map_start = align_backward(
            l.total_size - l.hyperlink_map_layout.total_size,
            GraphemeMap::base_align(),
        );
        let hyperlink_set_start = align_backward(
            hyperlink_map_start - l.hyperlink_set_layout.total_size,
            HyperlinkSet::base_align(),
        );
        let string_alloc_start = align_backward(
            hyperlink_set_start - l.string_alloc_layout.total_size,
            StringAlloc::BASE_ALIGN,
        );
        let grapheme_map_start = align_backward(
            string_alloc_start - l.grapheme_map_layout.total_size,
            GraphemeMap::base_align(),
        );
        let grapheme_alloc_start = align_backward(
            grapheme_map_start - l.grapheme_alloc_layout.total_size,
            GraphemeAlloc::BASE_ALIGN,
        );
        let styles_start = align_backward(
            grapheme_alloc_start - l.styles_layout.total_size,
            StyleSet::base_align(),
        );
        styles_start * 8
    }
}

// ---- Layout (page.zig:1681-1776) ----

/// The single-allocation layout for a page at a given capacity. Port of
/// `page.zig` `Layout`.
#[derive(Clone, Copy)]
pub struct Layout {
    pub total_size: usize,
    rows_start: usize,
    cells_start: usize,
    styles_start: usize,
    styles_layout: super::ref_set::SetLayout,
    grapheme_alloc_start: usize,
    grapheme_alloc_layout: super::bitmap::BitmapLayout,
    grapheme_map_start: usize,
    grapheme_map_layout: super::offset_map::MapLayout,
    string_alloc_start: usize,
    string_alloc_layout: super::bitmap::BitmapLayout,
    hyperlink_set_start: usize,
    hyperlink_set_layout: super::ref_set::SetLayout,
    hyperlink_map_start: usize,
    hyperlink_map_layout: super::offset_map::MapLayout,
    capacity: Capacity,
}

impl Layout {
    /// Compute the layout for a capacity. Port of `Page.layout`.
    pub fn compute(cap: Capacity) -> Layout {
        let rows_count = cap.rows as usize;
        let rows_start = 0;
        let rows_end = rows_start + rows_count * size_of::<Row>();

        let cells_count = cap.cols as usize * cap.rows as usize;
        let cells_start = align_forward(rows_end, align_of::<Cell>());
        let cells_end = cells_start + cells_count * size_of::<Cell>();

        let styles_layout = StyleSet::layout(cap.styles as usize);
        let styles_start = align_forward(cells_end, StyleSet::base_align());
        let styles_end = styles_start + styles_layout.total_size;

        let grapheme_alloc_layout = GraphemeAlloc::layout(cap.grapheme_bytes as usize);
        let grapheme_alloc_start = align_forward(styles_end, GraphemeAlloc::BASE_ALIGN);
        let grapheme_alloc_end = grapheme_alloc_start + grapheme_alloc_layout.total_size;

        let grapheme_count: usize = if cap.grapheme_bytes == 0 {
            0
        } else {
            let base = (cap.grapheme_bytes as usize).div_ceil(GRAPHEME_CHUNK);
            base.next_power_of_two()
        };
        let grapheme_map_layout = GraphemeMap::layout(grapheme_count as u32);
        let grapheme_map_start = align_forward(grapheme_alloc_end, GraphemeMap::base_align());
        let grapheme_map_end = grapheme_map_start + grapheme_map_layout.total_size;

        let string_alloc_layout = StringAlloc::layout(cap.string_bytes as usize);
        let string_alloc_start = align_forward(grapheme_map_end, StringAlloc::BASE_ALIGN);
        let string_alloc_end = string_alloc_start + string_alloc_layout.total_size;

        let hyperlink_count = cap.hyperlink_bytes as usize / hyperlink_item_size();
        let hyperlink_set_layout = HyperlinkSet::layout(hyperlink_count);
        let hyperlink_set_start = align_forward(string_alloc_end, HyperlinkSet::base_align());
        let hyperlink_set_end = hyperlink_set_start + hyperlink_set_layout.total_size;

        let hyperlink_map_count: u32 = if hyperlink_count == 0 {
            0
        } else {
            match (hyperlink_count * HYPERLINK_CELL_MULTIPLIER).try_into() {
                Ok(m) => u32::next_power_of_two(m),
                Err(_) => u32::MAX,
            }
        };
        let hyperlink_map_layout = hyperlink::Map::layout(hyperlink_map_count);
        let hyperlink_map_start = align_forward(hyperlink_set_end, hyperlink::Map::base_align());
        let hyperlink_map_end = hyperlink_map_start + hyperlink_map_layout.total_size;

        let total_size = align_forward(hyperlink_map_end, PAGE_SIZE_MIN);

        Layout {
            total_size,
            rows_start,
            cells_start,
            styles_start,
            styles_layout,
            grapheme_alloc_start,
            grapheme_alloc_layout,
            grapheme_map_start,
            grapheme_map_layout,
            string_alloc_start,
            string_alloc_layout,
            hyperlink_set_start,
            hyperlink_set_layout,
            hyperlink_map_start,
            hyperlink_map_layout,
            capacity: cap,
        }
    }
}

// ---- Errors (page.zig) ----

/// Errors from grapheme operations. Port of `page.zig` `GraphemeError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphemeError {
    GraphemeAllocOutOfMemory,
    GraphemeMapOutOfMemory,
}

/// Errors from `insert_hyperlink`. Port of `page.zig` `InsertHyperlinkError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertHyperlinkError {
    StringsOutOfMemory,
    SetOutOfMemory,
    SetNeedsRehash,
}

/// Errors from the `clone_from` family. Port of `page.zig` `CloneFromError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneFromError {
    StyleSetOutOfMemory,
    StyleSetNeedsRehash,
    HyperlinkMapOutOfMemory,
    HyperlinkSetOutOfMemory,
    HyperlinkSetNeedsRehash,
    StringAllocOutOfMemory,
    GraphemeAllocOutOfMemory,
    GraphemeMapOutOfMemory,
}

impl From<GraphemeError> for CloneFromError {
    fn from(e: GraphemeError) -> Self {
        match e {
            GraphemeError::GraphemeAllocOutOfMemory => CloneFromError::GraphemeAllocOutOfMemory,
            GraphemeError::GraphemeMapOutOfMemory => CloneFromError::GraphemeMapOutOfMemory,
        }
    }
}

/// The capacity dimension exhausted during a reflow cell copy, so the caller
/// (`ReflowCursor`) knows which capacity to grow before retrying. Mirrors the
/// `IncreaseCapacity` cases used in `writeCell` plus `Rehash` (grow with no
/// dimension change).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReflowManagedError {
    Styles,
    GraphemeBytes,
    HyperlinkBytes,
    StringBytes,
    Rehash,
}

/// Page integrity violations. Port of `page.zig` `IntegrityError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityError {
    ZeroRowCount,
    ZeroColCount,
    MissingGraphemeData,
    UnmarkedGraphemeCell,
    UnmarkedStyleRow,
    MissingHyperlinkData,
    UnmarkedHyperlinkRow,
    UnmarkedHyperlinkCell,
    InvalidSpacerTailLocation,
    InvalidSpacerHeadLocation,
    UnwrappedSpacerHead,
    UnmarkedGraphemeRow,
    InvalidGraphemeCount,
    MismatchedStyleRef,
    MismatchedHyperlinkRef,
}

// ---- Page ----

/// A page: one contiguous, offset-addressed block holding a grid and its side
/// tables. Port of `page.zig` `Page`.
pub struct Page {
    /// Backing memory pointer (page-aligned) and length. `owned` records
    /// whether we allocated it (and must free on drop).
    mem: *mut u8,
    mem_len: usize,
    owned: bool,

    rows: Offset<Row>,
    /// Offset to the base of the cells array. Rows carry their own cell offset,
    /// so this is retained for model parity (and future `get_cells` bounds
    /// asserts / PageList use) rather than read on the hot path.
    #[allow(dead_code)]
    cells: Offset<Cell>,

    pub dirty: bool,

    pub(crate) string_alloc: StringAlloc,
    grapheme_alloc: GraphemeAlloc,
    grapheme_map: GraphemeMap,
    styles: StyleSet,
    hyperlink_map: hyperlink::Map,
    hyperlink_set: HyperlinkSet,

    pub size: Size,
    pub capacity: Capacity,

    /// Suspend counter for [`Page::assert_integrity`], used only with the
    /// `slow_runtime_safety` feature. Incremented while an operation
    /// temporarily leaves the page inconsistent (e.g. reflow clone). Always
    /// zero otherwise (the field costs a word but keeps the layout uniform).
    /// Port of `page.zig` `pause_integrity_checks`.
    #[cfg_attr(not(feature = "slow_runtime_safety"), allow(dead_code))]
    pause_integrity: usize,
}

impl Drop for Page {
    fn drop(&mut self) {
        if self.owned && !self.mem.is_null() {
            // SAFETY: allocated by `alloc_zeroed` with this exact layout.
            unsafe {
                dealloc(
                    self.mem,
                    AllocLayout::from_size_align(self.mem_len, PAGE_SIZE_MIN).unwrap(),
                );
            }
        }
    }
}

impl Page {
    /// The page's base memory pointer (immutable view). Used by hyperlink
    /// hashing/eql which chase offsets.
    #[inline]
    pub fn memory(&self) -> *const u8 {
        self.mem
    }
    /// The page's base memory pointer (mutable view).
    #[inline]
    pub fn memory_mut(&mut self) -> *mut u8 {
        self.mem
    }
    /// The page's base memory pointer for offset resolution. Unlike
    /// [`Self::memory_mut`] this takes `&self`, which is required by the
    /// scroll/edit paths that already hold a `*mut Page` and only read `mem`.
    #[inline]
    pub(crate) fn mem(&self) -> *mut u8 {
        self.mem
    }

    /// The byte length of this page's backing memory (its layout total size).
    #[inline]
    pub fn byte_len(&self) -> usize {
        self.mem_len
    }

    /// Allocate a new page with page-aligned zeroed backing memory. Port of
    /// `Page.init`.
    pub fn init(cap: Capacity) -> Page {
        let l = Layout::compute(cap);
        debug_assert_eq!(l.total_size % PAGE_SIZE_MIN, 0);
        // SAFETY: total_size is a nonzero multiple of PAGE_SIZE_MIN.
        let backing = unsafe {
            alloc_zeroed(AllocLayout::from_size_align(l.total_size, PAGE_SIZE_MIN).unwrap())
        };
        assert!(!backing.is_null(), "page backing allocation failed");
        // SAFETY: backing is `total_size` zeroed, page-aligned bytes we own.
        unsafe { Page::init_buf(backing, l.total_size, l, true) }
    }

    /// Initialize a page into caller-provided backing memory. Port of
    /// `Page.initBuf`. `owned` controls whether Drop frees the memory.
    ///
    /// # Safety
    ///
    /// `backing` must point at `total_size` zeroed bytes aligned to
    /// `PAGE_SIZE_MIN`, valid for the page's lifetime.
    pub unsafe fn init_buf(backing: *mut u8, total_size: usize, l: Layout, owned: bool) -> Page {
        let cap = l.capacity;
        let buf = OffsetBuf::new(backing);
        let rows: Offset<Row> = buf.member(l.rows_start);
        let cells: Offset<Cell> = buf.member(l.cells_start);

        // Fix up each row's cells offset (zeroed rows point at offset 0, which
        // is only valid for row 0). Everything else is valid zeroed.
        // SAFETY: rows/cells regions in bounds per caller contract.
        unsafe {
            let rows_ptr = rows.ptr(backing);
            for y in 0..cap.rows as usize {
                let start = y * cap.cols as usize;
                let cell_ptr = cells.ptr(backing).add(start);
                let cell_off = get_offset(backing, cell_ptr);
                rows_ptr.add(y).write(Row::new_with_cells(cell_off));
            }
        }

        // SAFETY: each sub-region is disjoint and sized per the layout.
        let styles =
            unsafe { StyleSet::init(buf.add(l.styles_start), l.styles_layout, StyleContext) };
        let string_alloc =
            unsafe { StringAlloc::init(buf.add(l.string_alloc_start), &l.string_alloc_layout) };
        let grapheme_alloc = unsafe {
            GraphemeAlloc::init(buf.add(l.grapheme_alloc_start), &l.grapheme_alloc_layout)
        };
        let grapheme_map =
            unsafe { GraphemeMap::init(buf.add(l.grapheme_map_start), &l.grapheme_map_layout) };
        let hyperlink_map = unsafe {
            hyperlink::Map::init(buf.add(l.hyperlink_map_start), &l.hyperlink_map_layout)
        };
        // Hyperlink set context is (re)bound per-operation; init null.
        let hyperlink_set = unsafe {
            HyperlinkSet::init(
                buf.add(l.hyperlink_set_start),
                l.hyperlink_set_layout,
                HyperlinkContext::null(),
            )
        };

        Page {
            mem: backing,
            mem_len: total_size,
            owned,
            rows,
            cells,
            dirty: false,
            string_alloc,
            grapheme_alloc,
            grapheme_map,
            styles,
            hyperlink_map,
            hyperlink_set,
            size: Size {
                cols: cap.cols,
                rows: cap.rows,
            },
            capacity: cap,
            pause_integrity: 0,
        }
    }

    /// Reinitialize the page with the same capacity (zero + init_buf). Port of
    /// `Page.reinit`.
    pub fn reinit(&mut self) {
        // SAFETY: we own mem_len zeroable bytes. We overwrite `*self` with
        // `ptr::write` (no Drop) because Drop would free the very memory the
        // rebuilt page reuses — a use-after-free. This mirrors Zig's
        // `self.* = initBuf(...)`, which runs no destructor.
        unsafe {
            std::ptr::write_bytes(self.mem, 0, self.mem_len);
            let l = Layout::compute(self.capacity);
            let owned = self.owned;
            let mem = self.mem;
            let len = self.mem_len;
            let rebuilt = Page::init_buf(mem, len, l, owned);
            std::ptr::write(self as *mut Page, rebuilt);
        }
    }

    /// Opt-in integrity assertion. Port of `assertIntegrity`. Runs
    /// [`Page::verify_integrity`] under the `slow_runtime_safety` feature
    /// (upstream's build option of the same name) and panics on violation;
    /// a no-op otherwise. The full scan is far too slow to run after every
    /// mutation in ordinary debug/test builds — use
    /// [`Page::verify_integrity`] directly for explicit checks.
    #[inline]
    pub(crate) fn assert_integrity(&self) {
        #[cfg(feature = "slow_runtime_safety")]
        {
            if self.pause_integrity > 0 {
                return;
            }
            if let Err(e) = self.verify_integrity() {
                panic!("page integrity check failed: {e:?}");
            }
        }
    }

    /// Suspend/resume integrity checks across a multi-step mutation. Port of
    /// `pauseIntegrityChecks`.
    #[inline]
    pub(crate) fn pause_integrity_checks(&mut self, pause: bool) {
        #[cfg(feature = "slow_runtime_safety")]
        {
            if pause {
                self.pause_integrity += 1;
            } else {
                self.pause_integrity -= 1;
            }
        }
        #[cfg(not(feature = "slow_runtime_safety"))]
        {
            let _ = pause;
        }
    }

    /// Full page integrity verification. Port of `page.zig` `verifyIntegrity`.
    ///
    /// Checks grapheme flag ⇔ map presence, style presence + row.styled flag +
    /// ref-count floor, hyperlink flag ⇔ map + row flag + live set entry,
    /// spacer_tail/spacer_head placement, and per-row grapheme flag. The zombie
    /// style check is disabled upstream (fast paths can leave them) so we omit
    /// it. Debug/test only.
    pub fn verify_integrity(&self) -> Result<(), IntegrityError> {
        use std::collections::HashMap;

        if self.size.rows == 0 {
            return Err(IntegrityError::ZeroRowCount);
        }
        if self.size.cols == 0 {
            return Err(IntegrityError::ZeroColCount);
        }

        let mut graphemes_seen: usize = 0;
        let mut styles_seen: HashMap<style::Id, usize> = HashMap::new();
        let mut hyperlinks_seen: HashMap<hyperlink::Id, usize> = HashMap::new();
        let grapheme_count = self.grapheme_count();

        // SAFETY: rows/cells regions valid; we read within size bounds.
        unsafe {
            let rows = self.rows.ptr(self.mem);
            for y in 0..self.size.rows as usize {
                let row = rows.add(y);
                let graphemes_start = graphemes_seen;
                let cells = (*row).cells().ptr(self.mem);
                for x in 0..self.size.cols as usize {
                    let cell = cells.add(x);

                    if (*cell).has_grapheme() {
                        if self.lookup_grapheme(cell).is_none() {
                            return Err(IntegrityError::MissingGraphemeData);
                        }
                        graphemes_seen += 1;
                    } else if grapheme_count > 0 && self.lookup_grapheme(cell).is_some() {
                        return Err(IntegrityError::UnmarkedGraphemeCell);
                    }

                    if (*cell).style_id() != style::DEFAULT_ID {
                        // `get` asserts the id is live.
                        let _ = self.styles.get(self.mem, (*cell).style_id());
                        if !(*row).styled() {
                            return Err(IntegrityError::UnmarkedStyleRow);
                        }
                        *styles_seen.entry((*cell).style_id()).or_insert(0) += 1;
                    }

                    if (*cell).hyperlink() {
                        let id = self
                            .lookup_hyperlink(cell)
                            .ok_or(IntegrityError::MissingHyperlinkData)?;
                        if !(*row).hyperlink() {
                            return Err(IntegrityError::UnmarkedHyperlinkRow);
                        }
                        *hyperlinks_seen.entry(id).or_insert(0) += 1;
                        let _ = self.hyperlink_set.get(self.mem, id);
                    } else if self.lookup_hyperlink(cell).is_some() {
                        return Err(IntegrityError::UnmarkedHyperlinkCell);
                    }

                    match (*cell).wide() {
                        Wide::Narrow | Wide::Wide => {}
                        Wide::SpacerTail => {
                            if x == 0 {
                                return Err(IntegrityError::InvalidSpacerTailLocation);
                            }
                            if (*cells.add(x - 1)).wide() != Wide::Wide {
                                return Err(IntegrityError::InvalidSpacerTailLocation);
                            }
                        }
                        Wide::SpacerHead => {
                            if x != self.size.cols as usize - 1 {
                                return Err(IntegrityError::InvalidSpacerHeadLocation);
                            }
                            if !(*row).wrap() {
                                return Err(IntegrityError::UnwrappedSpacerHead);
                            }
                        }
                    }
                }

                if graphemes_seen > graphemes_start && !(*row).grapheme() {
                    return Err(IntegrityError::UnmarkedGraphemeRow);
                }
            }

            if graphemes_seen > self.grapheme_count() {
                return Err(IntegrityError::InvalidGraphemeCount);
            }

            for (&id, &seen) in &styles_seen {
                if (self.styles.ref_count(self.mem, id) as usize) < seen {
                    return Err(IntegrityError::MismatchedStyleRef);
                }
            }
            for (&id, &seen) in &hyperlinks_seen {
                if (self.hyperlink_set.ref_count(self.mem, id) as usize) < seen {
                    return Err(IntegrityError::MismatchedHyperlinkRef);
                }
            }
        }

        Ok(())
    }

    // ---- accessors (page.zig:1030-1061) ----

    /// Get a mutable pointer to row `y`. Port of `getRow`.
    pub fn get_row(&self, y: usize) -> *mut Row {
        debug_assert!(y < self.size.rows as usize);
        // SAFETY: y in bounds; rows region valid.
        unsafe { self.rows.ptr(self.mem).add(y) }
    }

    /// Rotate the rows `[y_start, y_end)` right by one: `[0 1 2 3]` becomes
    /// `[3 0 1 2]`. Port of `fastmem.rotateOnceR(Row, rows)`. Rotates the `Row`
    /// structs (which hold cell/managed-memory offsets), physically leaving cell
    /// data in place but re-homing it to a different row index — exactly what the
    /// Zig scroll paths rely on. Integrity checks must be paused by the caller.
    ///
    /// # Safety
    ///
    /// `y_start < y_end <= size.rows`.
    pub(crate) unsafe fn rotate_rows_once_right(&mut self, y_start: usize, y_end: usize) {
        debug_assert!(y_start < y_end && y_end <= self.size.rows as usize);
        // SAFETY: range in bounds per caller contract; rows region valid.
        unsafe {
            let base = self.rows.ptr(self.mem);
            let last = std::ptr::read(base.add(y_end - 1));
            let mut i = y_end - 1;
            while i > y_start {
                std::ptr::write(base.add(i), std::ptr::read(base.add(i - 1)));
                i -= 1;
            }
            std::ptr::write(base.add(y_start), last);
        }
    }

    /// Get the cells slice for a row. Port of `getCells`.
    ///
    /// # Safety
    ///
    /// `row` must point at a valid row of this page.
    pub unsafe fn get_cells(&self, row: *const Row) -> *mut [Cell] {
        // SAFETY: row valid per caller; its cells offset addresses `cols` cells.
        unsafe {
            let cells = (*row).cells().ptr(self.mem);
            std::ptr::slice_from_raw_parts_mut(cells, self.size.cols as usize)
        }
    }

    /// Get pointers to the row and a cell at (x, y). Port of `getRowAndCell`.
    pub fn get_row_and_cell(&self, x: usize, y: usize) -> (*mut Row, *mut Cell) {
        debug_assert!(y < self.size.rows as usize && x < self.size.cols as usize);
        // SAFETY: x/y in bounds.
        unsafe {
            let row = self.rows.ptr(self.mem).add(y);
            let cell = (*row).cells().ptr(self.mem).add(x);
            (row, cell)
        }
    }

    /// Direct access to the style set (dedup + refcount). Exposed for the
    /// terminal layer and clone paths.
    pub fn styles(&mut self) -> &mut StyleSet {
        &mut self.styles
    }

    /// Direct access to the hyperlink set (dedup + refcount). Exposed for the
    /// Screen layer's cursor-hyperlink caching (release across page moves).
    ///
    /// Callers must rebind the set context (`bind_hyperlink_ctx`) before ops
    /// that hash/compare page-resident values; `release` on an id does not
    /// require it (release only touches the item meta), matching how Screen
    /// uses it.
    pub fn hyperlink_set_mut(&mut self) -> &mut hyperlink::HyperlinkSet {
        &mut self.hyperlink_set
    }

    // ---- clearCells (page.zig:1195) ----

    /// Clear cells `[left, end)` of a row, reclaiming grapheme/hyperlink/style
    /// memory. Port of `clearCells`.
    ///
    /// # Safety
    ///
    /// `row` must be a valid row of this page and `left <= end <= cols`.
    pub unsafe fn clear_cells(&mut self, row: *mut Row, left: usize, end: usize) {
        // SAFETY: row valid per caller contract.
        unsafe {
            let mem = self.mem;
            let cells_base = (*row).cells().ptr(mem);
            let len = end - left;
            let full_row = len == self.size.cols as usize;

            if (*row).grapheme() {
                for i in left..end {
                    let cell = cells_base.add(i);
                    if (*cell).has_grapheme() {
                        self.clear_grapheme(cell);
                    }
                }
                if full_row {
                    (*row).set_grapheme(false);
                } else {
                    self.update_row_grapheme_flag(row);
                }
            }

            if (*row).hyperlink() {
                for i in left..end {
                    let cell = cells_base.add(i);
                    if (*cell).hyperlink() {
                        self.clear_hyperlink(cell);
                    }
                }
                if full_row {
                    (*row).set_hyperlink(false);
                } else {
                    self.update_row_hyperlink_flag(row);
                }
            }

            if (*row).styled() {
                // Styled cells overwhelmingly come in runs sharing the same
                // style (a colored status bar, a highlighted region, a full
                // row painted in one color), so group consecutive cells with
                // the same style id and release each run with a single
                // ref-count update instead of per cell. The release_multiple
                // ref_count >= n contract holds by construction: every cell
                // in the run held a reference. Port of upstream Screen.zig
                // clearCells (8d663a76e).
                let mut i = left;
                while i < end {
                    let id = (*cells_base.add(i)).style_id();
                    if id == style::DEFAULT_ID {
                        i += 1;
                        continue;
                    }
                    let mut j = i + 1;
                    while j < end && (*cells_base.add(j)).style_id() == id {
                        j += 1;
                    }
                    self.styles
                        .release_multiple(mem, id, SetId::from_usize(j - i));
                    i = j;
                }
                if full_row {
                    (*row).set_styled(false);
                } else {
                    self.update_row_styled_flag(row);
                }
            }

            // kitty placeholder recompute (only on full-row clears).
            if (*row).kitty_virtual_placeholder() && full_row {
                let mut any = false;
                for i in left..end {
                    if (*cells_base.add(i)).codepoint() == KITTY_PLACEHOLDER {
                        any = true;
                        break;
                    }
                }
                if !any {
                    (*row).set_kitty_virtual_placeholder(false);
                }
            }

            // Zero the cells.
            for i in left..end {
                cells_base.add(i).write(Cell(0));
            }
        }
        self.assert_integrity();
    }

    /// Like [`clear_cells`](Self::clear_cells) but fills the range with `blank`
    /// instead of the zero cell. Used by `Screen::clear_cells` to preserve the
    /// cursor background color. Port of the `@memset(cells, self.blankCell())`
    /// tail of Screen's `clearCells`.
    ///
    /// # Safety
    ///
    /// `row` valid for this page; `[left, end)` in bounds; `blank` must carry no
    /// managed memory (grapheme/style/hyperlink) — it is written raw.
    pub unsafe fn fill_cells(&mut self, row: *mut Row, left: usize, end: usize, blank: Cell) {
        // SAFETY: delegated to clear_cells for the release + flag recompute; the
        // fill afterwards only overwrites the just-cleared range.
        unsafe {
            self.pause_integrity_checks(true);
            self.clear_cells(row, left, end);
            let cells_base = (*row).cells().ptr(self.mem);
            for i in left..end {
                cells_base.add(i).write(blank);
            }
            self.pause_integrity_checks(false);
        }
        self.assert_integrity();
    }

    // ---- grapheme ops (page.zig:1486-1660) ----

    /// Set the grapheme codepoints for a cell (asserts no existing graphemes).
    /// Port of `setGraphemes`.
    ///
    /// # Safety
    ///
    /// `row`/`cell` must be valid for this page; the cell must be a single
    /// codepoint > 0.
    pub unsafe fn set_graphemes(
        &mut self,
        row: *mut Row,
        cell: *mut Cell,
        cps: &[u32],
    ) -> Result<(), GraphemeError> {
        // SAFETY: row/cell valid per caller contract.
        unsafe {
            debug_assert!((*cell).codepoint() > 0);
            debug_assert_eq!((*cell).content_tag(), ContentTag::Codepoint);

            let mem = self.mem;
            let cell_offset = get_offset(mem, cell);
            let mut map = self.grapheme_map.map(mem);

            let slice = self
                .grapheme_alloc
                .alloc::<u32>(mem, cps.len())
                .map_err(|_| GraphemeError::GraphemeAllocOutOfMemory)?;
            slice.slice_mut(mem).copy_from_slice(cps);

            let val = OffsetSlice {
                offset: get_offset(mem, slice.offset.ptr(mem)),
                len: slice.len,
            };
            if map.ensure_unused_capacity(1).is_err() {
                self.grapheme_alloc.free(mem, slice);
                return Err(GraphemeError::GraphemeMapOutOfMemory);
            }
            map.put_assume_capacity_no_clobber(cell_offset, val);

            (*cell).set_content_tag(ContentTag::CodepointGrapheme);
            (*row).set_grapheme(true);
        }
        self.assert_integrity();
        Ok(())
    }

    /// Append a codepoint to a cell as a grapheme. Port of `appendGrapheme`.
    ///
    /// # Safety
    ///
    /// `row`/`cell` valid for this page; cell codepoint != 0.
    pub unsafe fn append_grapheme(
        &mut self,
        row: *mut Row,
        cell: *mut Cell,
        cp: u32,
    ) -> Result<(), GraphemeError> {
        // SAFETY: row/cell valid per caller contract.
        unsafe {
            let mem = self.mem;
            let cell_offset = get_offset(mem, cell);
            let mut map = self.grapheme_map.map(mem);

            if (*cell).content_tag() != ContentTag::CodepointGrapheme {
                let cps = self
                    .grapheme_alloc
                    .alloc::<u32>(mem, 1)
                    .map_err(|_| GraphemeError::GraphemeAllocOutOfMemory)?;
                cps.slice_mut(mem)[0] = cp;
                if map.ensure_unused_capacity(1).is_err() {
                    self.grapheme_alloc.free(mem, cps);
                    return Err(GraphemeError::GraphemeMapOutOfMemory);
                }
                map.put_assume_capacity_no_clobber(
                    cell_offset,
                    OffsetSlice {
                        offset: get_offset(mem, cps.offset.ptr(mem)),
                        len: 1,
                    },
                );
                (*cell).set_content_tag(ContentTag::CodepointGrapheme);
                (*row).set_grapheme(true);
                self.assert_integrity();
                return Ok(());
            }

            debug_assert!((*row).grapheme());
            let slice_ptr = map.get_ptr(&cell_offset).unwrap();

            // Fast path: spare space in the last chunk.
            if !(*slice_ptr).len.is_multiple_of(GRAPHEME_CHUNK_LEN) {
                let cps = (*slice_ptr).offset.ptr(mem);
                cps.add((*slice_ptr).len).write(cp);
                (*slice_ptr).len += 1;
                self.assert_integrity();
                return Ok(());
            }

            // Slow path: grow the chunk.
            let old_len = (*slice_ptr).len;
            let cps = self
                .grapheme_alloc
                .alloc::<u32>(mem, old_len + 1)
                .map_err(|_| GraphemeError::GraphemeAllocOutOfMemory)?;
            let old = *slice_ptr;
            let new_slice = cps.slice_mut(mem);
            new_slice[..old_len].copy_from_slice(old.slice(mem));
            new_slice[old_len] = cp;
            *slice_ptr = OffsetSlice {
                offset: get_offset(mem, cps.offset.ptr(mem)),
                len: old_len + 1,
            };
            self.grapheme_alloc.free(mem, old);
        }
        self.assert_integrity();
        Ok(())
    }

    /// Look up the extra grapheme codepoints for a cell. Port of `lookupGrapheme`.
    ///
    /// # Safety
    ///
    /// `cell` must be a valid cell of this page.
    pub unsafe fn lookup_grapheme(&self, cell: *const Cell) -> Option<*const [u32]> {
        // SAFETY: cell valid per caller.
        unsafe {
            let mem = self.mem;
            let cell_offset = get_offset(self.mem, cell);
            let map = self.grapheme_map.map(mem);
            let slice = map.get(&cell_offset)?;
            Some(slice.slice(self.mem) as *const [u32])
        }
    }

    /// Move graphemes between cells (map keyed by offset). Port of `moveGrapheme`.
    pub(crate) unsafe fn move_grapheme(&mut self, src: *mut Cell, dst: *mut Cell) {
        // SAFETY: cells valid per caller of the calling method.
        unsafe {
            let mem = self.mem;
            let src_offset = get_offset(mem, src);
            let dst_offset = get_offset(mem, dst);
            let mut map = self.grapheme_map.map(mem);
            let value = map.get(&src_offset).unwrap();
            map.remove(&src_offset);
            map.put_assume_capacity_no_clobber(dst_offset, value);
        }
    }

    /// Clear a cell's graphemes. Port of `clearGrapheme`.
    pub(crate) unsafe fn clear_grapheme(&mut self, cell: *mut Cell) {
        // SAFETY: cell valid per caller of the calling method.
        unsafe {
            let mem = self.mem;
            let cell_offset = get_offset(mem, cell);
            let mut map = self.grapheme_map.map(mem);
            let slice = map.get(&cell_offset).unwrap();
            self.grapheme_alloc.free(mem, slice);
            map.remove(&cell_offset);
            (*cell).set_content_tag(ContentTag::Codepoint);
        }
        self.assert_integrity();
    }

    /// Recompute a row's grapheme flag. Port of `updateRowGraphemeFlag`.
    pub(crate) unsafe fn update_row_grapheme_flag(&self, row: *mut Row) {
        // SAFETY: row valid per caller.
        unsafe {
            let base = (*row).cells().ptr(self.mem);
            for i in 0..self.size.cols as usize {
                if (*base.add(i)).has_grapheme() {
                    return;
                }
            }
            (*row).set_grapheme(false);
        }
    }

    /// Number of unique cells with grapheme data. Port of `graphemeCount`.
    pub fn grapheme_count(&self) -> usize {
        // SAFETY: map region valid.
        unsafe { self.grapheme_map.map(self.mem).count() as usize }
    }

    /// Grapheme map capacity. Port of `graphemeCapacity`.
    pub fn grapheme_capacity(&self) -> usize {
        // SAFETY: map region valid.
        unsafe { self.grapheme_map.map(self.mem).capacity() as usize }
    }

    /// Recompute a row's styled flag. Port of `updateRowStyledFlag`.
    unsafe fn update_row_styled_flag(&self, row: *mut Row) {
        // SAFETY: row valid per caller.
        unsafe {
            let base = (*row).cells().ptr(self.mem);
            for i in 0..self.size.cols as usize {
                if (*base.add(i)).has_styling() {
                    return;
                }
            }
            (*row).set_styled(false);
        }
    }

    // ---- hyperlink ops (page.zig:1273-1482) ----

    /// Look up the hyperlink ID for a cell. Port of `lookupHyperlink`.
    ///
    /// # Safety
    ///
    /// `cell` valid for this page.
    pub unsafe fn lookup_hyperlink(&self, cell: *const Cell) -> Option<hyperlink::Id> {
        // SAFETY: cell valid per caller.
        unsafe {
            let mem = self.mem;
            let cell_offset = get_offset(self.mem, cell);
            self.hyperlink_map.map(mem).get(&cell_offset)
        }
    }

    /// Clear a cell's hyperlink. Port of `clearHyperlink`.
    ///
    /// # Safety
    ///
    /// `cell` valid for this page.
    pub unsafe fn clear_hyperlink(&mut self, cell: *mut Cell) {
        // SAFETY: cell valid per caller.
        unsafe {
            let mem = self.mem;
            let cell_offset = get_offset(mem, cell);
            let mut map = self.hyperlink_map.map(mem);
            let Some(id) = map.get(&cell_offset) else {
                return;
            };
            self.bind_hyperlink_ctx();
            self.hyperlink_set.release(mem, id);
            map.remove(&cell_offset);
            (*cell).set_hyperlink(false);
        }
        self.assert_integrity();
    }

    /// Recompute a row's hyperlink flag. Port of `updateRowHyperlinkFlag`.
    pub(crate) unsafe fn update_row_hyperlink_flag(&self, row: *mut Row) {
        // SAFETY: row valid per caller.
        unsafe {
            let base = (*row).cells().ptr(self.mem);
            for i in 0..self.size.cols as usize {
                if (*base.add(i)).hyperlink() {
                    return;
                }
            }
            (*row).set_hyperlink(false);
        }
    }

    /// Set a cell's hyperlink to `id`, releasing the old one. Port of
    /// `setHyperlink`. Does NOT increment the new hyperlink's ref count.
    ///
    /// # Safety
    ///
    /// `row`/`cell` valid for this page.
    pub unsafe fn set_hyperlink(
        &mut self,
        row: *mut Row,
        cell: *mut Cell,
        id: hyperlink::Id,
    ) -> Result<(), OutOfMemory> {
        // SAFETY: row/cell valid per caller.
        unsafe {
            let mem = self.mem;
            let cell_offset = get_offset(mem, cell);
            let mut map = self.hyperlink_map.map(mem);
            let gop = match map.get_or_put(cell_offset) {
                Ok(g) => g,
                Err(_) => return Err(OutOfMemory),
            };

            if gop.found_existing {
                self.bind_hyperlink_ctx();
                self.hyperlink_set.release(mem, *gop.value_ptr);
                if *gop.value_ptr == id {
                    debug_assert!((*row).hyperlink());
                    (*cell).set_hyperlink(true);
                    self.assert_integrity();
                    return Ok(());
                }
            }

            *gop.value_ptr = id;
            (*cell).set_hyperlink(true);
            (*row).set_hyperlink(true);
        }
        self.assert_integrity();
        Ok(())
    }

    /// Move a hyperlink between cells (map keyed by offset). Does NOT touch cell
    /// flags. Port of `moveHyperlink`.
    unsafe fn move_hyperlink(&mut self, src: *mut Cell, dst: *mut Cell) {
        // SAFETY: cells valid per caller of the calling method.
        unsafe {
            let mem = self.mem;
            let src_offset = get_offset(mem, src);
            let dst_offset = get_offset(mem, dst);
            let mut map = self.hyperlink_map.map(mem);
            let value = map.get(&src_offset).unwrap();
            map.remove(&src_offset);
            map.put_assume_capacity_no_clobber(dst_offset, value);
        }
    }

    /// Number of unique cells with hyperlink data. Port of `hyperlinkCount`.
    pub fn hyperlink_count(&self) -> usize {
        // SAFETY: map region valid.
        unsafe { self.hyperlink_map.map(self.mem).count() as usize }
    }

    /// Hyperlink map capacity. Port of `hyperlinkCapacity`.
    pub fn hyperlink_capacity(&self) -> usize {
        // SAFETY: map region valid.
        unsafe { self.hyperlink_map.map(self.mem).capacity() as usize }
    }

    /// Insert a hyperlink (URI + optional explicit id) into the page, returning
    /// its set ID. Port of `insertHyperlink`. Strings are NOT de-duped.
    ///
    /// The `uri` and (for explicit) `id` are borrowed byte slices.
    pub fn insert_hyperlink(
        &mut self,
        uri: &[u8],
        id: HyperlinkInsertId,
    ) -> Result<hyperlink::Id, InsertHyperlinkError> {
        // SAFETY: string_alloc/set regions valid.
        unsafe {
            let mem = self.mem;

            let uri_slice = self
                .string_alloc
                .alloc::<u8>(mem, uri.len())
                .map_err(|_| InsertHyperlinkError::StringsOutOfMemory)?;
            uri_slice.slice_mut(mem).copy_from_slice(uri);
            let page_uri = OffsetSlice {
                offset: get_offset(mem, &uri_slice.slice(mem)[0]),
                len: uri.len(),
            };

            let page_id = match id {
                HyperlinkInsertId::Implicit(v) => EntryId::Implicit(v),
                HyperlinkInsertId::Explicit(idbytes) => {
                    let idslice = match self.string_alloc.alloc::<u8>(mem, idbytes.len()) {
                        Ok(s) => s,
                        Err(_) => {
                            self.string_alloc.free(mem, uri_slice);
                            return Err(InsertHyperlinkError::StringsOutOfMemory);
                        }
                    };
                    idslice.slice_mut(mem).copy_from_slice(idbytes);
                    EntryId::Explicit(OffsetSlice {
                        offset: get_offset(mem, &idslice.slice(mem)[0]),
                        len: idbytes.len(),
                    })
                }
            };

            let entry = PageEntry {
                id: page_id,
                uri: page_uri,
            };

            self.bind_hyperlink_ctx();
            let alloc: *mut StringAlloc = &raw mut self.string_alloc;
            match self.hyperlink_set.add(mem, entry) {
                Ok(id) => Ok(id),
                Err(AddError::OutOfMemory) => {
                    // Free the strings we allocated.
                    entry.free(mem, alloc);
                    Err(InsertHyperlinkError::SetOutOfMemory)
                }
                Err(AddError::NeedsRehash) => {
                    entry.free(mem, alloc);
                    Err(InsertHyperlinkError::SetNeedsRehash)
                }
            }
        }
    }

    /// Rebind the hyperlink set's context to this page (same source and dest).
    ///
    /// The context holds the page's memory base and a pointer to its
    /// `string_alloc` *field* — both disjoint from `hyperlink_set` — so the set
    /// can be `&mut`-borrowed for the operation without invalidating them under
    /// Stacked Borrows (a self-aliasing `*mut Page` would be invalidated).
    fn bind_hyperlink_ctx(&mut self) {
        let base = self.mem;
        let alloc: *mut StringAlloc = &raw mut self.string_alloc;
        let ctx = self.hyperlink_set.context_mut();
        ctx.dst_base = base;
        ctx.dst_alloc = alloc;
        ctx.src_base = base;
    }

    /// Rebind the hyperlink context for a cross-page clone: destination is this
    /// page; probe values live against `src_base`.
    fn bind_hyperlink_ctx_src(&mut self, src_base: *const u8) {
        let base = self.mem;
        let alloc: *mut StringAlloc = &raw mut self.string_alloc;
        let ctx = self.hyperlink_set.context_mut();
        ctx.dst_base = base;
        ctx.dst_alloc = alloc;
        ctx.src_base = src_base;
    }

    // ---- moveCells / swapCells (page.zig:1066-1188) ----

    /// Move `len` cells from `src_row[src_left..]` to `dst_row[dst_left..]`,
    /// blanking the source. Port of `moveCells`.
    ///
    /// # Safety
    ///
    /// Rows valid for this page; ranges in bounds.
    pub unsafe fn move_cells(
        &mut self,
        src_row: *mut Row,
        src_left: usize,
        dst_row: *mut Row,
        dst_left: usize,
        len: usize,
    ) {
        // SAFETY: rows valid per caller.
        unsafe {
            let mem = self.mem;
            self.clear_cells(dst_row, dst_left, dst_left + len);

            let src_base = (*src_row).cells().ptr(mem);
            let dst_base = (*dst_row).cells().ptr(mem);

            if !(*src_row).managed_memory() {
                for i in 0..len {
                    dst_base
                        .add(dst_left + i)
                        .write(*src_base.add(src_left + i));
                }
            } else {
                for i in 0..len {
                    let src = src_base.add(src_left + i);
                    let dst = dst_base.add(dst_left + i);
                    *dst = *src;
                    if (*src).has_grapheme() {
                        (*dst).set_content_tag(ContentTag::Codepoint);
                        self.move_grapheme(src, dst);
                        (*src).set_content_tag(ContentTag::Codepoint);
                        (*dst).set_content_tag(ContentTag::CodepointGrapheme);
                        (*dst_row).set_grapheme(true);
                    }
                    if (*src).hyperlink() {
                        (*dst).set_hyperlink(false);
                        self.move_hyperlink(src, dst);
                        (*dst).set_hyperlink(true);
                        (*dst_row).set_hyperlink(true);
                    }
                    if (*src).codepoint() == KITTY_PLACEHOLDER {
                        (*dst_row).set_kitty_virtual_placeholder(true);
                    }
                }
            }

            // dst styled if any moved cell is styled.
            if !(*dst_row).styled() {
                let mut styled = false;
                for i in 0..len {
                    if (*dst_base.add(dst_left + i)).style_id() != style::DEFAULT_ID {
                        styled = true;
                        break;
                    }
                }
                (*dst_row).set_styled(styled);
            }

            // Zero source cells directly (do NOT clearCells: refs were moved).
            for i in 0..len {
                src_base.add(src_left + i).write(Cell(0));
            }
            if len == self.size.cols as usize {
                (*src_row).set_grapheme(false);
                (*src_row).set_hyperlink(false);
                (*src_row).set_styled(false);
                (*src_row).set_kitty_virtual_placeholder(false);
            }
        }
        self.assert_integrity();
    }

    /// Swap two cells within the same row as quickly as possible. Port of
    /// `swapCells`.
    ///
    /// # Safety
    ///
    /// `src`/`dst` must be valid cell pointers within this page.
    pub unsafe fn swap_cells(&mut self, src: *mut Cell, dst: *mut Cell) {
        // SAFETY: cells valid per caller.
        unsafe {
            let mem = self.mem;

            // Graphemes are keyed by cell offset so we do have to move them.
            // We do this first so that all our grapheme state is correct.
            if (*src).has_grapheme() || (*dst).has_grapheme() {
                if (*src).has_grapheme() && !(*dst).has_grapheme() {
                    self.move_grapheme(src, dst);
                } else if !(*src).has_grapheme() && (*dst).has_grapheme() {
                    self.move_grapheme(dst, src);
                } else {
                    // Both had graphemes, so we have to manually swap.
                    let src_offset = get_offset(mem, src);
                    let dst_offset = get_offset(mem, dst);
                    let mut map = self.grapheme_map.map(mem);
                    let src_value = map.get(&src_offset).unwrap();
                    let dst_value = map.get(&dst_offset).unwrap();
                    map.remove(&src_offset);
                    map.remove(&dst_offset);
                    map.put_assume_capacity_no_clobber(src_offset, dst_value);
                    map.put_assume_capacity_no_clobber(dst_offset, src_value);
                }
            }

            // Hyperlinks are keyed by cell offset.
            if (*src).hyperlink() || (*dst).hyperlink() {
                if (*src).hyperlink() && !(*dst).hyperlink() {
                    self.move_hyperlink(src, dst);
                } else if !(*src).hyperlink() && (*dst).hyperlink() {
                    self.move_hyperlink(dst, src);
                } else {
                    // Both had hyperlinks, so we have to manually swap.
                    let src_offset = get_offset(mem, src);
                    let dst_offset = get_offset(mem, dst);
                    let mut map = self.hyperlink_map.map(mem);
                    let src_value = map.get(&src_offset).unwrap();
                    let dst_value = map.get(&dst_offset).unwrap();
                    map.remove(&src_offset);
                    map.remove(&dst_offset);
                    map.put_assume_capacity_no_clobber(src_offset, dst_value);
                    map.put_assume_capacity_no_clobber(dst_offset, src_value);
                }
            }

            // Copy the metadata. Styles are keyed by ID and we're preserving the
            // exact ref count and row state here, so no style accounting needed.
            std::ptr::swap(src, dst);
        }
        self.assert_integrity();
    }

    /// Compute the minimal capacity that can hold rows `[y_start, y_end)`. Port
    /// of `exactRowCapacity`. Used by PageList compaction.
    ///
    /// # Safety
    ///
    /// `y_start < y_end <= size.rows`.
    pub unsafe fn exact_row_capacity(&self, y_start: usize, y_end: usize) -> Capacity {
        debug_assert!(y_start < y_end && y_end <= self.size.rows as usize);
        // SAFETY: row range in bounds per caller contract.
        unsafe {
            // Style and hyperlink IDs are both u16; reuse one bitset for both.
            let mut id_seen = vec![false; u16::MAX as usize + 1];
            let mut grapheme_bytes = 0usize;

            // Pass 1: unique styles + grapheme bytes.
            for y in y_start..y_end {
                let row = self.rows.ptr(self.mem).add(y);
                let base = (*row).cells().ptr(self.mem);
                for x in 0..self.size.cols as usize {
                    let cell = base.add(x);
                    if (*cell).style_id() != style::DEFAULT_ID {
                        id_seen[(*cell).style_id() as usize] = true;
                    }
                    if (*cell).has_grapheme()
                        && let Some(cps) = self.lookup_grapheme(cell)
                    {
                        let cps: &[u32] = &*cps;
                        grapheme_bytes += GraphemeAlloc::bytes_required::<u32>(cps.len());
                    }
                }
            }
            let unique_styles = id_seen.iter().filter(|&&b| b).count();
            let styles_cap = StyleSet::capacity_for_count(unique_styles);

            // Pass 2: unique hyperlinks + string bytes + hyperlink cells.
            for v in id_seen.iter_mut() {
                *v = false;
            }
            let mut hyperlink_cells = 0usize;
            let mut string_bytes = 0usize;
            for y in y_start..y_end {
                let row = self.rows.ptr(self.mem).add(y);
                let base = (*row).cells().ptr(self.mem);
                for x in 0..self.size.cols as usize {
                    let cell = base.add(x);
                    if (*cell).hyperlink() {
                        hyperlink_cells += 1;
                        if let Some(id) = self.lookup_hyperlink(cell)
                            && !id_seen[id as usize]
                        {
                            id_seen[id as usize] = true;
                            let entry = &*self.hyperlink_set_get(id);
                            string_bytes += StringAlloc::bytes_required::<u8>(entry.uri.len);
                            if let EntryId::Explicit(slice) = entry.id {
                                string_bytes += StringAlloc::bytes_required::<u8>(slice.len);
                            }
                        }
                    }
                }
            }
            let unique_links = id_seen.iter().filter(|&&b| b).count();
            let hyperlink_set_cap = HyperlinkSet::capacity_for_count(unique_links);
            let hyperlink_map_min = if hyperlink_cells == 0 {
                0
            } else {
                hyperlink_cells.div_ceil(HYPERLINK_CELL_MULTIPLIER)
            };
            let hyperlink_cap = hyperlink_set_cap.max(hyperlink_map_min);

            Capacity {
                cols: self.size.cols,
                rows: (y_end - y_start) as CellCountInt,
                styles: styles_cap as StyleCountInt,
                grapheme_bytes: grapheme_bytes as GraphemeBytesInt,
                hyperlink_bytes: (hyperlink_cap * hyperlink_item_size()) as HyperlinkCountInt,
                string_bytes: string_bytes as StringBytesInt,
            }
        }
    }

    // ---- cloneFrom family (page.zig:797-1027) ----

    /// Clone rows `[y_start, y_end)` of `other` into this page. Port of
    /// `cloneFrom`.
    ///
    /// # Safety
    ///
    /// `other` valid; row ranges fit this page.
    pub unsafe fn clone_from(
        &mut self,
        other: *const Page,
        y_start: usize,
        y_end: usize,
    ) -> Result<(), CloneFromError> {
        // SAFETY: other valid per caller.
        unsafe {
            debug_assert!(y_start <= y_end && y_end <= (*other).size.rows as usize);
            debug_assert!(y_end - y_start <= self.size.rows as usize);
            for (i, y) in (y_start..y_end).enumerate() {
                let src_row = (*other).rows.ptr((*other).mem).add(y);
                let dst_row = self.rows.ptr(self.mem).add(i);
                self.clone_partial_row_from(other, dst_row, src_row, 0, self.size.cols as usize)?;
            }
        }
        self.assert_integrity();
        Ok(())
    }

    /// Clone a full row from another page. Port of `cloneRowFrom`.
    ///
    /// # Safety
    ///
    /// Rows valid for their respective pages.
    pub unsafe fn clone_row_from(
        &mut self,
        other: *const Page,
        dst_row: *mut Row,
        src_row: *const Row,
    ) -> Result<(), CloneFromError> {
        // SAFETY: per caller.
        unsafe { self.clone_partial_row_from(other, dst_row, src_row, 0, self.size.cols as usize) }
    }

    /// Clone a (partial) row from `other`, re-homing managed memory. Port of
    /// `clonePartialRowFrom`.
    ///
    /// # Safety
    ///
    /// Rows valid for their respective pages; `x_start <= x_end_req`.
    pub unsafe fn clone_partial_row_from(
        &mut self,
        other: *const Page,
        dst_row: *mut Row,
        src_row: *const Row,
        x_start: usize,
        x_end_req: usize,
    ) -> Result<(), CloneFromError> {
        // SAFETY: rows valid per caller.
        unsafe {
            let same_page = std::ptr::eq(other, self as *const Page);
            let mem = self.mem;
            let other_mem = (*other).mem;

            let cell_len = (self.size.cols as usize).min((*other).size.cols as usize);
            let x_end = x_end_req.min(cell_len);
            debug_assert!(x_start <= x_end);

            let other_base = (*src_row).cells().ptr(other_mem);
            let dst_base = (*dst_row).cells().ptr(mem);

            if (*dst_row).managed_memory() {
                self.clear_cells(dst_row, x_start, x_end);
            }

            // Copy row metadata but preserve the dst cells offset (and, for
            // partial copies, dst's wrap/managed/dirty state).
            let dst_cells_off = (*dst_row).cells();
            let mut copy = *src_row;
            if (x_end - x_start) < self.size.cols as usize {
                copy.set_wrap((*dst_row).wrap());
                copy.set_wrap_continuation((*dst_row).wrap_continuation());
                copy.set_grapheme((*dst_row).grapheme());
                copy.set_hyperlink((*dst_row).hyperlink());
                copy.set_styled((*dst_row).styled());
                copy.set_dirty(copy.dirty() || (*dst_row).dirty());
            }
            copy.set_cells(dst_cells_off);
            *dst_row = copy;

            if !(*src_row).managed_memory() {
                for i in x_start..x_end {
                    dst_base.add(i).write(*other_base.add(i));
                }
            } else {
                for i in x_start..x_end {
                    let src_cell = other_base.add(i);
                    let dst_cell = dst_base.add(i);
                    *dst_cell = *src_cell;
                    // Reset managed markers so an early error can't trip checks.
                    (*dst_cell).set_hyperlink(false);
                    (*dst_cell).set_style_id(style::DEFAULT_ID);
                    if (*dst_cell).content_tag() == ContentTag::CodepointGrapheme {
                        (*dst_cell).set_content_tag(ContentTag::Codepoint);
                    }

                    if (*src_cell).has_grapheme() {
                        let cps = (*other).lookup_grapheme(src_cell).unwrap();
                        self.set_graphemes(dst_row, dst_cell, &*cps)?;
                    }

                    if (*src_cell).hyperlink() {
                        let id = (*other).lookup_hyperlink(src_cell).unwrap();
                        if same_page {
                            self.bind_hyperlink_ctx();
                            self.hyperlink_set.use_id(mem, id);
                            self.set_hyperlink(dst_row, dst_cell, id)
                                .map_err(|_| CloneFromError::HyperlinkMapOutOfMemory)?;
                        } else {
                            if self.hyperlink_count() >= self.hyperlink_capacity() {
                                return Err(CloneFromError::HyperlinkMapOutOfMemory);
                            }
                            let other_link = *(*other).hyperlink_set_get(id);
                            // Bind ctx with src_base = other for cross-page lookup.
                            self.bind_hyperlink_ctx_src(other_mem);
                            let dst_id =
                                if let Some(i) = self.hyperlink_set.lookup(mem, &other_link) {
                                    self.hyperlink_set.use_id(mem, i);
                                    i
                                } else {
                                    let dst_alloc: *mut StringAlloc = &raw mut self.string_alloc;
                                    let dst_link = other_link
                                        .dupe(other_mem, mem, dst_alloc)
                                        .map_err(|_| CloneFromError::StringAllocOutOfMemory)?;
                                    // Rebind ctx to this page (deleted callback etc).
                                    self.bind_hyperlink_ctx();
                                    match self.hyperlink_set.add_with_id(mem, dst_link, id) {
                                        Ok(Some(i)) => i,
                                        Ok(None) => id,
                                        Err(AddError::OutOfMemory) => {
                                            return Err(CloneFromError::HyperlinkSetOutOfMemory);
                                        }
                                        Err(AddError::NeedsRehash) => {
                                            return Err(CloneFromError::HyperlinkSetNeedsRehash);
                                        }
                                    }
                                };
                            self.set_hyperlink(dst_row, dst_cell, dst_id)
                                .map_err(|_| CloneFromError::HyperlinkMapOutOfMemory)?;
                        }
                    }

                    if (*src_cell).style_id() != style::DEFAULT_ID {
                        (*dst_row).set_styled(true);
                        if same_page {
                            (*dst_cell).set_style_id((*src_cell).style_id());
                            self.styles.use_id(mem, (*dst_cell).style_id());
                        } else {
                            let other_style = *(*other).styles_get((*src_cell).style_id());
                            let sid = match self.styles.add_with_id(
                                mem,
                                other_style,
                                (*src_cell).style_id(),
                            ) {
                                Ok(Some(i)) => i,
                                Ok(None) => (*src_cell).style_id(),
                                Err(AddError::OutOfMemory) => {
                                    return Err(CloneFromError::StyleSetOutOfMemory);
                                }
                                Err(AddError::NeedsRehash) => {
                                    return Err(CloneFromError::StyleSetNeedsRehash);
                                }
                            };
                            (*dst_cell).set_style_id(sid);
                        }
                    }

                    if (*src_cell).codepoint() == KITTY_PLACEHOLDER {
                        (*dst_row).set_kitty_virtual_placeholder(true);
                    }
                }
            }

            // Growing columns: clear a stale spacer_head on the old last col.
            if self.size.cols > (*other).size.cols {
                let last = dst_base.add((*other).size.cols as usize - 1);
                if (*last).wide() == Wide::SpacerHead {
                    (*last).set_wide(Wide::Narrow);
                }
            }
        }
        Ok(())
    }

    /// Read-only access to a hyperlink set value by ID (for clone paths).
    ///
    /// # Safety
    ///
    /// `id` must be a live entry in this page's hyperlink set.
    unsafe fn hyperlink_set_get(&self, id: hyperlink::Id) -> *const PageEntry {
        // SAFETY: id valid per caller.
        unsafe { self.hyperlink_set.get(self.mem, id) as *const PageEntry }
    }

    /// Read-only access to a style set value by ID (for clone paths).
    ///
    /// # Safety
    ///
    /// `id` must be a live entry in this page's style set.
    unsafe fn styles_get(&self, id: style::Id) -> *const Style {
        // SAFETY: id valid per caller.
        unsafe { self.styles.get(self.mem, id) as *const Style }
    }

    /// Resolve an interned style id to a pointer to its [`Style`] value, without
    /// changing the ref count. Read-only accessor for the snapshot/read-back
    /// path (`crate::snapshot`).
    ///
    /// # Safety
    /// `id` must be a live, non-default style id in this page (ref count > 0),
    /// i.e. exactly the `style_id()` of one of this page's cells.
    pub(crate) unsafe fn style_by_id(&self, id: style::Id) -> *const Style {
        // SAFETY: per caller contract.
        unsafe { self.styles_get(id) }
    }

    /// Copy the managed memory (grapheme / hyperlink / style) of a source cell
    /// into an already-basic-initialized destination cell, for reflow. Port of
    /// the managed-memory section of `PageList.zig` `ReflowCursor.writeCell`
    /// (`:1552-1802`).
    ///
    /// The destination cell must already have its unmanaged bits set and its
    /// managed markers cleared (content_tag=codepoint, hyperlink=false,
    /// style_id=default) — the caller does this. This copies only the managed
    /// side tables. On capacity exhaustion it returns the exhausted dimension so
    /// the caller can grow the page and retry the whole cell; it does NOT modify
    /// capacity itself. Partial managed writes on error are left in a valid state
    /// (the caller's page-grow reinit discards uncommitted managed memory).
    ///
    /// # Safety
    ///
    /// `src_page` valid; `src_cell` a cell of `src_page`; `dst_row`/`dst_cell`
    /// valid cells of `self`.
    pub unsafe fn reflow_copy_managed(
        &mut self,
        src_page: *const Page,
        src_cell: *const Cell,
        dst_row: *mut Row,
        dst_cell: *mut Cell,
    ) -> Result<(), ReflowManagedError> {
        // SAFETY: pointers valid per caller contract.
        unsafe {
            let mem = self.mem;
            let src_mem = (*src_page).mem;

            // Grapheme data.
            if (*src_cell).content_tag() == ContentTag::CodepointGrapheme {
                let cps = &*(*src_page).lookup_grapheme(src_cell).unwrap();

                if self.grapheme_count() >= self.grapheme_capacity() {
                    return Err(ReflowManagedError::GraphemeBytes);
                }
                // Probe that we can allocate the grapheme bytes.
                match self.grapheme_alloc.alloc::<u32>(mem, cps.len()) {
                    Ok(slice) => self.grapheme_alloc.free(mem, slice),
                    Err(_) => return Err(ReflowManagedError::GraphemeBytes),
                }
                // This must succeed now; on failure degrade to replacement char.
                if self.set_graphemes(dst_row, dst_cell, cps).is_err() {
                    (*dst_cell).set_content_tag(ContentTag::Codepoint);
                    (*dst_cell).set_codepoint(0xFFFD);
                }
            }

            // Hyperlink data.
            if (*src_cell).hyperlink() {
                let src_id = (*src_page).lookup_hyperlink(src_cell).unwrap();
                let src_link = *(*src_page).hyperlink_set_get(src_id);

                if self.hyperlink_count() >= self.hyperlink_capacity() {
                    return Err(ReflowManagedError::HyperlinkBytes);
                }

                let additional = src_link.uri.len
                    + match src_link.id {
                        EntryId::Explicit(s) => s.len,
                        EntryId::Implicit(_) => 0,
                    };
                match self.string_alloc.alloc::<u8>(mem, additional) {
                    Ok(slice) => self.string_alloc.free(mem, slice),
                    Err(_) => return Err(ReflowManagedError::StringBytes),
                }

                let dst_alloc: *mut StringAlloc = &raw mut self.string_alloc;
                let dst_link = match src_link.dupe(src_mem, mem, dst_alloc) {
                    Ok(l) => l,
                    Err(_) => return Err(ReflowManagedError::StringBytes),
                };

                self.bind_hyperlink_ctx();
                let dst_id = match self.hyperlink_set.add_with_id(mem, dst_link, src_id) {
                    Ok(Some(i)) => i,
                    Ok(None) => src_id,
                    Err(AddError::OutOfMemory) => {
                        dst_link.free(mem, dst_alloc);
                        return Err(ReflowManagedError::HyperlinkBytes);
                    }
                    Err(AddError::NeedsRehash) => {
                        dst_link.free(mem, dst_alloc);
                        return Err(ReflowManagedError::Rehash);
                    }
                };

                if self.set_hyperlink(dst_row, dst_cell, dst_id).is_err() {
                    self.bind_hyperlink_ctx();
                    self.hyperlink_set.release(mem, dst_id);
                    (*dst_cell).set_hyperlink(false);
                }
            }

            // Style data.
            if (*src_cell).has_styling() {
                let style = *(*src_page).styles_get((*src_cell).style_id());
                let id = match self.styles.add_with_id(mem, style, (*src_cell).style_id()) {
                    Ok(Some(i)) => i,
                    Ok(None) => (*src_cell).style_id(),
                    Err(AddError::OutOfMemory) => return Err(ReflowManagedError::Styles),
                    Err(AddError::NeedsRehash) => return Err(ReflowManagedError::Rehash),
                };
                (*dst_row).set_styled(true);
                (*dst_cell).set_style_id(id);
            }
        }
        Ok(())
    }
}

/// The kitty unicode placeholder codepoint (U+10EEEE). Always tracked even when
/// the kitty feature is off, matching upstream (the row flag bit exists either
/// way).
const KITTY_PLACEHOLDER: u32 = 0x10EEEE;

/// The ID form for [`Page::insert_hyperlink`].
pub enum HyperlinkInsertId<'a> {
    Explicit(&'a [u8]),
    Implicit(OffsetInt),
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of page.zig "Page.layout can take a maxed capacity".
    #[test]
    fn layout_maxed_capacity() {
        let cap = Capacity {
            cols: 50000,
            rows: 50000,
            styles: u16::MAX,
            grapheme_bytes: GraphemeBytesInt::MAX,
            hyperlink_bytes: HyperlinkCountInt::MAX,
            string_bytes: StringBytesInt::MAX,
        };
        let l = Layout::compute(cap);
        assert!(l.total_size > 0);
        assert_eq!(l.total_size % PAGE_SIZE_MIN, 0);
    }

    // Port of page.zig "Cell is zero by default".
    #[test]
    fn cell_zero_default() {
        let c = Cell::default();
        assert!(c.is_zero());
        assert_eq!(c.cval(), 0);
    }

    #[test]
    fn row_cell_sizes() {
        assert_eq!(size_of::<Row>(), 8);
        assert_eq!(size_of::<Cell>(), 8);
        assert_eq!(size_of::<Row>() % align_of::<Cell>(), 0);
    }

    // Cell bitfield round-trips.
    #[test]
    fn cell_bitfields() {
        let mut c = Cell::init(0x41);
        assert_eq!(c.codepoint(), 0x41);
        assert_eq!(c.content_tag(), ContentTag::Codepoint);
        c.set_style_id(1234);
        assert_eq!(c.style_id(), 1234);
        c.set_wide(Wide::Wide);
        assert_eq!(c.wide(), Wide::Wide);
        assert_eq!(c.grid_width(), 2);
        c.set_hyperlink(true);
        assert!(c.hyperlink());
        c.set_protected(true);
        assert!(c.protected());
        // Codepoint and style survive the other sets.
        assert_eq!(c.codepoint(), 0x41);
        assert_eq!(c.style_id(), 1234);
    }

    // Row bitfield round-trips.
    #[test]
    fn row_bitfields() {
        let mut r = Row::new_with_cells(Offset::new(0x1234));
        assert_eq!(r.cells().get(), 0x1234);
        r.set_wrap(true);
        r.set_grapheme(true);
        r.set_dirty(true);
        r.set_semantic_prompt(SemanticPrompt::PromptContinuation);
        assert!(r.wrap());
        assert!(r.grapheme());
        assert!(r.dirty());
        assert_eq!(r.semantic_prompt(), SemanticPrompt::PromptContinuation);
        assert_eq!(r.cells().get(), 0x1234);
        assert!(r.managed_memory());
    }

    // Port of page.zig "Page init".
    #[test]
    fn page_init() {
        let page = Page::init(Capacity::new(120, 80));
        assert_eq!(page.size.cols, 120);
        assert_eq!(page.size.rows, 80);
    }

    // Port of page.zig "Page read and write cells".
    #[test]
    fn read_write_cells() {
        let page = Page::init(Capacity::new(10, 10));
        // SAFETY: coords in bounds.
        unsafe {
            for y in 0..page.size.rows as usize {
                let (_row, cell) = page.get_row_and_cell(0, y);
                *cell = Cell::init(b'A' as u32 + y as u32);
            }
            for y in 0..page.size.rows as usize {
                let (_row, cell) = page.get_row_and_cell(0, y);
                assert_eq!((*cell).codepoint(), b'A' as u32 + y as u32);
            }
        }
    }

    // Port of page.zig "Page appendGrapheme small".
    #[test]
    fn append_grapheme_small() {
        let mut page = Page::init(Capacity::new(10, 10));
        // SAFETY: coords in bounds.
        unsafe {
            let (row, cell) = page.get_row_and_cell(0, 0);
            *cell = Cell::init(0x09);
            page.append_grapheme(row, cell, 0x0A).unwrap();
            page.append_grapheme(row, cell, 0x0B).unwrap();
            let cps = &*page.lookup_grapheme(cell).unwrap();
            assert_eq!(cps, &[0x0A, 0x0B]);
            assert_eq!((*cell).codepoint(), 0x09);
            assert!((*row).grapheme());
        }
    }

    // Port of page.zig "Page appendGrapheme larger than chunk".
    #[test]
    fn append_grapheme_larger_than_chunk() {
        let mut page = Page::init(Capacity::new(10, 10));
        // SAFETY: coords in bounds.
        unsafe {
            let (row, cell) = page.get_row_and_cell(0, 0);
            *cell = Cell::init(0x09);
            for i in 0..30u32 {
                page.append_grapheme(row, cell, 0x0A + i).unwrap();
            }
            let cps = &*page.lookup_grapheme(cell).unwrap();
            assert_eq!(cps.len(), 30);
            for (i, &cp) in cps.iter().enumerate() {
                assert_eq!(cp, 0x0A + i as u32);
            }
        }
    }

    // Port of page.zig "Page clearGrapheme not all cells".
    #[test]
    fn clear_grapheme_not_all_cells() {
        let mut page = Page::init(Capacity::new(10, 10));
        // SAFETY: coords in bounds.
        unsafe {
            let (row, cell0) = page.get_row_and_cell(0, 0);
            *cell0 = Cell::init(0x09);
            page.append_grapheme(row, cell0, 0x0A).unwrap();
            let (_row, cell1) = page.get_row_and_cell(1, 0);
            *cell1 = Cell::init(0x09);
            page.append_grapheme(row, cell1, 0x0A).unwrap();

            page.clear_grapheme(cell0);
            assert!(!(*cell0).has_grapheme());
            assert!((*cell1).has_grapheme());
            assert!((*row).grapheme());
        }
    }

    // Port of page.zig capacity adjust tests (consolidated).
    #[test]
    fn capacity_adjust_cols() {
        let cap = Capacity::std();
        // Down.
        let down = cap.adjust_cols(cap.cols / 2).unwrap();
        assert!(down.rows > cap.rows);
        assert_eq!(down.cols, cap.cols / 2);
        assert_eq!(
            Layout::compute(cap).total_size,
            Layout::compute(down).total_size
        );
        // Down to 1.
        let one = cap.adjust_cols(1).unwrap();
        assert_eq!(one.cols, 1);
        assert!(one.rows > cap.rows);
        // Up.
        let up = cap.adjust_cols(cap.cols * 2).unwrap();
        assert!(up.rows < cap.rows);
        assert_eq!(up.cols, cap.cols * 2);
        assert_eq!(
            Layout::compute(cap).total_size,
            Layout::compute(up).total_size
        );
        // Too high.
        assert_eq!(cap.adjust_cols(u16::MAX), Err(OutOfMemory));
    }

    // Port of page.zig "Capacity maxCols" tests.
    #[test]
    fn capacity_max_cols() {
        let cap = Capacity::std();
        let max = cap.max_cols().unwrap();
        assert!(max > 0);
        // maxCols preserves total size.
        let adjusted = cap.adjust_cols(max).unwrap();
        assert_eq!(
            Layout::compute(cap).total_size,
            Layout::compute(adjusted).total_size
        );
        // At least one row fits.
        assert!(adjusted.rows >= 1);
    }

    // Style set integration through a live page.
    #[test]
    fn page_styles_dedup() {
        let mut page = Page::init(Capacity::new(10, 10));
        let mem = page.memory_mut();
        let s1 = Style {
            flags: style::Flags {
                bold: true,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: page base valid.
        unsafe {
            let id_a = page.styles().add(mem, s1).unwrap();
            let id_b = page.styles().add(mem, s1).unwrap();
            assert_eq!(id_a, id_b);
            assert_eq!(page.styles().ref_count(mem, id_a), 2);
        }
    }

    // Hyperlink insert + set + lookup round-trip.
    #[test]
    fn page_hyperlink_roundtrip() {
        let mut page = Page::init(Capacity::new(10, 10));
        let id = page
            .insert_hyperlink(b"https://example.com", HyperlinkInsertId::Implicit(1))
            .unwrap();
        // SAFETY: coords in bounds; id fresh.
        unsafe {
            let (row, cell) = page.get_row_and_cell(0, 0);
            *cell = Cell::init(b'x' as u32);
            page.set_hyperlink(row, cell, id).unwrap();
            assert!((*cell).hyperlink());
            assert!((*row).hyperlink());
            assert_eq!(page.lookup_hyperlink(cell), Some(id));
            assert_eq!(page.hyperlink_count(), 1);
            page.clear_hyperlink(cell);
            assert!(!(*cell).hyperlink());
            assert_eq!(page.lookup_hyperlink(cell), None);
        }
    }

    // Port of page.zig "Page cloneFrom" (basic text clone).
    #[test]
    fn clone_from_basic() {
        let src = Page::init(Capacity::new(10, 10));
        // SAFETY: coords in bounds.
        unsafe {
            for y in 0..src.size.rows as usize {
                let (_row, cell) = src.get_row_and_cell(0, y);
                *cell = Cell::init(b'A' as u32 + y as u32);
            }
        }
        let mut dst = Page::init(Capacity::new(10, 10));
        // SAFETY: src valid, ranges fit.
        unsafe {
            dst.clone_from(&src, 0, src.size.rows as usize).unwrap();
            for y in 0..dst.size.rows as usize {
                let (_row, cell) = dst.get_row_and_cell(0, y);
                assert_eq!((*cell).codepoint(), b'A' as u32 + y as u32);
            }
        }
    }

    // Port of page.zig "Page cloneFrom graphemes".
    #[test]
    fn clone_from_graphemes() {
        let mut src = Page::init(Capacity::new(10, 10));
        // SAFETY: coords in bounds.
        unsafe {
            for y in 0..src.size.rows as usize {
                let (row, cell) = src.get_row_and_cell(0, y);
                *cell = Cell::init(0x09);
                src.append_grapheme(row, cell, 0x0A + y as u32).unwrap();
            }
        }
        let mut dst = Page::init(Capacity::new(10, 10));
        // SAFETY: src valid, ranges fit.
        unsafe {
            dst.clone_from(&src, 0, src.size.rows as usize).unwrap();
            for y in 0..dst.size.rows as usize {
                let (_row, cell) = dst.get_row_and_cell(0, y);
                assert_eq!((*cell).codepoint(), 0x09);
                let cps = &*dst.lookup_grapheme(cell).unwrap();
                assert_eq!(cps, &[0x0A + y as u32]);
            }
        }
    }

    // Port of page.zig "Page cloneFrom styles".
    #[test]
    fn clone_from_styles() {
        let mut src = Page::init(Capacity::new(10, 10));
        let src_mem = src.memory_mut();
        // SAFETY: base valid, coords in bounds.
        unsafe {
            let style = Style {
                flags: style::Flags {
                    bold: true,
                    ..Default::default()
                },
                ..Default::default()
            };
            let sid = src.styles().add(src_mem, style).unwrap();
            for y in 0..src.size.rows as usize {
                let (row, cell) = src.get_row_and_cell(0, y);
                *cell = Cell::init(b'A' as u32);
                (*cell).set_style_id(sid);
                (*row).set_styled(true);
                if y > 0 {
                    src.styles().use_id(src_mem, sid);
                }
            }
        }
        let mut dst = Page::init(Capacity::new(10, 10));
        // SAFETY: src valid, ranges fit.
        unsafe {
            dst.clone_from(&src, 0, src.size.rows as usize).unwrap();
            let dst_mem = dst.memory_mut();
            for y in 0..dst.size.rows as usize {
                let (_row, cell) = dst.get_row_and_cell(0, y);
                assert!((*cell).has_styling());
                let resolved = &*dst.styles().get(dst_mem, (*cell).style_id());
                assert!(resolved.flags.bold);
            }
        }
    }

    // Port of page.zig "Page moveCells text-only".
    #[test]
    fn move_cells_text_only() {
        let mut page = Page::init(Capacity::new(10, 10));
        // SAFETY: coords in bounds.
        unsafe {
            let row = page.get_row(0);
            let (_r, c0) = page.get_row_and_cell(0, 0);
            *c0 = Cell::init(b'A' as u32);
            page.move_cells(row, 0, row, 1, 1);
            let (_r, c0b) = page.get_row_and_cell(0, 0);
            let (_r, c1) = page.get_row_and_cell(1, 0);
            assert_eq!((*c0b).codepoint(), 0);
            assert_eq!((*c1).codepoint(), b'A' as u32);
        }
    }

    // Port of page.zig "Page moveCells graphemes".
    #[test]
    fn move_cells_graphemes() {
        let mut page = Page::init(Capacity::new(10, 10));
        // SAFETY: coords in bounds.
        unsafe {
            let row = page.get_row(0);
            let (r, c0) = page.get_row_and_cell(0, 0);
            *c0 = Cell::init(0x09);
            page.append_grapheme(r, c0, 0x0A).unwrap();
            page.move_cells(row, 0, row, 1, 1);
            let (_r, c0b) = page.get_row_and_cell(0, 0);
            let (_r, c1) = page.get_row_and_cell(1, 0);
            assert!(!(*c0b).has_grapheme());
            assert!((*c1).has_grapheme());
            let cps = &*page.lookup_grapheme(c1).unwrap();
            assert_eq!(cps, &[0x0A]);
        }
    }

    // reinit zeroes and rebuilds.
    #[test]
    fn reinit_clears() {
        let mut page = Page::init(Capacity::new(5, 5));
        // SAFETY: coords in bounds.
        unsafe {
            let (_r, c) = page.get_row_and_cell(0, 0);
            *c = Cell::init(b'Z' as u32);
        }
        page.reinit();
        // SAFETY: coords in bounds.
        unsafe {
            let (_r, c) = page.get_row_and_cell(0, 0);
            assert_eq!((*c).codepoint(), 0);
        }
    }

    // Port of page.zig "Page exactRowCapacity empty rows".
    #[test]
    fn exact_row_capacity_empty() {
        let page = Page::init(Capacity::new(10, 10));
        // SAFETY: range in bounds.
        let cap = unsafe { page.exact_row_capacity(0, page.size.rows as usize) };
        assert_eq!(cap.cols, 10);
        assert_eq!(cap.rows, 10);
        assert_eq!(cap.styles, 0);
        assert_eq!(cap.grapheme_bytes, 0);
    }

    // Port of page.zig "Page exactRowCapacity styles" (a used style is counted).
    #[test]
    fn exact_row_capacity_styles() {
        let mut page = Page::init(Capacity::new(10, 10));
        let mem = page.memory_mut();
        // SAFETY: base/coords valid.
        unsafe {
            let style = Style {
                flags: style::Flags {
                    bold: true,
                    ..Default::default()
                },
                ..Default::default()
            };
            let sid = page.styles().add(mem, style).unwrap();
            let (row, cell) = page.get_row_and_cell(0, 0);
            *cell = Cell::init(b'A' as u32);
            (*cell).set_style_id(sid);
            (*row).set_styled(true);
            let cap = page.exact_row_capacity(0, 1);
            // At least one style must be reserved.
            assert!(cap.styles >= 1);
            assert_eq!(cap.rows, 1);
        }
    }

    // Port of page.zig "Page exactRowCapacity grapheme_bytes".
    #[test]
    fn exact_row_capacity_grapheme_bytes() {
        let mut page = Page::init(Capacity::new(10, 10));
        // SAFETY: coords in bounds.
        unsafe {
            let (row, cell) = page.get_row_and_cell(0, 0);
            *cell = Cell::init(0x09);
            page.append_grapheme(row, cell, 0x0A).unwrap();
            let cap = page.exact_row_capacity(0, 1);
            assert!(cap.grapheme_bytes >= GRAPHEME_CHUNK as u32);
        }
    }

    // Port of page.zig "Page cloneFrom shrink columns".
    #[test]
    fn clone_from_shrink_columns() {
        let src = Page::init(Capacity::new(10, 5));
        // SAFETY: coords in bounds.
        unsafe {
            for x in 0..src.size.cols as usize {
                let (_r, cell) = src.get_row_and_cell(x, 0);
                *cell = Cell::init(b'A' as u32 + x as u32);
            }
        }
        // Destination has fewer columns; extra source columns are truncated.
        let mut dst = Page::init(Capacity::new(5, 5));
        // SAFETY: src valid, ranges fit.
        unsafe {
            dst.clone_from(&src, 0, 5).unwrap();
            for x in 0..dst.size.cols as usize {
                let (_r, cell) = dst.get_row_and_cell(x, 0);
                assert_eq!((*cell).codepoint(), b'A' as u32 + x as u32);
            }
        }
    }

    // Port of page.zig "Page cloneRowFrom partial".
    #[test]
    fn clone_partial_row() {
        let src = Page::init(Capacity::new(10, 5));
        // SAFETY: coords in bounds.
        unsafe {
            for x in 0..src.size.cols as usize {
                let (_r, cell) = src.get_row_and_cell(x, 0);
                *cell = Cell::init(b'A' as u32 + x as u32);
            }
        }
        let mut dst = Page::init(Capacity::new(10, 5));
        // SAFETY: src valid, ranges fit.
        unsafe {
            let src_row = src.get_row(0);
            let dst_row = dst.get_row(0);
            // Copy only columns [2, 5).
            dst.clone_partial_row_from(&src, dst_row, src_row, 2, 5)
                .unwrap();
            for x in 2..5usize {
                let (_r, cell) = dst.get_row_and_cell(x, 0);
                assert_eq!((*cell).codepoint(), b'A' as u32 + x as u32);
            }
            // Columns outside the range stay blank.
            let (_r, c0) = dst.get_row_and_cell(0, 0);
            assert_eq!((*c0).codepoint(), 0);
            let (_r, c5) = dst.get_row_and_cell(5, 0);
            assert_eq!((*c5).codepoint(), 0);
        }
    }

    // Cross-page hyperlink clone (dedup path).
    #[test]
    fn clone_from_hyperlinks() {
        let mut src = Page::init(Capacity::new(10, 5));
        let id = src
            .insert_hyperlink(b"https://a.example", HyperlinkInsertId::Implicit(1))
            .unwrap();
        // SAFETY: coords in bounds; id fresh.
        unsafe {
            let (row, cell) = src.get_row_and_cell(0, 0);
            *cell = Cell::init(b'x' as u32);
            src.set_hyperlink(row, cell, id).unwrap();
        }
        let mut dst = Page::init(Capacity::new(10, 5));
        // SAFETY: src valid, ranges fit.
        unsafe {
            dst.clone_from(&src, 0, 5).unwrap();
            let (_r, cell) = dst.get_row_and_cell(0, 0);
            assert!((*cell).hyperlink());
            let dst_id = dst.lookup_hyperlink(cell).unwrap();
            let entry = &*dst.hyperlink_set_get(dst_id);
            let uri = entry.uri.slice(dst.memory());
            assert_eq!(uri, b"https://a.example");
        }
    }
}
