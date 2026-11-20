//! The font an inline box of a paragraph is set in: a TrueType font misty
//! builds itself, with one empty glyph for U+FFFC OBJECT REPLACEMENT
//! CHARACTER, no advance, and an ascent of one em over a descent of none.
//!
//! cosmic-text lays out glyphs only, so an [`InlineBox`] is a glyph of
//! this font: set at a font size equal to the box's height above the
//! baseline, its ascent lifts the line exactly that far, it adds nothing
//! below the baseline, and letter spacing gives it the box's width. The
//! glyph has no outline, so nothing is drawn where it lands; the app
//! paints the box there itself.
//!
//! [`InlineBox`]: crate::InlineBox

// ========================================================================
// Constants
// ========================================================================

/// The family name the font is loaded under; nothing else uses it.
pub(crate) const FAMILY: &str = "Misty Inline Box";

/// The character an inline box stands for in a paragraph's text.
pub(crate) const CHAR: char = '\u{FFFC}';

/// The PostScript name of the font.
const POSTSCRIPT_NAME: &str = "MistyInlineBox";

/// The subfamily (style) name of the font.
const SUBFAMILY: &str = "Regular";

/// Font units per em: the ascent is exactly one em of them.
const UNITS_PER_EM: u16 = 1000;

/// How many glyphs the font has: `.notdef` and the box.
const GLYPH_COUNT: u16 = 2;

/// The glyph U+FFFC maps to.
const BOX_GLYPH: u16 = 1;

/// The sfnt version of a TrueType font.
const TRUETYPE: u32 = 0x0001_0000;

/// The magic number every `head` table holds.
const HEAD_MAGIC: u32 = 0x5F0F_3CF5;

/// `head` flags: the baseline at y 0 and the left side bearing at x 0.
const HEAD_FLAGS: u16 = 0x0003;

/// The smallest size in pixels per em the font is readable at.
const LOWEST_PPEM: u16 = 8;

/// `head`: glyphs are strongly left to right and contain neutrals.
const FONT_DIRECTION_HINT: i16 = 2;

/// `maxp`: the twilight zone and the glyph zone.
const MAX_ZONES: u16 = 2;

/// `OS/2` version 4, the last whose layout every parser reads.
const OS2_VERSION: u16 = 4;

/// `OS/2`: the regular weight.
const WEIGHT_REGULAR: u16 = 400;

/// `OS/2`: the normal width.
const WIDTH_NORMAL: u16 = 5;

/// `OS/2` fsSelection: regular, and the typographic metrics are the ones
/// to use.
const FS_SELECTION: u16 = 0x0040 | 0x0080;

/// The Windows platform, which the `cmap` and `name` records are for.
const PLATFORM_WINDOWS: u16 = 3;

/// The Windows platform's Unicode BMP encoding.
const ENCODING_UNICODE_BMP: u16 = 1;

/// US English, the language of the names.
const LANGUAGE_EN_US: u16 = 0x0409;

/// The `name` id of the family name.
const NAME_FAMILY: u16 = 1;

/// The `name` id of the subfamily (style) name.
const NAME_SUBFAMILY: u16 = 2;

/// The `name` id of the full name.
const NAME_FULL: u16 = 4;

/// The `name` id of the PostScript name.
const NAME_POSTSCRIPT: u16 = 6;

/// `post` version 3: no glyph names.
const POST_VERSION: u32 = 0x0003_0000;

/// `cmap` subtable format 4, segments of BMP characters.
const CMAP_FORMAT: u16 = 4;

/// The last character of the closing `cmap` segment every format 4 table
/// ends with.
const CMAP_END: u16 = 0xFFFF;

// ========================================================================
// Data Structures
// ========================================================================

/// Big-endian bytes of one font table, appended to.
#[derive(Default)]
struct TableBytes(Vec<u8>);

// ========================================================================
// TableBytes Implementations
// ========================================================================

impl TableBytes {
    /// Appends `value`.
    fn u16(&mut self, value: u16) -> &mut Self {
        self.0.extend_from_slice(&value.to_be_bytes());
        self
    }

    /// Appends `value`.
    fn i16(&mut self, value: i16) -> &mut Self {
        self.0.extend_from_slice(&value.to_be_bytes());
        self
    }

