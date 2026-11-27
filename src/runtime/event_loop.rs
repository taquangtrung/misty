//! The window runtime: runs the event loop, opens the window and its GPU
//! surface, turns window-system events and [`Waker`] messages into
//! [`Application`] calls (each guarded against panics), follows the user's
//! appearance settings and each window's place on the desktop, and sleeps
//! until the next event or deadline.

use std::mem;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use accesskit::TreeUpdate;
use accesskit_winit::{Adapter, Event as AccessEvent, WindowEvent as AccessEventKind};
use wgpu::TextureViewDescriptor;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{self, Fullscreen, Theme, Window};

use super::super::app::access::{AccessTree, access_action};
use super::super::app::application::{AppContext, Application, OpenView};
use super::super::app::context::{Requests, WindowAction, WindowState};
use super::super::app::frame::Frame;
use super::super::app::hang::HangPhase;
use super::super::app::window::{
    OptionAsAlt, Position, ShowPolicy, Size, WindowGeometry, WindowOptions, monitors, window_icon,
};
use super::super::base::appearance::{Appearance, ColorScheme};
use super::super::base::cursor::CursorIcon;
use super::super::input::event::{Input, InputReader};
use super::super::platform::appearance;
use super::super::platform::clipboard::SystemClipboard;
use super::super::platform::compositor;
use super::super::platform::error::Error;
use super::super::platform::focus;
use super::super::platform::schedule::{RedrawSchedule, earliest};
use super::super::platform::waker::{UserEvent, Waker};
use super::super::platform::window_group;
use super::super::platform::window_id::WindowId;
use super::super::render::font::{CellMetrics, Font};
use super::super::render::gpu::GpuContext;
use super::super::render::surface::WindowSurface;
use super::super::render::{Fonts, FrameTarget, Renderer, ShapingFont};
use super::guard::{PanicRate, survive};
use super::watchdog::{self, Beat, Busy};

// ========================================================================
// Data Structures
// ========================================================================

/// The event loop's handler: the app, its windows, and the first error
/// that stopped it.
struct Runtime<A: Application> {
    /// The app being run.
    app: A,
    /// The user's appearance settings, as last read.
    appearance: Appearance,
    /// The clipboard lent to every callback.
    clipboard: SystemClipboard,
    /// The error that ended the event loop early, returned by [`run`].
    error: Option<Error>,
    /// How to open the first window, until it is open.
    first: Option<WindowOptions>,
    /// What the main thread marks its callbacks in, for the watchdog to
    /// find one that hangs; `None` when the app watches nothing.
    beat: Option<Arc<Beat>>,
    /// The app's recent panics.
    panics: PanicRate,
    /// The event loop's inbox, which the accessibility adapters post to.
    proxy: EventLoopProxy<UserEvent<A::Message>>,
    /// The app's reduced-motion override: `None` to follow the user's
    /// settings.
    reduced_motion: Option<bool>,
    /// The redraws the app asked for at a later time.
    schedule: RedrawSchedule,
    /// The GPU and fonts every window shares, made with the first window.
    shared: Option<Shared>,
    /// The event loop's inbox, cloned out to the app.
    waker: Waker<A::Message>,
    /// The open windows, in the order opened.
    windows: Vec<OpenWindow>,
}

/// What every window draws with.
struct Shared {
    /// The installed fonts and rasterized glyphs, also lent to frames to
    /// measure paragraphs.
    fonts: Arc<Mutex<Fonts>>,
    /// The GPU.
    gpu: GpuContext,
}

