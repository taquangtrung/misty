//! Documents in a `RichText`: a dictionary-like entry built in code
//! (headings, a list, a table, a link, a clickable data span, a boxed
//! usage note, and an image drawn in code), and a Markdown document
//! (with the `markdown` feature on).
//!
//! Keys: `1` and `2` switch between the two documents, `u` jumps to the
//! entry's usage note, `f` searches for "cat" and `n` steps through the
//! matches, `t` switches between the dark and the light theme, and Esc or
//! the window's close button quits. Drag to select, Ctrl+C copies, and a
//! click on the link or on "kitten" shows what it reported at the bottom.
//!
//! ```sh
//! cargo run -p misty --example document --features markdown
//! ```

use std::time::Instant;

use misty::{
    AppContext, Application, Block, BlockImage, Container, DataLink, Document, Error, Frame,
    Heading, Image, Inline, Input, Key, KeyCode, Link, ListBlock, Mods, Paragraph, Rect, Rgba,
    RichText, RichTextResponse, Span, TableBlock, TableRow, TextColor, Theme, ThemeToken, Widget,
    WindowId, WindowOptions,
};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty document";

/// The Markdown document.
#[cfg(feature = "markdown")]
const MARKDOWN: &str = "# A Markdown page\n\nSome *emphasis*, **strong** text, `code`, and a \
[link](https://example.org).\n\n> A quotation, set in behind a bar.\n\n1. First\n2. Second\n   - \
nested\n\n| Column | Another |\n|--------|---------|\n| cat    | ~~dog~~ |\n\n```\nfn main() {}\n```\n";

/// The side of the image drawn in code, in pixels.
const IMAGE_SIDE: u32 = 64;

/// The height of the status line at the bottom, in lines.
const STATUS_LINES: f32 = 1.5;

/// What `f` searches for.
const QUERY: &str = "cat";

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: the view, what the last click reported, and the theme.
struct Demo {
    /// What the last click on a link or a data span reported.
    status: String,
    /// The colors everything is drawn in.
    theme: Theme,
    /// The document view.
    view: RichText,
    /// The one window, once open.
    window: Option<WindowId>,
}

// ========================================================================
// Demo Implementations
// ========================================================================

impl Demo {
    /// The demo showing the entry.
    fn new() -> Self {
        let mut view = RichText::document(entry());
        view.set_focused(true);
        Self {
            status: "Click the link or \"kitten\".".into(),
            theme: Theme::dark(),
            view,
            window: None,
        }
    }
}

impl Application for Demo {
    type Message = ();

    fn init(&mut self, _cx: &mut AppContext<'_, ()>, window: WindowId) {
        self.window = Some(window);
    }

    fn input(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        let key = |code| Input::Key(Key::new(code, Mods::NONE));
        match input {
            Input::CloseRequested => return cx.exit(),
            input if *input == key(KeyCode::Esc) => return cx.exit(),
            input if *input == key(KeyCode::Char('1')) => self.view.set_document(entry()),
            input if *input == key(KeyCode::Char('2')) => self.view.set_document(markdown()),
            input if *input == key(KeyCode::Char('u')) => {
                self.view.scroll_to_anchor("usage");
            }
            input if *input == key(KeyCode::Char('f')) => self.view.set_find(QUERY, false),
            input if *input == key(KeyCode::Char('n')) => self.view.next_match(),
            input if *input == key(KeyCode::Char('t')) => {
                self.theme = match self.theme == Theme::dark() {
                    true => Theme::light(),
                    false => Theme::dark(),
                };
            }
            input => match self.view.handle(input, &mut cx.event_cx(window)) {
                RichTextResponse::Data(data) => {
                    self.status = format!("data {} = {}", data.name, data.value)
                }
                RichTextResponse::Link(target) => self.status = format!("link {target}"),
                RichTextResponse::Handled | RichTextResponse::Ignored => {}
                _ => {}
            },
        }
        cx.event_cx(window).request_redraw();
    }

