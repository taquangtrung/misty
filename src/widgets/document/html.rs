//! A safe subset of HTML into a [`Document`], behind the `html` feature:
//! the tags dictionary entries use, read leniently (unclosed and stray
//! tags are fine) by the html5gum tokenizer. Nothing runs and nothing is
//! fetched: scripts, styles, and forms are left out, and an image's bytes
//! come from the app's [`HtmlOptions::image`].
//!
//! Read: `p`, `div` (and `section`, `article`, `header`, `footer`, `main`,
//! `aside`, `nav`, `figure`, `figcaption`, `dl`, `dt`, `dd`), `span`,
//! `b`/`strong`, `i`/`em`, `u`, `s`/`strike`/`del`, `sub`/`sup`/`small`
//! (drawn smaller, in line), `code`/`kbd`/`tt`, `br`, `h1` to `h6`,
//! `ul`/`ol`/`li`, `blockquote`, `pre`, `table`/`thead`/`tr`/`td`/`th`,
//! `a href`, `img src alt`, and `hr`. Any other tag is dropped and its
//! text kept. On every element, the inline `style` properties `color`,
//! `background-color` (as `#rrggbb`), `font-size` (`em`, `%`, `px`),
//! `font-weight`, `font-style`, `text-decoration`, and `display: none`
//! apply, and so do the app's [`ClassStyle`] for each of its classes; an
//! attribute named in [`HtmlOptions::data_attributes`] makes the
//! element's text a [`Link::Data`].
//!
//! Map a dictionary's CSS onto [`HtmlOptions::classes`]: a class that
//! colors text gets a [`ClassStyle::color`] theme role, one that frames a
//! section a [`ClassStyle::border`] or [`ClassStyle::background`] (on a
//! block element it becomes a [`Container`](super::Container)), one that
//! hides a part [`ClassStyle::hidden`].

use std::collections::HashMap;

use html5gum::{Token, Tokenizer};

use super::super::super::base::color::Rgba;
use super::super::super::base::styled::TextColor;
use super::super::super::base::theme::ThemeToken;
use super::super::super::render::image::Image;
use super::build::{Builder, InlineStyle, OpenKind, SMALL_SCALE};
use super::{Block, BlockImage, Container, DataLink, Document, Link};

// ========================================================================
// Constants
// ========================================================================

/// The font size `font-size` in pixels is measured against.
const BASE_FONT_PIXELS: f32 = 16.0;

/// The share of 100 a `font-size` percentage stands for.
const PERCENT: f32 = 100.0;

/// The lowest numeric `font-weight` drawn bold.
const BOLD_WEIGHT: u32 = 600;

/// Tags whose content is left out entirely.
const SKIPPED: &[&str] = &["head", "noscript", "script", "style", "template", "title"];

/// Tags that hold blocks, ending the paragraph before and after them.
const BLOCK_TAGS: &[&str] = &[
    "article",
    "aside",
    "dd",
    "div",
    "dl",
    "dt",
    "figcaption",
    "figure",
    "footer",
    "header",
    "main",
    "nav",
    "p",
    "section",
];

// ========================================================================
// Data Structures
// ========================================================================

/// What finds an image's bytes from its `src`: `None` leaves it out.
pub type ImageSource<'a> = dyn FnMut(&str) -> Option<Image> + 'a;

/// How an app's HTML is read into a [`Document`].
pub struct HtmlOptions<'a> {
    /// The style of each class name.
    pub classes: HashMap<String, ClassStyle>,
    /// The attributes (`data-lookup`, `data-audio`) that make an element's
    /// text a data link reporting the attribute's value.
    pub data_attributes: Vec<String>,
    /// The image `src` names, or `None` to leave it out.
    pub image: Box<ImageSource<'a>>,
}

/// What a class does to an element: its text's style, and for a block
/// element its box. Every field left at its default changes nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClassStyle {
    /// The fill behind the text, or behind a block element's box.
    pub background: Option<ThemeToken>,
    /// Whether the text is bold.
    pub bold: bool,
    /// The line around a block element's box.
    pub border: Option<ThemeToken>,
    /// The text's color.
    pub color: Option<ThemeToken>,
    /// Whether the element is left out.
    pub hidden: bool,
    /// How many indent steps a block element is set in.
    pub indent: u8,
    /// Whether the text is italic.
    pub italic: bool,
    /// The text's size, as a multiple of the size outside.
    pub scale: Option<f32>,
}