/// A window the runtime has opened, with its surface.
struct OpenWindow {
    /// The window's accessibility adapter.
    access: Adapter,
    /// Whether an assistive technology listens, so trees are sent.
    access_active: bool,
    /// The last tree sent, so an unchanged one is not sent again.
    access_sent: Option<TreeUpdate>,
    /// The accessibility tree of the last frame, for actions on it.
    access_tree: AccessTree,
    /// The radius misty rounds the window's corners by itself, in logical
    /// pixels, or `None` for square corners or a system that rounds them.
    corner_radius: Option<u16>,
    /// The font the window draws text in.
    font: Font,
    /// The window's identity, as the app knows it.
    id: WindowId,
    /// Reads the window's events into inputs.
    input: InputReader,
    /// Whether the window is maximized, read again after each resize.
    maximized: bool,
    /// Where the window was last while not maximized, where the window
    /// system says.
    normal_position: Option<PhysicalPosition<i32>>,
    /// The window's inner size when last not maximized.
    normal_size: PhysicalSize<u32>,
    /// The size of a cell in `font` at the window's scale factor.
    metrics: CellMetrics,
    /// The window's pipelines.
    renderer: Renderer,
    /// Whether the window has been made visible.
    shown: bool,
    /// The GPU surface the window is drawn to. Declared before `window`
    /// so it is dropped first.
    surface: WindowSurface,
    /// The window's title, which names its accessibility tree (winit
    /// cannot read it back on X11 and Wayland).
    title: String,
    /// The window itself.
    window: Arc<Window>,
}

/// What the runtime does after a callback, worked out from what the app
/// asked for and which windows are open.
#[derive(Debug, Default, PartialEq)]
struct Settlement {
    /// The open windows to close, each once.
    close: Vec<WindowId>,
    /// Whether to end the event loop.
    exit: bool,
    /// The windows to redraw, each once, none of them closing.
    redraw: Vec<WindowId>,
}

// ========================================================================
// Runtime Implementations
// ========================================================================

