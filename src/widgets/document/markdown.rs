//! Markdown into a [`Document`], behind the `markdown` feature: CommonMark
//! with tables, strikethrough, task lists, and heading anchors
//! (`# Title {#id}`), read by pulldown-cmark. Raw HTML in the Markdown is
//! left out.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use super::super::super::base::styled::TextColor;
use super::super::super::base::theme::ThemeToken;
use super::super::super::render::image::Image;
use super::build::{Builder, InlineStyle, OpenKind};
use super::{Block, BlockImage, Document, Link};

// ========================================================================
// Constants
// ========================================================================

/// What a done task list item shows before its text.
const TASK_DONE: &str = "☑ ";

/// What an open task list item shows before its text.
const TASK_OPEN: &str = "☐ ";

// ========================================================================
// Document Implementations
// ========================================================================

impl Document {
    /// The document Markdown `source` describes, its images' bytes found
    /// by `image` from their URLs (`None` leaves an image out, keeping its
    /// alternative text as a paragraph).
    pub fn from_markdown(source: &str, mut image: impl FnMut(&str) -> Option<Image>) -> Self {
        let options = Options::ENABLE_TABLES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_HEADING_ATTRIBUTES;
        let mut builder = Builder::new();
        // The image being read: its URL and the text read for it so far.
        let mut reading_image: Option<(String, String)> = None;
        let mut code: Option<String> = None;
        for event in Parser::new_ext(source, options) {
            match event {
                Event::Start(tag) => match tag {
                    Tag::BlockQuote(_) => builder.open(OpenKind::Quote),
                    Tag::CodeBlock(_) => code = Some(String::new()),
                    Tag::Emphasis => builder.push_style(InlineStyle {
                        italic: true,
                        ..InlineStyle::default()
                    }),
                    Tag::Heading { level, id, .. } => {
                        builder.start_heading(level as u8, id.map(|id| id.to_string()));
                    }
                    Tag::Image { dest_url, .. } => {
                        reading_image = Some((dest_url.to_string(), String::new()))
                    }
                    Tag::Item => builder.open(OpenKind::Item),
                    Tag::Link { dest_url, .. } => builder.push_style(InlineStyle {
                        color: Some(TextColor::Token(ThemeToken::Accent)),
                        link: Some(Link::Href(dest_url.to_string())),
                        ..InlineStyle::default()
                    }),
                    Tag::List(start) => builder.open(OpenKind::List(start)),
                    Tag::Paragraph => builder.flush(),
                    Tag::Strikethrough => builder.push_style(InlineStyle {
                        strike: true,
                        ..InlineStyle::default()
                    }),
                    Tag::Strong => builder.push_style(InlineStyle {
                        bold: true,
                        ..InlineStyle::default()
                    }),
                    Tag::Table(_) => builder.open(OpenKind::Table),
                    Tag::TableCell => builder.open(OpenKind::Cell),
                    Tag::TableHead => builder.open(OpenKind::Row(true)),
                    Tag::TableRow => builder.open(OpenKind::Row(false)),
                    Tag::DefinitionList
                    | Tag::DefinitionListDefinition
                    | Tag::DefinitionListTitle
                    | Tag::FootnoteDefinition(_)
                    | Tag::HtmlBlock
                    | Tag::MetadataBlock(_)
                    | Tag::Subscript
                    | Tag::Superscript => {}
                },
                Event::End(tag) => match tag {
                    TagEnd::BlockQuote(_) => builder.close(|kind| *kind == OpenKind::Quote),
                    TagEnd::CodeBlock => {
                        if let Some(text) = code.take() {
                            builder.push(Block::CodeBlock(text));
                        }
                    }
                    TagEnd::Emphasis | TagEnd::Link | TagEnd::Strikethrough | TagEnd::Strong => {
                        builder.pop_style();
                    }
                    TagEnd::Heading(_) => builder.end_heading(),
                    TagEnd::Image => {
                        if let Some((url, alt)) = reading_image.take() {
                            match image(&url) {
                                Some(image) => {
                                    builder.push(Block::Image(BlockImage { alt, image }))
                                }
                                None => builder.text(&alt),
                            }
                        }
                    }
                    TagEnd::Item => builder.close(|kind| *kind == OpenKind::Item),
                    TagEnd::List(_) => builder.close(|kind| matches!(kind, OpenKind::List(_))),
                    TagEnd::Paragraph => builder.flush(),
                    TagEnd::Table => builder.close(|kind| *kind == OpenKind::Table),
                    TagEnd::TableCell => builder.close(|kind| *kind == OpenKind::Cell),
                    TagEnd::TableHead | TagEnd::TableRow => {
                        builder.close(|kind| matches!(kind, OpenKind::Row(_)));
                    }
                    TagEnd::DefinitionList
                    | TagEnd::DefinitionListDefinition
                    | TagEnd::DefinitionListTitle
                    | TagEnd::FootnoteDefinition
                    | TagEnd::HtmlBlock
                    | TagEnd::MetadataBlock(_)
                    | TagEnd::Subscript
                    | TagEnd::Superscript => {}
                },
                Event::Code(text) => {
                    builder.push_style(InlineStyle {
                        background: Some(TextColor::Token(ThemeToken::Surface)),
                        monospace: true,
                        ..InlineStyle::default()
                    });
                    builder.text(&text);
                    builder.pop_style();
                }
                Event::HardBreak => builder.text("\n"),
                Event::Rule => builder.push(Block::Rule),
                Event::SoftBreak => builder.text(" "),
                Event::TaskListMarker(done) => {
                    builder.text(if done { TASK_DONE } else { TASK_OPEN })
                }
                Event::Text(text) => match (&mut code, &mut reading_image) {
                    (Some(code), _) => code.push_str(&text),
                    (None, Some((_, alt))) => alt.push_str(&text),
                    (None, None) => builder.text(&text),
                },
                Event::DisplayMath(_)
                | Event::FootnoteReference(_)
                | Event::Html(_)
                | Event::InlineHtml(_)
                | Event::InlineMath(_) => {}
            }
        }
        builder.finish()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::document::{Heading, ListBlock, TableBlock};

    fn texts(inlines: &[super::super::Inline]) -> String {
        inlines
            .iter()
            .map(|inline| inline.span.text.as_str())
            .collect()
    }

    #[test]
    fn test_headings_lists_and_code_become_blocks() {
        let doc = Document::from_markdown(
            "# Title {#top}\n\nSome *soft* **bold** `code`\ntext.\n\n1. one\n2. two\n   - nested\n\n```\nlet x;\n```\n",
            |_| None,
        );
        let [
            Block::Heading(Heading { id, level, inlines }),
            Block::Paragraph(body),
            Block::List(list),
            Block::CodeBlock(code),
        ] = doc.blocks.as_slice()
        else {
            panic!("{doc:#?}");
        };
        assert_eq!(
            (id.as_deref(), *level, texts(inlines).as_str()),
            (Some("top"), 1, "Title")
        );
        assert_eq!(texts(body), "Some soft bold code text.");
        assert!(body[1].span.italic && body[3].span.bold);
        assert_eq!(list.start, Some(1));
        assert_eq!(list.items.len(), 2);
        assert!(matches!(
            list.items[1].last(),
            Some(Block::List(ListBlock { start: None, .. }))
        ));
        assert_eq!(code, "let x;\n");
    }

    #[test]
    fn test_a_table_keeps_its_header_row_and_a_link_its_target() {
        let doc = Document::from_markdown(
            "| a | b |\n|---|---|\n| [x](https://e.org) | ~~y~~ |\n",
            |_| None,
        );
        let [Block::Table(TableBlock { rows })] = doc.blocks.as_slice() else {
            panic!("{doc:#?}");
        };
        assert_eq!(rows.len(), 2);
        assert!(rows[0].header && !rows[1].header);
        let [Block::Paragraph(link)] = rows[1].cells[0].as_slice() else {
            panic!("{rows:#?}");
        };
        assert_eq!(link[0].link, Some(Link::Href("https://e.org".into())));
        let [Block::Paragraph(struck)] = rows[1].cells[1].as_slice() else {
            panic!("{rows:#?}");
        };
        assert!(struck[0].span.strikethrough);
    }

    #[test]
    fn test_an_image_is_a_block_or_its_alt_text_and_tasks_get_boxes() {
        let pixels = vec![0; 4];
        let doc = Document::from_markdown("![a cat](cat.png) and\n\n- [x] done\n", |url| {
            (url == "cat.png").then(|| Image::from_rgba(1, 1, pixels.clone()).expect("an image"))
        });
        assert!(matches!(&doc.blocks[0], Block::Image(BlockImage { alt, .. }) if alt == "a cat"));
        assert!(matches!(&doc.blocks[1], Block::Paragraph(rest) if texts(rest).trim() == "and"));
        let Block::List(list) = &doc.blocks[2] else {
            panic!("{doc:#?}");
        };
        let [Block::Paragraph(task)] = list.items[0].as_slice() else {
            panic!("{list:#?}");
        };
        assert_eq!(texts(task), "☑ done");
        let missing = Document::from_markdown("![gone](x.png)\n", |_| None);
        assert!(matches!(&missing.blocks[0], Block::Paragraph(alt) if texts(alt) == "gone"));
    }
}