    /// Appends `value`.
    fn u32(&mut self, value: u32) -> &mut Self {
        self.0.extend_from_slice(&value.to_be_bytes());
        self
    }

    /// Appends `count` zero bytes.
    fn zeros(&mut self, count: usize) -> &mut Self {
        self.0.resize(self.0.len() + count, 0);
        self
    }

    /// The bytes.
    fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The bytes of the font, a complete TrueType file.
pub(crate) fn font_data() -> Vec<u8> {
    // Sorted by tag, as the table directory must be.
    let tables: [(&[u8; 4], Vec<u8>); 10] = [
        (b"OS/2", os2()),
        (b"cmap", cmap()),
        (b"glyf", Vec::new()),
        (b"head", head()),
        (b"hhea", hhea()),
        (b"hmtx", hmtx()),
        (b"loca", loca()),
        (b"maxp", maxp()),
        (b"name", name()),
        (b"post", post()),
    ];
    let count = tables.len() as u16;
    let entry_selector = 15 - count.leading_zeros() as u16;
    let search_range = 16 << entry_selector;
    let mut file = TableBytes::default();
    file.u32(TRUETYPE)
        .u16(count)
        .u16(search_range)
        .u16(entry_selector)
        .u16(count * 16 - search_range);
    let mut offset = 12 + 16 * tables.len();
    for (tag, data) in &tables {
        file.0.extend_from_slice(*tag);
        file.u32(checksum(data))
            .u32(offset as u32)
            .u32(data.len() as u32);
        offset += padded(data.len());
    }
    for (_, data) in &tables {
        file.0.extend_from_slice(data);
        file.zeros(padded(data.len()) - data.len());
    }
    file.into_bytes()
}

/// `len` rounded up to a whole number of 4-byte words, as tables are laid.
fn padded(len: usize) -> usize {
    len.div_ceil(4) * 4
}

/// The sum of `data` as big-endian 32-bit words, zero padded.
fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, chunk| {
        let mut word = [0; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

/// The `OS/2` table: weight, width, and the one-em ascent with no descent.
fn os2() -> Vec<u8> {
    let em = UNITS_PER_EM as i16;
    let mut table = TableBytes::default();
    table
        .u16(OS2_VERSION)
        .i16(0) // average advance
        .u16(WEIGHT_REGULAR)
        .u16(WIDTH_NORMAL)
        .u16(0) // embedding allowed
        .zeros(2 * 10) // subscript, superscript, strikeout, family class
        .zeros(10) // panose
        .zeros(16) // Unicode ranges
        .zeros(4) // vendor
        .u16(FS_SELECTION)
        .u16(CHAR as u16)
        .u16(CHAR as u16)
        .i16(em) // typographic ascender
        .i16(0) // typographic descender
        .i16(0) // typographic line gap
        .u16(UNITS_PER_EM) // Windows ascent
        .u16(0) // Windows descent
        .zeros(8) // code pages
        .zeros(2 * 5); // x height, cap height, default, break, context
    table.into_bytes()
}

/// The `cmap` table: U+FFFC to the box glyph, in one format 4 subtable.
fn cmap() -> Vec<u8> {
    let char = CHAR as u16;
    // Two segments, the box's and the closing one: 2 * 2^1 = 4.
    let (seg_count_x2, search_range, entry_selector, range_shift) = (4, 4, 1, 0);
    let mut sub = TableBytes::default();
    sub.u16(CMAP_FORMAT)
        .u16(32) // length
        .u16(0) // language
        .u16(seg_count_x2)
        .u16(search_range)
        .u16(entry_selector)
        .u16(range_shift)
        .u16(char) // end codes
        .u16(CMAP_END)
        .u16(0) // reserved
        .u16(char) // start codes
        .u16(CMAP_END)
        .u16(BOX_GLYPH.wrapping_sub(char)) // deltas
        .u16(1)
        .u16(0) // range offsets
        .u16(0);
    let mut table = TableBytes::default();
    table
        .u16(0) // version
        .u16(1) // subtables
        .u16(PLATFORM_WINDOWS)
        .u16(ENCODING_UNICODE_BMP)
        .u32(12); // the subtable's offset
    table.0.extend(sub.into_bytes());
    table.into_bytes()
}

/// The `head` table: units per em, short glyph offsets, no bounds.
fn head() -> Vec<u8> {
    let mut table = TableBytes::default();
    table
        .u32(TRUETYPE) // version
        .u32(TRUETYPE) // revision 1.0
        .u32(0) // checksum adjustment
        .u32(HEAD_MAGIC)
        .u16(HEAD_FLAGS)
        .u16(UNITS_PER_EM)
        .zeros(16) // created, modified
        .zeros(8) // bounds
        .u16(0) // mac style
        .u16(LOWEST_PPEM)
        .i16(FONT_DIRECTION_HINT)
        .i16(0) // short offsets in loca
        .i16(0); // glyph data format
    table.into_bytes()
}

/// The `hhea` table: the one-em ascent, no descent, every glyph's metrics.
fn hhea() -> Vec<u8> {
    let mut table = TableBytes::default();
    table
        .u32(TRUETYPE)
        .i16(UNITS_PER_EM as i16) // ascender
        .i16(0) // descender
        .i16(0) // line gap
        .zeros(2 * 4) // advance max, side bearings, extent
        .i16(1) // caret slope rise
        .zeros(2 * 2) // caret slope run, offset
        .zeros(2 * 4) // reserved
        .i16(0) // metric data format
        .u16(GLYPH_COUNT);
    table.into_bytes()
}

/// The `hmtx` table: no glyph advances.
fn hmtx() -> Vec<u8> {
    let mut table = TableBytes::default();
    table.zeros(4 * usize::from(GLYPH_COUNT));
    table.into_bytes()
}

/// The `loca` table: every glyph empty.
fn loca() -> Vec<u8> {
    let mut table = TableBytes::default();
    table.zeros(2 * (usize::from(GLYPH_COUNT) + 1));
    table.into_bytes()
}

/// The `maxp` table, version 1.0 as TrueType outlines want.
fn maxp() -> Vec<u8> {
    let mut table = TableBytes::default();
    table
        .u32(TRUETYPE)
        .u16(GLYPH_COUNT)
        .zeros(2 * 4) // points, contours, composite points and contours
        .u16(MAX_ZONES)
        .zeros(2 * 8); // twilight points through component depth
    table.into_bytes()
}

/// The `name` table: the family, style, full, and PostScript names.
fn name() -> Vec<u8> {
    let full = format!("{FAMILY} {SUBFAMILY}");
    let names = [
        (NAME_FAMILY, FAMILY),
        (NAME_SUBFAMILY, SUBFAMILY),
        (NAME_FULL, full.as_str()),
        (NAME_POSTSCRIPT, POSTSCRIPT_NAME),
    ];
    let mut strings = TableBytes::default();
    let mut table = TableBytes::default();
    table
        .u16(0) // format
        .u16(names.len() as u16)
        .u16(6 + 12 * names.len() as u16);
    for (id, text) in names {
        let start = strings.0.len() as u16;
        for unit in text.encode_utf16() {
            strings.u16(unit);
        }
        table
            .u16(PLATFORM_WINDOWS)
            .u16(ENCODING_UNICODE_BMP)
            .u16(LANGUAGE_EN_US)
            .u16(id)
            .u16(strings.0.len() as u16 - start)
            .u16(start);
    }
    table.0.extend(strings.into_bytes());
    table.into_bytes()
}

/// The `post` table: proportional, no glyph names.
fn post() -> Vec<u8> {
    let mut table = TableBytes::default();
    table.u32(POST_VERSION).zeros(4 + 2 + 2 + 4 + 4 * 4);
    table.into_bytes()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glyphon::fontdb::{Database, Source};

    use super::*;

    #[test]
    fn test_the_font_loads_under_its_family_in_the_regular_face() {
        let mut db = Database::new();
        db.load_font_source(Source::Binary(Arc::new(font_data())));
        let faces: Vec<_> = db.faces().collect();
        assert_eq!(faces.len(), 1, "one face");
        assert_eq!(faces[0].families[0].0, FAMILY);
        assert_eq!(faces[0].post_script_name, POSTSCRIPT_NAME);
        assert!(!faces[0].monospaced);
    }
}
