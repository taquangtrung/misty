//! Motion: three buttons whose hover fades in and out, a card that
//! crossfades between two looks, a side panel that slides in and out, and
//! a floating panel that snaps to the window's corners. Each motion follows
//! the one pattern `misty::Tween` documents: started on an input, read in
//! `render`, and its next frame returned from `about_to_wait`.
//!
//! Keys: `c` crossfades the card, `s` slides the side panel, `p` brings
//! back the floating panel after its close button closed it, Alt and an
//! arrow move the floating panel to another corner, `t` switches between
//! the dark and the light theme, and Esc or the window's close button
//! quits. The buttons do what `c`, `s`, and `p` do; drag the floating
//! panel by its header.
//!
//! ```sh
//! cargo run -p misty --example motion
//! ```

use std::time::{Duration, Instant};

use misty::{
    AppContext, Application, Button, ButtonResponse, ButtonVariant, Corner, Easing, Error,
    FloatingPanel, FloatingPanelResponse, Frame, Input, Key, KeyCode, Mods, Paragraph, Point, Rect,
    Rgba, TextColor, Theme, ThemeToken, Tween, Widget, WindowId, WindowOptions,
};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty motion";

/// How long the card's crossfade takes.
const CROSSFADE: Duration = Duration::from_millis(400);

/// How long the side panel takes to slide in or out.
const SLIDE: Duration = Duration::from_millis(250);

/// The room around everything, in logical pixels.
const MARGIN: f32 = 24.0;

/// The height of the row of buttons, in logical pixels.
const BUTTON_ROW: f32 = 40.0;

/// The gap between two buttons, in logical pixels.
const BUTTON_GAP: f32 = 12.0;

/// The card's size, in logical pixels.
const CARD_SIZE: (f32, f32) = (260.0, 140.0);

/// The side panel's width, in logical pixels.
const SIDE_WIDTH: f32 = 220.0;

/// The floating panel's size, in logical pixels.
const FLOATING_SIZE: (f32, f32) = (200.0, 110.0);

/// The labels of the buttons, in order.
const LABELS: [&str; 3] = ["Crossfade", "Slide", "Panel"];

// ========================================================================
// Data Structures
// ========================================================================

/// The demo's state.
struct Demo {
    /// The row of buttons.
    buttons: [Button; 3],
    /// The card's mix from its first look (0) to its second (1).
    crossfade: Tween<f32>,
    /// The floating panel, or `None` once closed.
    floating: Option<FloatingPanel>,
    /// How far the side panel is in, from 0 (hidden) to 1.
    slide: Tween<f32>,
    /// The colors everything is drawn in.
    theme: Theme,
    /// The one window, once open.
    window: Option<WindowId>,
}

/// What the user asked for, by key or by button.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    /// Crossfade the card to its other look.
    Crossfade,
    /// Bring back the floating panel.
    Panel,
    /// Slide the side panel in or out.
    Slide,
}

// ========================================================================
// Demo Implementations
// ========================================================================

impl Demo {
    /// The demo at rest: the card in its first look, the side panel
    /// hidden, the floating panel in the bottom right corner.
    fn new() -> Self {
        let buttons = [
            Button::new(LABELS[0]).with_variant(ButtonVariant::Primary),
            Button::new(LABELS[1]),
            Button::new(LABELS[2]).with_variant(ButtonVariant::Ghost),
        ];
        Self {
            buttons,
            crossfade: Tween::new(0.0, 0.0, CROSSFADE).with_easing(Easing::EaseInOut),
            floating: Some(floating_panel()),
            slide: Tween::new(0.0, 0.0, SLIDE),
            theme: Theme::dark(),
            window: None,
        }
    }

    /// Does `action`, starting its motion at `now`.
    fn act(&mut self, action: Action, now: Instant) {
        match action {
            Action::Crossfade => {
                let to = 1.0 - self.crossfade.target();
                self.crossfade.retarget(to, now);
            }
            Action::Panel => {
                self.floating.get_or_insert_with(floating_panel);
            }
            Action::Slide => {
                let to = 1.0 - self.slide.target();
                self.slide.retarget(to, now);
            }
        }
    }

    /// Offers the pointer and keys to the floating panel, then to the
    /// buttons, returning the action a click asked for.
    fn route(
        &mut self,
        cx: &mut AppContext<'_, ()>,
        window: WindowId,
        input: &Input,
    ) -> Option<Action> {
        let mut event = cx.event_cx(window);
        if let Some(panel) = &mut self.floating {
            match panel.handle(input, &mut event) {
                FloatingPanelResponse::Closed => {
                    self.floating = None;
                    event.request_redraw();
                    return None;
                }
                FloatingPanelResponse::Handled | FloatingPanelResponse::Moved => return None,
                FloatingPanelResponse::Ignored => {}
                _ => return None,
            }
        }
        let mut clicked = None;
        for (button, action) in
            self.buttons
                .iter_mut()
                .zip([Action::Crossfade, Action::Slide, Action::Panel])
        {
            if button.handle(input, &mut event) == ButtonResponse::Clicked {
                clicked = Some(action);
            }
        }
        clicked
    }