    fn about_to_wait(&mut self, cx: &mut AppContext<'_, ()>) -> Option<Instant> {
        let window = self.window?;
        self.view.about_to_wait(&mut cx.event_cx(window))
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        frame.clear(Rgba::from(self.theme.background));
        let bounds = frame.bounds();
        let status = (frame.cell_metrics().height * STATUS_LINES).round();
        let body = Rect::new(bounds.x, bounds.y, bounds.width, bounds.height - status);
        self.view.draw(frame, body, &self.theme);
        let line = Paragraph::plain(self.status.as_str(), TextColor::Token(ThemeToken::Muted));
        frame.draw_paragraph(
            &line,
            Rect::new(bounds.x, body.height, bounds.width, status),
            &self.theme,
        );
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A dictionary-like entry built in code.
fn entry() -> Document {
    let text = |text: &str| Block::Paragraph(vec![Inline::plain(text)]);
    let accent = |text: &str| Span::new(text, TextColor::Token(ThemeToken::Accent));
    Document::new(vec![
        Block::Heading(Heading {
            id: None,
            inlines: vec![Inline::plain("cat")],
            level: 1,
        }),
        Block::Paragraph(vec![
            Inline::styled(Span::new("noun", TextColor::Token(ThemeToken::Muted)).italic()),
            Inline::plain(" A small domesticated carnivorous mammal; see "),
            Inline::styled(accent("kitten").underlined()).with_link(Link::Data(DataLink {
                name: "data-lookup".into(),
                value: "kitten".into(),
            })),
            Inline::plain(" or "),
            Inline::styled(accent("the encyclopedia"))
                .with_link(Link::Href("https://en.wikipedia.org/wiki/Cat".into())),
            Inline::plain("."),
        ]),
        Block::Image(BlockImage {
            alt: "A gradient standing in for a picture of a cat".into(),
            image: gradient(),
        }),
        Block::Heading(Heading {
            id: None,
            inlines: vec![Inline::plain("Senses")],
            level: 2,
        }),
        Block::List(ListBlock {
            items: vec![
                vec![text("The animal itself.")],
                vec![text(
                    "Any member of the cat family, such as a lion or a tiger.",
                )],
            ],
            start: Some(1),
        }),
        Block::Table(TableBlock {
            rows: vec![
                TableRow {
                    cells: vec![vec![text("Form")], vec![text("Example")]],
                    header: true,
                },
                TableRow {
                    cells: vec![vec![text("plural")], vec![text("cats")]],
                    header: false,
                },
                TableRow {
                    cells: vec![vec![text("diminutive")], vec![text("kitty, kitten")]],
                    header: false,
                },
            ],
        }),
        Block::Container(Container {
            background: Some(ThemeToken::Surface),
            blocks: vec![
                Block::Heading(Heading {
                    id: None,
                    inlines: vec![Inline::plain("Usage")],
                    level: 3,
                }),
                text("\"Let the cat out of the bag\" means to reveal a secret."),
            ],
            border: Some(ThemeToken::Border),
            id: Some("usage".into()),
            indent: 0,
            padded: true,
        }),
    ])
}

/// The Markdown document, or a note saying how to see it.
fn markdown() -> Document {
    #[cfg(feature = "markdown")]
    return Document::from_markdown(MARKDOWN, |_| None);
    #[cfg(not(feature = "markdown"))]
    Document::new(vec![Block::Paragraph(vec![Inline::plain(
        "Run with --features markdown to see a Markdown document here.",
    )])])
}

/// A square image drawn in code: a gradient from the corners.
fn gradient() -> Image {
    let side = IMAGE_SIDE;
    let channel = |value: u32| (value * u32::from(u8::MAX) / side) as u8;
    let pixels = (0..side * side)
        .flat_map(|index| {
            let (x, y) = (index % side, index / side);
            [channel(x), channel(y), channel(side - x), u8::MAX]
        })
        .collect();
    Image::from_rgba(side, side, pixels).expect("as many pixels as the size says")
}

fn main() -> Result<(), Error> {
    misty::run(Demo::new(), WindowOptions::new(TITLE))
}