/// An element open while its content is read.
#[derive(Debug)]
struct Element {
    /// What closing it ends.
    closes: Closes,
    /// Its tag name, lower case.
    name: String,
    /// Whether it pushed an inline style, popped on closing.
    styled: bool,
}

/// What closing an element ends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Closes {
    /// A table cell.
    Cell,
    /// A box of blocks.
    Container,
    /// A heading.
    Heading,
    /// A list item.
    Item,
    /// A list.
    List,
    /// Nothing but its style.
    Nothing,
    /// A paragraph.
    Paragraph,
    /// Preformatted text.
    Pre,
    /// A block quote.
    Quote,
    /// A table row.
    Row,
    /// Content left out.
    Skip,
    /// A table.
    Table,
}

/// A document being read from HTML.
struct Reader<'o, 'a> {
    /// The document so far.
    builder: Builder,
    /// The elements open now.
    elements: Vec<Element>,
    /// Whether the rows read now are a table's head.
    in_head: bool,
    /// How to read it.
    options: &'o mut HtmlOptions<'a>,
    /// The text of the preformatted block being read.
    pre: Option<String>,
}

// ========================================================================
// Document Implementations
// ========================================================================

impl Document {
    /// The document the HTML `html` describes, read as `options` says.
    pub fn from_html(html: &str, options: &mut HtmlOptions<'_>) -> Self {
        let mut reader = Reader {
            builder: Builder::new(),
            elements: Vec::new(),
            in_head: false,
            options,
            pre: None,
        };
        for Ok(token) in Tokenizer::new(html) {
            match token {
                Token::StartTag(tag) => {
                    let name = String::from_utf8_lossy(&tag.name).to_lowercase();
                    let attributes: HashMap<String, String> = tag
                        .attributes
                        .iter()
                        .map(|(key, value)| {
                            (
                                String::from_utf8_lossy(key).to_lowercase(),
                                String::from_utf8_lossy(value).into_owned(),
                            )
                        })
                        .collect();
                    reader.start(&name, &attributes, tag.self_closing);
                }
                Token::EndTag(tag) => {
                    reader.end(&String::from_utf8_lossy(&tag.name).to_lowercase())
                }
                Token::String(text) => reader.text(&String::from_utf8_lossy(&text)),
                Token::Comment(_) | Token::Doctype(_) | Token::Error(_) => {}
            }
        }
        while let Some(element) = reader.elements.pop() {
            reader.close(element);
        }
        reader.builder.finish()
    }
}

// ========================================================================
// HtmlOptions Implementations
// ========================================================================

impl Default for HtmlOptions<'_> {
    /// No classes, no data attributes, and every image left out.
    fn default() -> Self {
        Self {
            classes: HashMap::new(),
            data_attributes: Vec::new(),
            image: Box::new(|_| None),
        }
    }
}

// ========================================================================
// Reader Implementations
// ========================================================================

impl Reader<'_, '_> {
    /// Whether content is being left out.
    fn is_skipping(&self) -> bool {
        self.elements
            .iter()
            .any(|element| element.closes == Closes::Skip)
    }

    /// Reads the start of a `name` element with `attributes`.
    fn start(&mut self, name: &str, attributes: &HashMap<String, String>, self_closing: bool) {
        if self.is_skipping() {
            if SKIPPED.contains(&name) && !self_closing {
                self.elements.push(Element {
                    closes: Closes::Skip,
                    name: name.to_owned(),
                    styled: false,
                });
            }
            return;
        }
        self.close_implied(name);
        let classes = self.classes(attributes);
        match name {
            "br" => return self.text_raw("\n"),
            "hr" => return self.builder.push(Block::Rule),
            "img" => {
                let alt = attributes.get("alt").cloned().unwrap_or_default();
                let found = attributes
                    .get("src")
                    .and_then(|src| (self.options.image)(src));
                match found {
                    Some(image) => self.builder.push(Block::Image(BlockImage { alt, image })),
                    None => self.text(&alt),
                }
                return;
            }
            _ if self_closing => return,
            _ => {}
        }
        let closes = match name {
            _ if SKIPPED.contains(&name) => Closes::Skip,
            _ if self.pre.is_some() => Closes::Nothing,
            "blockquote" => {
                self.builder.open(OpenKind::Quote);
                Closes::Quote
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = name[1..].parse().unwrap_or(1);
                self.builder
                    .start_heading(level, attributes.get("id").cloned());
                Closes::Heading
            }
            "li" => {
                self.builder.open(OpenKind::Item);
                Closes::Item
            }
            "ol" => {
                let start = attributes.get("start").and_then(|start| start.parse().ok());
                self.builder.open(OpenKind::List(Some(start.unwrap_or(1))));
                Closes::List
            }
            "pre" => {
                self.builder.flush();
                self.pre = Some(String::new());
                Closes::Pre
            }
            "table" => {
                self.builder.open(OpenKind::Table);
                Closes::Table
            }
            "td" | "th" => {
                self.builder.open(OpenKind::Cell);
                Closes::Cell
            }
            "thead" => {
                self.in_head = true;
                Closes::Nothing
            }
            "tr" => {
                self.builder.open(OpenKind::Row(self.in_head));
                Closes::Row
            }
            "ul" => {
                self.builder.open(OpenKind::List(None));
                Closes::List
            }
            _ if BLOCK_TAGS.contains(&name) => match container_of(&classes, attributes.get("id")) {
                Some(container) => {
                    self.builder.open(OpenKind::Container(container));
                    Closes::Container
                }
                None => {
                    self.builder.flush();
                    Closes::Paragraph
                }
            },
            _ => Closes::Nothing,
        };
        let style = self.style_of(name, attributes, &classes);
        let styled = style != InlineStyle::default();
        if styled {
            self.builder.push_style(style);
        }
        self.elements.push(Element {
            closes,
            name: name.to_owned(),
            styled,
        });
    }