impl<A: Application> Runtime<A> {
    /// Gives `window`, opened with `options`, its surface and pipelines,
    /// then lets the app initialize it.
    fn set_up_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        window: Arc<Window>,
        options: &WindowOptions,
    ) -> Result<(), Error> {
        let (shared, surface) = match self.shared.take() {
            Some(shared) => match shared.gpu.create_surface(Arc::clone(&window)) {
                Ok(surface) => (shared, surface),
                Err(error) => {
                    self.shared = Some(shared);
                    return Err(error);
                }
            },
            None => {
                let display = event_loop.owned_display_handle();
                let (gpu, surface) = GpuContext::new(display, Arc::clone(&window))?;
                let fonts = Arc::new(Mutex::new(Fonts::new()));
                (Shared { fonts, gpu }, surface)
            }
        };
        let shared = self.shared.insert(shared);
        let size = window.inner_size();
        let maximized = window.is_maximized();
        let normal_size = match maximized {
            true => options
                .size()
                .to_logical()
                .to_physical(window.scale_factor()),
            false => size,
        };
        // Windows and macOS say light or dark through winit; the system's
        // settings, where they were read, say it first.
        if self.appearance.color_scheme.is_none() {
            self.appearance.color_scheme = window.theme().map(scheme_of);
        }
        let surface = WindowSurface::new(
            &shared.gpu,
            surface,
            size.width,
            size.height,
            options.transparent(),
        )?;
        let mut renderer = Renderer::new(&shared.gpu, surface.format());
        renderer.set_texture_budget(options.texture_budget());
        let font = options.font().clone();
        let metrics = lock(&shared.fonts).measure_cell(&font, window.scale_factor());
        let id = WindowId(window.id());
        // Made while the window is still hidden, as AccessKit requires.
        let access = Adapter::with_event_loop_proxy(event_loop, &window, self.proxy.clone());
        if !options.is_active() {
            focus::decline_first_focus(&window);
        }
        window_group::join(&window);
        let shown = match options.show() {
            ShowPolicy::AfterFirstFrame => false,
            ShowPolicy::Immediately => {
                window.set_visible(true);
                true
            }
        };
        window.request_redraw();
        self.windows.push(OpenWindow {
            access,
            access_active: false,
            access_sent: None,
            access_tree: AccessTree::default(),
            // Corners cut out without a compositor would show black.
            corner_radius: options
                .drawn_corner_radius()
                .filter(|_| compositor::composites(&window)),
            font,
            id,
            input: InputReader::default(),
            maximized,
            metrics,
            normal_position: window.outer_position().ok(),
            normal_size,
            renderer,
            shown,
            surface,
            title: options.title().to_owned(),
            window,
        });
        self.call_app(event_loop, |app, cx| app.init(cx, id));
        Ok(())
    }

    /// Runs `call` on the app with a fresh context, then carries out what
    /// the app asked for. A panic in `call` is reported to the app and the
    /// result is `None`.
    fn call_app<R>(
        &mut self,
        event_loop: &ActiveEventLoop,
        call: impl FnOnce(&mut A, &mut AppContext<'_, A::Message>) -> R,
    ) -> Option<R> {
        let windows = self
            .windows
            .iter()
            .map(|open| OpenView {
                geometry: open.geometry(),
                id: open.id,
                state: open.state(),
                #[cfg(feature = "dialogs")]
                window: Some(Arc::clone(&open.window)),
            })
            .collect();
        let mut cx = AppContext::new(
            &self.waker,
            Some(event_loop),
            windows,
            &mut self.clipboard,
            self.appearance,
            self.reduced_motion,
            self.shared.as_ref().map(|shared| shared.gpu.lend()),
        );
        let app = &mut self.app;
        let result = survive(&mut self.panics, || call(app, &mut cx));
        let value = match result {
            Ok(value) => Some(value),
            Err(why) => {
                self.app.on_panic(&mut cx, &why);
                None
            }
        };
        let requests = cx.into_requests();
        self.apply_requests(event_loop, requests);
        value
    }

    /// Carries out what the app asked for: sets up the windows it opened,
    /// closes those it closed, and redraws and schedules the rest.
    fn apply_requests(&mut self, event_loop: &ActiveEventLoop, requests: Requests) {
        let Requests {
            actions,
            closed,
            cursors,
            exit,
            // Always empty: only a headless test mints windows.
            minted: _,
            mints: _,
            opened,
            redraws,
            reduced_motion,
            scheduled,
        } = requests;
        if let Some(reduced_motion) = reduced_motion.filter(|asked| *asked != self.reduced_motion) {
            self.reduced_motion = reduced_motion;
            for open in &self.windows {
                open.window.request_redraw();
            }
        }
        for new in opened {
            if let Err(error) = self.set_up_window(event_loop, new.window, &new.options) {
                log::error!("could not draw to a new window, so it was closed: {error}");
            }
        }
        let open: Vec<WindowId> = self.windows.iter().map(|open| open.id).collect();
        let settlement = settle(
            &open,
            &closed,
            &redraws,
            exit,
            self.app.exit_when_last_window_closes(),
        );
        for id in &settlement.close {
            self.windows.retain(|open| open.id != *id);
            self.schedule.forget(*id);
        }
        for (window, at) in scheduled {
            if self.find(window).is_some() {
                self.schedule.add(window, at);
            }
        }
        for id in settlement.redraw {
            if let Some(open) = self.find(id) {
                open.window.request_redraw();
            }
        }
        for (id, action) in actions {
            let Some(open) = self.windows.iter_mut().find(|open| open.id == id) else {
                continue;
            };
            match (action, &self.shared) {
                (WindowAction::SetFont(font), Some(shared)) => {
                    let scale_factor = open.window.scale_factor();
                    open.metrics = lock(&shared.fonts).measure_cell(&font, scale_factor);
                    open.font = font;
                }
                (WindowAction::SetTransparent(transparent), Some(shared)) => {
                    // Corners misty rounds itself need the desktop behind them.
                    let transparent = transparent || open.corner_radius.is_some();
                    open.window.set_transparent(transparent);
                    open.surface.set_transparent(&shared.gpu, transparent);
                    open.window.request_redraw();
                }
                (WindowAction::SetTitle(title), _) => {
                    open.window.set_title(&title);
                    open.title = title;
                }
                (action, _) => open.carry_out(action),
            }
        }
        for (id, icon) in pointer_shapes(&cursors) {
            if let Some(open) = self.find(id) {
                open.window.set_cursor(winit_cursor(icon));
            }
        }
        if settlement.exit {
            event_loop.exit();
        }
    }

    /// Has the app build a frame of window `id` and presents it, showing
    /// the window after its first frame if it is still hidden. A frame
    /// whose `render` panicked is not presented.
    fn draw_frame(&mut self, event_loop: &ActiveEventLoop, id: WindowId) {
        let Some(open) = self.find(id) else {
            return;
        };
        let Some(shared) = &self.shared else {
            return;
        };
        let (width, height) = open.surface.size();
        let scale_factor = open.window.scale_factor();
        let mut frame = Frame::new(
            (width, height),
            scale_factor,
            open.metrics,
            Arc::clone(&shared.fonts),
            open.font.physical_size(scale_factor),
            open.font.weights(),
        );
        frame.set_maximized(open.maximized);
        // Rounded only while its edges float on the desktop, and only where
        // the surface shows what is behind it.
        if let Some(radius) = open
            .corner_radius
            .filter(|_| !open.maximized && open.surface.is_blending())
        {
            frame.set_corner_radius((f32::from(radius) * scale_factor as f32).round());
        }
        frame.set_reduced_motion(self.reduced_motion());
        let app = &mut self.app;
        if let Err(why) = survive(&mut self.panics, || app.render(id, &mut frame)) {
            self.call_app(event_loop, |app, cx| app.on_panic(cx, &why));
            return;
        }
        let Some(shared) = &mut self.shared else {
            return;
        };
        let Some(open) = self.windows.iter_mut().find(|open| open.id == id) else {
            return;
        };
        open.send_access_tree(mem::take(&mut frame.access));
        let Some(texture) = open.surface.acquire_texture(&shared.gpu) else {
            return;
        };
        let view = texture
            .texture
            .create_view(&TextureViewDescriptor::default());
        let font = ShapingFont {
            family: open.font.family(),
            ligatures: open.font.has_ligatures(),
            metrics: open.metrics,
            size: open.font.physical_size(open.window.scale_factor()),
            weights: open.font.weights(),
        };
        let target = FrameTarget {
            clear: open.surface.clear_color(frame.clear_color),
            corner_radius: frame.corner_radius(),
            layers: [&frame.draw, &frame.overlay],
            size: (width, height),
            view: &view,
        };
        open.renderer
            .render(&shared.gpu, &mut lock(&shared.fonts), &font, target);
        open.window.pre_present_notify();
        shared.gpu.queue.present(texture);
        if !open.shown {
            open.shown = true;
            open.window.set_visible(true);
            // Some window systems drop what was presented while the window
            // was unmapped; draw once more now that it is on screen.
            open.window.request_redraw();
        }
    }

    /// Keeps the surface and cell metrics of window `id` in step with its
    /// resize or scale change.
    fn follow_window_size(&mut self, id: WindowId, event: &WindowEvent) {
        let Some(shared) = &mut self.shared else {
            return;
        };
        let Some(open) = self.windows.iter_mut().find(|open| open.id == id) else {
            return;
        };
        if let WindowEvent::ScaleFactorChanged { scale_factor, .. } = event {
            open.metrics = lock(&shared.fonts).measure_cell(&open.font, *scale_factor);
        }
        // A scale change may or may not be followed by a resize, so both
        // read the size the window has now.
        open.maximized = open.window.is_maximized();
        let size = open.window.inner_size();
        if !open.maximized {
            open.normal_size = size;
        }
        open.surface.resize(&shared.gpu, size.width, size.height);
        open.window.request_redraw();
    }

    /// Acts on what an assistive technology asked of a window: start
    /// sending it trees (the whole tree with the next frame), stop, or
    /// pass a click or focus request on to the app.
    fn follow_access(&mut self, event_loop: &ActiveEventLoop, event: AccessEvent) {
        let id = WindowId(event.window_id);
        let Some(open) = self.windows.iter_mut().find(|open| open.id == id) else {
            return;
        };
        let action = match event.window_event {
            AccessEventKind::AccessibilityDeactivated => {
                open.access_active = false;
                None
            }
            AccessEventKind::ActionRequested(request) => access_action(&request, &open.access_tree),
            AccessEventKind::InitialTreeRequested => {
                open.access_active = true;
                open.access_sent = None;
                open.window.request_redraw();
                None
            }
        };
        if let Some(action) = action {
            self.call_app(event_loop, |app, cx| app.access_action(cx, id, action));
        }
    }

    /// Takes `appearance` as the user's settings and, when it differs from
    /// what they were, tells the app through every open window.
    fn follow_appearance(&mut self, event_loop: &ActiveEventLoop, appearance: Appearance) {
        if appearance == self.appearance {
            return;
        }
        self.appearance = appearance;
        let input = Input::AppearanceChanged(appearance);
        let ids: Vec<WindowId> = self.windows.iter().map(|open| open.id).collect();
        for id in ids {
            self.call_app(event_loop, |app, cx| app.input(cx, id, &input));
        }
    }

    /// Whether widgets cut their animations short: the app's override, or
    /// else the user's settings.
    fn reduced_motion(&self) -> bool {
        self.reduced_motion
            .unwrap_or(self.appearance.reduced_motion)
    }

    /// The open window `id`.
    fn find(&self, id: WindowId) -> Option<&OpenWindow> {
        self.windows.iter().find(|open| open.id == id)
    }
}