    /// Draws the card in `area`, crossfading from its first look to its
    /// second by `mix`.
    fn draw_card(&self, frame: &mut Frame, area: Rect, mix: f32) {
        let theme = &self.theme;
        let scale = frame.scale_factor();
        let looks = [
            (theme.surface, "Day", theme.foreground),
            (theme.accent, "Night", theme.background),
        ];
        for ((fill, text, ink), opacity) in looks.into_iter().zip([1.0 - mix, mix]) {
            frame.with_opacity(opacity, |frame| {
                frame.draw_box(area, &theme.shape.floating_box(fill, theme.border, scale));
                let label = Paragraph::plain(text, TextColor::Rgba(Rgba::from(ink)));
                let inset = MARGIN * scale as f32;
                let room = Rect::new(
                    area.x + inset,
                    area.y + inset,
                    area.width - 2.0 * inset,
                    area.height,
                );
                frame.draw_paragraph(&label, room, theme);
            });
        }
    }

    /// Draws the side panel along the left of `bounds`, slid in by `shown`
    /// of its width.
    fn draw_side(&self, frame: &mut Frame, bounds: Rect, shown: f32) {
        if shown <= 0.0 {
            return;
        }
        let theme = &self.theme;
        let scale = frame.scale_factor();
        let width = SIDE_WIDTH * scale as f32;
        let panel = Rect::new(bounds.x, bounds.y, width, bounds.height);
        frame.translated(Point::new(-width * (1.0 - shown), 0.0), |frame| {
            frame.draw_box(
                panel,
                &theme.shape.floating_box(theme.surface, theme.border, scale),
            );
            let inset = MARGIN * scale as f32;
            let label = Paragraph::plain(
                "A panel sliding in",
                TextColor::Token(ThemeToken::Foreground),
            );
            let room = Rect::new(
                panel.x + inset,
                panel.y + inset,
                panel.width - 2.0 * inset,
                panel.height,
            );
            frame.draw_paragraph(&label, room, theme);
        });
    }
}

impl Application for Demo {
    type Message = ();

    fn init(&mut self, _cx: &mut AppContext<'_, ()>, window: WindowId) {
        self.window = Some(window);
    }

    fn input(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        let key = |code| Input::Key(Key::new(code, Mods::NONE));
        let now = Instant::now();
        let action = match input {
            Input::CloseRequested => return cx.exit(),
            input if *input == key(KeyCode::Esc) => return cx.exit(),
            input if *input == key(KeyCode::Char('c')) => Some(Action::Crossfade),
            input if *input == key(KeyCode::Char('s')) => Some(Action::Slide),
            input if *input == key(KeyCode::Char('p')) => Some(Action::Panel),
            input if *input == key(KeyCode::Char('t')) => {
                self.theme = match self.theme == Theme::dark() {
                    true => Theme::light(),
                    false => Theme::dark(),
                };
                None
            }
            input => self.route(cx, window, input),
        };
        if let Some(action) = action {
            self.act(action, now);
        }
        cx.event_cx(window).request_redraw();
    }

    fn about_to_wait(&mut self, cx: &mut AppContext<'_, ()>) -> Option<Instant> {
        let window = self.window?;
        let now = Instant::now();
        let mut event = cx.event_cx(window);
        let buttons = self
            .buttons
            .iter_mut()
            .filter_map(|button| button.about_to_wait(&mut event));
        let tweens = [self.crossfade.next_frame(now), self.slide.next_frame(now)];
        let next = buttons.chain(tweens.into_iter().flatten()).min()?;
        cx.event_cx(window).request_redraw_at(next);
        Some(next)
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        let now = Instant::now();
        let theme = self.theme.clone();
        frame.clear(Rgba::from(theme.background));
        let scale = frame.scale_factor() as f32;
        let bounds = frame.bounds();
        let margin = MARGIN * scale;
        let row = Rect::new(
            margin,
            margin,
            bounds.width - 2.0 * margin,
            BUTTON_ROW * scale,
        );
        let mut x = row.x;
        for button in &mut self.buttons {
            let drawn = button.draw(
                frame,
                Rect::new(x, row.y, row.width - (x - row.x), row.height),
                &theme,
            );
            x = drawn.x + drawn.width + BUTTON_GAP * scale;
        }
        let card = Rect::new(
            margin,
            row.y + row.height + margin,
            CARD_SIZE.0 * scale,
            CARD_SIZE.1 * scale,
        );
        self.draw_card(frame, card, self.crossfade.value(now));
        self.draw_side(frame, bounds, self.slide.value(now));
        if let Some(panel) = &mut self.floating {
            frame.overlay(|frame| {
                let content = panel.draw(frame, bounds, &theme);
                let label =
                    Paragraph::plain("Drag me by my header", TextColor::Token(ThemeToken::Muted));
                let inset = MARGIN * scale / 2.0;
                let room = Rect::new(
                    content.x + inset,
                    content.y + inset,
                    content.width - 2.0 * inset,
                    content.height,
                );
                frame.draw_paragraph(&label, room, &theme);
            });
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The floating panel, closable, snapped to the bottom right corner and
/// taking Alt and the arrows.
fn floating_panel() -> FloatingPanel {
    let mut panel = FloatingPanel::new("Notes", FLOATING_SIZE.0, FLOATING_SIZE.1)
        .with_close_button()
        .with_snapping(Corner::BottomRight);
    panel.set_focused(true);
    panel
}

fn main() -> Result<(), Error> {
    misty::run(Demo::new(), WindowOptions::new(TITLE))
}