    /// Reads the end of a `name` element: closes the innermost open one,
    /// and every element opened inside it; a stray end is ignored.
    fn end(&mut self, name: &str) {
        let Some(index) = self
            .elements
            .iter()
            .rposition(|element| element.name == name)
        else {
            return;
        };
        while self.elements.len() > index {
            if let Some(element) = self.elements.pop() {
                self.close(element);
            }
        }
    }

    /// Ends what `element` began.
    fn close(&mut self, element: Element) {
        if element.styled {
            self.builder.pop_style();
        }
        match element.closes {
            Closes::Cell => self.builder.close(|kind| *kind == OpenKind::Cell),
            Closes::Container => self
                .builder
                .close(|kind| matches!(kind, OpenKind::Container(_))),
            Closes::Heading => self.builder.end_heading(),
            Closes::Item => self.builder.close(|kind| *kind == OpenKind::Item),
            Closes::List => self.builder.close(|kind| matches!(kind, OpenKind::List(_))),
            Closes::Nothing | Closes::Skip => {
                if element.name == "thead" {
                    self.in_head = false;
                }
            }
            Closes::Paragraph => self.builder.flush(),
            Closes::Pre => {
                if let Some(text) = self.pre.take() {
                    self.builder
                        .push(Block::CodeBlock(text.trim_start_matches('\n').to_owned()));
                }
            }
            Closes::Quote => self.builder.close(|kind| *kind == OpenKind::Quote),
            Closes::Row => self.builder.close(|kind| matches!(kind, OpenKind::Row(_))),
            Closes::Table => self.builder.close(|kind| *kind == OpenKind::Table),
        }
    }

    /// Closes the elements a `name` start ends without saying so: a `p` or
    /// an `li` before the next of its kind, a cell or a row before the
    /// next cell or row of its table.
    fn close_implied(&mut self, name: &str) {
        let (closes, within): (&[&str], &[&str]) = match name {
            "li" => (&["li"], &["ol", "ul"]),
            "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "table"
            | "pre" | "blockquote" => (&["p"], &["div", "li", "td", "th", "blockquote"]),
            "td" | "th" => (&["td", "th"], &["tr", "table"]),
            "tr" => (&["tr", "td", "th"], &["table", "thead", "tbody"]),
            _ => return,
        };
        let boundary = self
            .elements
            .iter()
            .rposition(|element| within.contains(&element.name.as_str()));
        let open = self
            .elements
            .iter()
            .rposition(|element| closes.contains(&element.name.as_str()));
        if let Some(index) = open.filter(|&open| boundary.is_none_or(|boundary| open > boundary)) {
            while self.elements.len() > index {
                if let Some(element) = self.elements.pop() {
                    self.close(element);
                }
            }
        }
    }