// ========================================================================
// OpenWindow Implementations
// ========================================================================

impl OpenWindow {
    /// Where the window is and how big, in logical pixels: where it goes
    /// back to when it is maximized.
    fn geometry(&self) -> WindowGeometry {
        let scale_factor = self.window.scale_factor();
        let size = self.normal_size.to_logical::<u32>(scale_factor);
        WindowGeometry {
            maximized: self.maximized,
            position: self.normal_position.map(|position| {
                let position = position.to_logical::<i32>(scale_factor);
                Position::new(position.x, position.y)
            }),
            size: Size::new(size.width, size.height),
        }
    }

    /// The window's pointer, modifiers, and maximized state.
    fn state(&self) -> WindowState {
        WindowState {
            clicks: self.input.click_count(),
            key_repeat: self.input.key_repeat(),
            maximized: self.maximized,
            mods: self.input.mods(),
            physical_key: self.input.physical_key(),
            pointer: self.input.pointer(),
            // The context fills it in from the app's override and settings.
            reduced_motion: false,
        }
    }

    /// Asks `action` of the window system. A move or resize is refused
    /// when no button is down or the platform cannot; either way there is
    /// nothing to do.
    fn carry_out(&self, action: WindowAction) {
        let window = &self.window;
        match action {
            WindowAction::Drag => {
                let _ = window.drag_window();
            }
            WindowAction::Focus => window.focus_window(),
            WindowAction::Minimize => window.set_minimized(true),
            WindowAction::Move(position) => {
                window.set_outer_position(LogicalPosition::new(position.x, position.y));
            }
            WindowAction::Resize(edge) => {
                let _ = window.drag_resize_window(edge.direction());
            }
            WindowAction::SetImeAllowed(allowed) => window.set_ime_allowed(allowed),
            WindowAction::SetOptionAsAlt(option_as_alt) => set_option_as_alt(window, option_as_alt),
            WindowAction::SetImeArea(area) => window.set_ime_cursor_area(
                PhysicalPosition::new(area.x, area.y),
                PhysicalSize::new(area.width, area.height),
            ),
            WindowAction::SetIcon(icon) => {
                window.set_window_icon(icon.as_ref().and_then(window_icon));
            }
            WindowAction::SetVisible(visible) => window.set_visible(visible),
            WindowAction::ToggleFullscreen => window.set_fullscreen(match window.fullscreen() {
                Some(_) => None,
                None => Some(Fullscreen::Borderless(None)),
            }),
            WindowAction::ToggleMaximize => window.set_maximized(!window.is_maximized()),
            // Carried out with the shared GPU and fonts, or on the window's
            // own state, before any reach here.
            WindowAction::SetFont(_)
            | WindowAction::SetTitle(_)
            | WindowAction::SetTransparent(_) => {}
        }
    }