    /// Reads text: kept as it is in a preformatted block, its runs of
    /// blanks squeezed to one elsewhere.
    fn text(&mut self, text: &str) {
        if self.is_skipping() {
            return;
        }
        if let Some(pre) = &mut self.pre {
            pre.push_str(text);
            return;
        }
        let mut squeezed = String::with_capacity(text.len());
        let mut blank = false;
        for c in text.chars() {
            match c.is_whitespace() {
                true => blank = true,
                false => {
                    if blank && (!squeezed.is_empty() || self.builder.wants_space()) {
                        squeezed.push(' ');
                    }
                    blank = false;
                    squeezed.push(c);
                }
            }
        }
        if blank && (!squeezed.is_empty() || self.builder.wants_space()) {
            squeezed.push(' ');
        }
        self.text_raw(&squeezed);
    }

    /// Adds `text` as it is, or to the preformatted block being read.
    fn text_raw(&mut self, text: &str) {
        match &mut self.pre {
            Some(pre) => pre.push_str(text),
            None => self.builder.text(text),
        }
    }

    /// The styles of the classes in `attributes`, those the app styled.
    fn classes(&self, attributes: &HashMap<String, String>) -> Vec<ClassStyle> {
        attributes
            .get("class")
            .map(|classes| {
                classes
                    .split_whitespace()
                    .filter_map(|class| self.options.classes.get(class).copied())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The inline style a `name` element with `attributes` and `classes`
    /// sets.
    fn style_of(
        &self,
        name: &str,
        attributes: &HashMap<String, String>,
        classes: &[ClassStyle],
    ) -> InlineStyle {
        let mut style = InlineStyle::default();
        match name {
            "a" => {
                if let Some(href) = attributes.get("href") {
                    style.link = Some(Link::Href(href.clone()));
                    style.color = Some(TextColor::Token(ThemeToken::Accent));
                }
            }
            "b" | "strong" | "th" => style.bold = true,
            "code" | "kbd" | "tt" => style.monospace = true,
            "del" | "s" | "strike" => style.strike = true,
            "em" | "i" => style.italic = true,
            "small" | "sub" | "sup" => style.scale = Some(SMALL_SCALE),
            "u" => style.underline = true,
            _ => {}
        }
        for class in classes {
            style.bold |= class.bold;
            style.italic |= class.italic;
            style.hidden |= class.hidden;
            if let Some(color) = class.color {
                style.color = Some(TextColor::Token(color));
            }
            if let (Some(background), false) = (class.background, BLOCK_TAGS.contains(&name)) {
                style.background = Some(TextColor::Token(background));
            }
            if let Some(scale) = class.scale {
                style.scale = Some(style.scale.unwrap_or(1.0) * scale);
            }
        }
        if let Some(css) = attributes.get("style") {
            apply_css(&mut style, css);
        }
        for attribute in &self.options.data_attributes {
            if let Some(value) = attributes.get(attribute) {
                style.link = Some(Link::Data(DataLink {
                    name: attribute.clone(),
                    value: value.clone(),
                }));
            }
        }
        style
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The box a block element with `classes` (and the anchor `id`) is set
/// in, or `None` when its classes give it none and it has no anchor.
fn container_of(classes: &[ClassStyle], id: Option<&String>) -> Option<Container> {
    let mut container = Container {
        id: id.cloned(),
        ..Container::default()
    };
    for class in classes {
        container.background = class.background.or(container.background);
        container.border = class.border.or(container.border);
        container.indent = container.indent.saturating_add(class.indent);
    }
    container.padded = container.background.is_some() || container.border.is_some();
    let boxed = container.padded || container.indent > 0 || container.id.is_some();
    boxed.then_some(container)
}

/// Applies the inline `css` properties misty reads to `style`.
fn apply_css(style: &mut InlineStyle, css: &str) {
    for declaration in css.split(';') {
        let Some((property, value)) = declaration.split_once(':') else {
            continue;
        };
        let value = value.trim().to_lowercase();
        match property.trim().to_lowercase().as_str() {
            "background-color" => {
                if let Ok(color) = Rgba::from_hex(&value) {
                    style.background = Some(TextColor::Rgba(color));
                }
            }
            "color" => {
                if let Ok(color) = Rgba::from_hex(&value) {
                    style.color = Some(TextColor::Rgba(color));
                }
            }
            "display" => style.hidden |= value == "none",
            "font-size" => {
                if let Some(scale) = font_scale(&value) {
                    style.scale = Some(scale);
                }
            }
            "font-style" => style.italic |= value == "italic" || value == "oblique",
            "font-weight" => {
                let numeric = value
                    .parse::<u32>()
                    .is_ok_and(|weight| weight >= BOLD_WEIGHT);
                style.bold |= numeric || value == "bold" || value == "bolder";
            }
            "text-decoration" | "text-decoration-line" => {
                style.underline |= value.contains("underline");
                style.strike |= value.contains("line-through");
            }
            _ => {}
        }
    }
}

/// The multiple of the body's size a CSS `font-size` value stands for.
fn font_scale(value: &str) -> Option<f32> {
    let number = |suffix: &str| value.strip_suffix(suffix)?.trim().parse::<f32>().ok();
    number("em")
        .or_else(|| number("%").map(|percent| percent / PERCENT))
        .or_else(|| number("px").map(|pixels| pixels / BASE_FONT_PIXELS))
        .filter(|scale| *scale > 0.0)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::document::{Heading, Inline, TableBlock};

    fn texts(inlines: &[Inline]) -> String {
        inlines
            .iter()
            .map(|inline| inline.span.text.as_str())
            .collect()
    }

    fn read(html: &str) -> Document {
        Document::from_html(html, &mut HtmlOptions::default())
    }

    #[test]
    fn test_unclosed_tags_squeezed_blanks_and_skipped_scripts() {
        let doc =
            read("<p>one   <b>two</b>\n three<p>four<script>alert(1)</script><style>p{}</style>");
        let [Block::Paragraph(first), Block::Paragraph(second)] = doc.blocks.as_slice() else {
            panic!("{doc:#?}");
        };
        assert_eq!(texts(first), "one two three");
        assert!(first[1].span.bold);
        assert_eq!(texts(second), "four");
    }

    #[test]
    fn test_headings_lists_tables_and_breaks() {
        let doc = read(
            "<h2 id=\"en\">English</h2><ol start=3><li>a<li>b</ol>\
             <table><thead><tr><th>H</th></tr></thead><tr><td>x<br>y</td></tr></table>",
        );
        let [
            Block::Heading(Heading { id, level, .. }),
            Block::List(list),
            Block::Table(TableBlock { rows }),
        ] = doc.blocks.as_slice()
        else {
            panic!("{doc:#?}");
        };
        assert_eq!((id.as_deref(), *level), (Some("en"), 2));
        assert_eq!((list.start, list.items.len()), (Some(3), 2));
        assert!(rows[0].header && !rows[1].header);
        let [Block::Paragraph(cell)] = rows[1].cells[0].as_slice() else {
            panic!("{rows:#?}");
        };
        assert_eq!(texts(cell), "x\ny");
    }

    #[test]
    fn test_classes_styles_and_data_attributes_map_through_the_options() {
        let mut options = HtmlOptions {
            data_attributes: vec!["data-lookup".into()],
            ..HtmlOptions::default()
        };
        options.classes.insert(
            "sense".into(),
            ClassStyle {
                border: Some(ThemeToken::Border),
                ..ClassStyle::default()
            },
        );
        options.classes.insert(
            "hide".into(),
            ClassStyle {
                hidden: true,
                ..ClassStyle::default()
            },
        );
        let doc = Document::from_html(
            "<div class=\"sense\"><span style=\"color: #ff0000; font-size: 150%\">red</span> \
             <a data-lookup=\"cat\">cat</a><span class=\"hide\">secret</span> <sub>2</sub></div>",
            &mut options,
        );
        let [Block::Container(container)] = doc.blocks.as_slice() else {
            panic!("{doc:#?}");
        };
        assert_eq!(container.border, Some(ThemeToken::Border));
        let [Block::Paragraph(inlines)] = container.blocks.as_slice() else {
            panic!("{container:#?}");
        };
        assert_eq!(texts(inlines), "red cat 2", "the hidden span is left out");
        assert_eq!(inlines[0].span.color, TextColor::Rgba(Rgba::rgb(255, 0, 0)));
        assert!(
            matches!(inlines[0].span.size, crate::base::styled::TextSize::Scale(scale) if scale == 1.5)
        );
        assert_eq!(
            inlines[2].link,
            Some(Link::Data(DataLink {
                name: "data-lookup".into(),
                value: "cat".into()
            }))
        );
    }

    #[test]
    fn test_pre_keeps_its_lines_and_an_image_without_bytes_keeps_its_alt() {
        let doc = read("<pre>\nfn main() {\n    go();\n}</pre><img src=\"x.png\" alt=\"a fox\">");
        assert_eq!(
            doc.blocks[0],
            Block::CodeBlock("fn main() {\n    go();\n}".into())
        );
        assert!(matches!(&doc.blocks[1], Block::Paragraph(alt) if texts(alt) == "a fox"));
    }
}