    /// Keeps `tree`, the accessibility tree of the frame just drawn, and
    /// sends it to the adapter while an assistive technology listens and
    /// it changed since the last one sent.
    fn send_access_tree(&mut self, tree: AccessTree) {
        self.access_tree = tree;
        if !self.access_active {
            return;
        }
        let update = self.access_tree.tree_update(&self.title);
        if self.access_sent.as_ref() == Some(&update) {
            return;
        }
        let sent = update.clone();
        self.access.update_if_active(|| update);
        self.access_sent = Some(sent);
    }
}

impl<A: Application> ApplicationHandler<UserEvent<A::Message>> for Runtime<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let Some(options) = self.first.take() else {
            return;
        };
        let opened = event_loop
            .create_window(options.window_attributes(&monitors(event_loop)))
            .map_err(Error::create_window)
            .and_then(|window| self.set_up_window(event_loop, Arc::new(window), &options));
        if let Err(error) = opened {
            self.error = Some(error);
            event_loop.exit();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent<A::Message>) {
        let _busy = Busy::begin(self.beat.as_ref(), HangPhase::Message);
        match event {
            UserEvent::Access(event) => self.follow_access(event_loop, event),
            UserEvent::Appearance(appearance) => self.follow_appearance(event_loop, appearance),
            UserEvent::Message(message) => {
                self.call_app(event_loop, |app, cx| app.message(cx, message));
            }
            // Waking was the point; `about_to_wait` follows.
            UserEvent::Wake => {}
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: window::WindowId,
        event: WindowEvent,
    ) {
        let id = WindowId(window_id);
        let phase = match event {
            WindowEvent::RedrawRequested => HangPhase::Drawing,
            _ => HangPhase::Input,
        };
        let _busy = Busy::begin(self.beat.as_ref(), phase);
        let Some(open) = self.windows.iter_mut().find(|open| open.id == id) else {
            return;
        };
        open.access.process_event(&open.window, &event);
        if let WindowEvent::RedrawRequested = event {
            self.draw_frame(event_loop, id);
            return;
        }
        if let WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } = event {
            self.follow_window_size(id, &event);
            if let Some(open) = self.find(id) {
                let scale_factor = open.window.scale_factor();
                let size = open.window.inner_size().to_logical::<u32>(scale_factor);
                let size = Size::new(size.width, size.height);
                self.call_app(event_loop, |app, cx| {
                    app.resized(cx, id, size, scale_factor)
                });
            }
            return;
        }
        if let WindowEvent::Moved(position) = event {
            // Maximizing moves a window too; only a window left as it is
            // keeps the place it goes back to.
            if !open.window.is_maximized() {
                open.normal_position = Some(position);
            }
            return;
        }
        if let WindowEvent::ThemeChanged(theme) = event {
            let appearance = Appearance {
                color_scheme: Some(scheme_of(theme)),
                ..self.appearance
            };
            self.follow_appearance(event_loop, appearance);
            return;
        }
        if let Some(input) = open.input.read(&event, open.window.scale_factor()) {
            self.call_app(event_loop, |app, cx| app.input(cx, id, &input));
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let _busy = Busy::begin(self.beat.as_ref(), HangPhase::Waiting);
        let app_deadline = self
            .call_app(event_loop, |app, cx| app.about_to_wait(cx))
            .flatten();
        for id in self.schedule.take_due(Instant::now()) {
            if let Some(open) = self.find(id) {
                open.window.request_redraw();
            }
        }
        let control_flow = match earliest(app_deadline, self.schedule.next_deadline()) {
            Some(deadline) => ControlFlow::WaitUntil(deadline),
            None => ControlFlow::Wait,
        };
        event_loop.set_control_flow(control_flow);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Runs `app`, opening its first window with `options`, until the app
/// calls [`AppContext::exit`] or closes its last window. The app may open
/// more windows with [`AppContext::open_window`]. Must be called on the
/// main thread, and returns only when the app has quit or the first window
/// could not be brought up.
pub fn run<A: Application>(app: A, options: WindowOptions) -> Result<(), Error> {
    let event_loop = EventLoop::<UserEvent<A::Message>>::with_user_event()
        .build()
        .map_err(Error::event_loop)?;
    let appearance = appearance::read();
    let proxy = event_loop.create_proxy();
    appearance::watch(appearance, move |appearance| {
        // A loop that has closed has no app left to tell.
        let _ = proxy.send_event(UserEvent::Appearance(appearance));
    });
    let beat = watchdog::start(app.hang_watch());
    let mut runtime = Runtime {
        app,
        appearance,
        beat,
        clipboard: SystemClipboard::new(),
        error: None,
        first: Some(options),
        panics: PanicRate::default(),
        proxy: event_loop.create_proxy(),
        reduced_motion: None,
        schedule: RedrawSchedule::default(),
        shared: None,
        waker: Waker::new(event_loop.create_proxy()),
        windows: Vec::new(),
    };
    event_loop
        .run_app(&mut runtime)
        .map_err(Error::event_loop)?;
    runtime.error.map_or(Ok(()), Err)
}

/// Makes the Option keys `option_as_alt` names act as Alt in `window`.
#[cfg(target_os = "macos")]
fn set_option_as_alt(window: &Window, option_as_alt: OptionAsAlt) {
    use winit::platform::macos::{OptionAsAlt as WinitOptionAsAlt, WindowExtMacOS};
    window.set_option_as_alt(match option_as_alt {
        OptionAsAlt::Both => WinitOptionAsAlt::Both,
        OptionAsAlt::Left => WinitOptionAsAlt::OnlyLeft,
        OptionAsAlt::None => WinitOptionAsAlt::None,
        OptionAsAlt::Right => WinitOptionAsAlt::OnlyRight,
    });
}

/// Other systems have no Option key.
#[cfg(not(target_os = "macos"))]
fn set_option_as_alt(_window: &Window, _option_as_alt: OptionAsAlt) {}

/// The color scheme winit's `theme` names.
fn scheme_of(theme: Theme) -> ColorScheme {
    match theme {
        Theme::Dark => ColorScheme::Dark,
        Theme::Light => ColorScheme::Light,
    }
}

/// The fonts, for one use; a panic while they were held left them usable.
fn lock(fonts: &Mutex<Fonts>) -> MutexGuard<'_, Fonts> {
    fonts.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The window system's shape for `icon`.
fn winit_cursor(icon: CursorIcon) -> window::CursorIcon {
    match icon {
        CursorIcon::Alias => window::CursorIcon::Alias,
        CursorIcon::ColResize => window::CursorIcon::ColResize,
        CursorIcon::ContextMenu => window::CursorIcon::ContextMenu,
        CursorIcon::Copy => window::CursorIcon::Copy,
        CursorIcon::Crosshair => window::CursorIcon::Crosshair,
        CursorIcon::Default => window::CursorIcon::Default,
        CursorIcon::EResize => window::CursorIcon::EResize,
        CursorIcon::EwResize => window::CursorIcon::EwResize,
        CursorIcon::Grab => window::CursorIcon::Grab,
        CursorIcon::Grabbing => window::CursorIcon::Grabbing,
        CursorIcon::Help => window::CursorIcon::Help,
        CursorIcon::Move => window::CursorIcon::Move,
        CursorIcon::NeResize => window::CursorIcon::NeResize,
        CursorIcon::NeswResize => window::CursorIcon::NeswResize,
        CursorIcon::NResize => window::CursorIcon::NResize,
        CursorIcon::NotAllowed => window::CursorIcon::NotAllowed,
        CursorIcon::NsResize => window::CursorIcon::NsResize,
        CursorIcon::NwResize => window::CursorIcon::NwResize,
        CursorIcon::NwseResize => window::CursorIcon::NwseResize,
        CursorIcon::Pointer => window::CursorIcon::Pointer,
        CursorIcon::Progress => window::CursorIcon::Progress,
        CursorIcon::RowResize => window::CursorIcon::RowResize,
        CursorIcon::SeResize => window::CursorIcon::SeResize,
        CursorIcon::SResize => window::CursorIcon::SResize,
        CursorIcon::SwResize => window::CursorIcon::SwResize,
        CursorIcon::Text => window::CursorIcon::Text,
        CursorIcon::Wait => window::CursorIcon::Wait,
        CursorIcon::WResize => window::CursorIcon::WResize,
        CursorIcon::ZoomIn => window::CursorIcon::ZoomIn,
        CursorIcon::ZoomOut => window::CursorIcon::ZoomOut,
    }
}

/// The pointer shape each window ends a callback with, from the shapes
/// `requested` in it in order: the last that is not the default, else the
/// default, so a widget the pointer left never undoes the shape of the one
/// it entered.
fn pointer_shapes(requested: &[(WindowId, CursorIcon)]) -> Vec<(WindowId, CursorIcon)> {
    let mut shapes: Vec<(WindowId, CursorIcon)> = Vec::new();
    for &(id, icon) in requested {
        match shapes.iter_mut().find(|(window, _)| *window == id) {
            Some(shape) if icon != CursorIcon::Default => shape.1 = icon,
            Some(_) => {}
            None => shapes.push((id, icon)),
        }
    }
    shapes
}

/// What to do after a callback in which the app asked to close `closed`,
/// redraw `redraws`, and quit when `exit`, while `open` windows are open:
/// close each open window asked for once, redraw each open window asked
/// for once unless it is closing, and quit when asked or when the last
/// window closed and `exit_on_last` holds.
fn settle(
    open: &[WindowId],
    closed: &[WindowId],
    redraws: &[WindowId],
    exit: bool,
    exit_on_last: bool,
) -> Settlement {
    let mut close: Vec<WindowId> = Vec::new();
    for id in closed {
        if open.contains(id) && !close.contains(id) {
            close.push(*id);
        }
    }
    let mut redraw: Vec<WindowId> = Vec::new();
    for id in redraws {
        if open.contains(id) && !close.contains(id) && !redraw.contains(id) {
            redraw.push(*id);
        }
    }
    let closed_last = !close.is_empty() && close.len() == open.len();
    Settlement {
        close,
        exit: exit || (closed_last && exit_on_last),
        redraw,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn id(raw: u64) -> WindowId {
        WindowId(window::WindowId::from(raw))
    }

    #[test]
    fn test_requests_reach_only_open_windows_and_each_once() {
        let open = [id(1), id(2)];
        let settled = settle(
            &open,
            &[id(2), id(2), id(9)],
            &[id(1), id(2), id(1), id(9)],
            false,
            true,
        );
        assert_eq!(settled.close, [id(2)], "the unknown window is not closed");
        assert_eq!(
            settled.redraw,
            [id(1)],
            "no redraw of a closing or unknown window"
        );
        assert!(!settled.exit, "a window is still open");
    }

    #[test]
    fn test_a_pointer_shape_beats_the_default_whichever_widget_asked_first() {
        let text = CursorIcon::Text;
        let default = CursorIcon::Default;
        assert_eq!(
            pointer_shapes(&[(id(1), text), (id(1), default)]),
            [(id(1), text)],
            "the field entered asked first"
        );
        assert_eq!(
            pointer_shapes(&[(id(1), default), (id(1), text), (id(2), default)]),
            [(id(1), text), (id(2), default)],
            "each window apart"
        );
        assert_eq!(
            pointer_shapes(&[(id(1), text), (id(1), CursorIcon::ColResize)]),
            [(id(1), CursorIcon::ColResize)],
            "the last of two shapes"
        );
    }

    #[test]
    fn test_closing_the_last_window_quits_only_when_the_app_wants_it() {
        let open = [id(1), id(2)];
        assert!(settle(&open, &[id(1), id(2)], &[], false, true).exit);
        assert!(!settle(&open, &[id(1), id(2)], &[], false, false).exit);
        assert!(
            settle(&open, &[], &[], true, false).exit,
            "an explicit exit"
        );
        assert!(
            !settle(&[], &[id(1)], &[], false, true).exit,
            "closing nothing when none is open quits nothing"
        );
    }
}
