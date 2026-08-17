#![deny(unsafe_op_in_unsafe_fn)]

use std::{
    cell::{Cell, OnceCell, RefCell},
    convert::Infallible,
    path::Path,
    sync::mpsc::TryRecvError,
    time::Instant,
};

use app_config::AppConfig;
use app_core::{
    App, Button, Command, DISPLAY_SIZE, Event, NetworkStatus, RECOMMENDED_SCRATCH_PIXELS, Screen,
    TouchPoint,
};
use app_runtime::{
    hifi::{
        HifiDriver,
        worker::{
            self as runtime_worker, Request as RuntimeWorkerRequest,
            Response as RuntimeWorkerResponse, Worker as RuntimeWorker,
        },
    },
    host_tcp::HostTcpConnector,
    lpec::{Error as LpecError, LpecSessionHifi},
};
use embedded_graphics::{
    Pixel,
    draw_target::DrawTarget,
    geometry::OriginDimensions,
    pixelcolor::{Rgb565, RgbColor},
    prelude::*,
    primitives::{Circle, PrimitiveStyle},
};
use image::{ColorType, ImageEncoder, codecs::png::PngEncoder};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSAutoresizingMaskOptions,
    NSBackingStoreType, NSButton, NSClickGestureRecognizer, NSImage, NSImageScaling, NSImageView,
    NSPopUpButton, NSTextField, NSView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSData, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString, NSTimer, ns_string,
};

const REFRESH_INTERVAL_SECONDS: f64 = 0.05;
const TAP_FULL_MS: u64 = 1_000;
const TAP_FADE_MS: u64 = 700;
const WINDOW_MARGIN: f64 = 24.0;
const SIDE_GAP: f64 = 24.0;
const SIDE_WIDTH: f64 = 216.0;
const BUTTON_WIDTH: f64 = 216.0;
const BUTTON_HEIGHT: f64 = 32.0;
const BUTTON_SPACING: f64 = 40.0;
/// Size of one key in the 3x3 directional-pad cross.
const PAD_KEY_WIDTH: f64 = 68.0;
const PAD_KEY_HEIGHT: f64 = 30.0;
/// Width of each of the two control buttons sitting under the pad.
const CONTROL_BUTTON_WIDTH: f64 = 104.0;
const DEBUG_LABEL_HEIGHT: f64 = 120.0;
/// Height the side panel needs for the full control stack. The window grows to
/// this even when the display is shorter, so nothing overlaps the debug text.
const SIDE_MIN_HEIGHT: f64 = 620.0;

// Arrow keys as AppKit key equivalents, so the pad is drivable from the
// keyboard. These are the NSUpArrowFunctionKey family from NSEvent.h.
const KEY_ARROW_UP: &str = "\u{f700}";
const KEY_ARROW_DOWN: &str = "\u{f701}";
const KEY_ARROW_LEFT: &str = "\u{f702}";
const KEY_ARROW_RIGHT: &str = "\u{f703}";
const HIFI_STATUS_POLL_MS: u64 = 2_000;

type SimHifiError = LpecError<std::io::Error>;

fn start_runtime_worker() -> RuntimeWorker<SimHifiError> {
    let endpoint = AppConfig::load_local_or_default().linn_lpec_endpoint;
    let hifi = LpecSessionHifi::new(HostTcpConnector::new(), endpoint);
    let command_hifi = LpecSessionHifi::new(HostTcpConnector::new(), endpoint);
    runtime_worker::start(HifiDriver::new(hifi, HIFI_STATUS_POLL_MS), command_hifi)
}

#[derive(Clone, Debug)]
struct Framebuffer {
    size: Size,
    pixels: Vec<Rgb565>,
}

impl Framebuffer {
    fn new(size: Size) -> Self {
        Self {
            size,
            pixels: vec![Rgb565::BLACK; (size.width * size.height) as usize],
        }
    }

    fn to_png(&self) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(self.pixels.len() * 4);

        for color in &self.pixels {
            rgba.push(scale_channel(color.r(), Rgb565::MAX_R));
            rgba.push(scale_channel(color.g(), Rgb565::MAX_G));
            rgba.push(scale_channel(color.b(), Rgb565::MAX_B));
            // Every pixel is opaque: unlike the round board this replaced,
            // the Kode Dot panel has no masked-off corners.
            rgba.push(255);
        }

        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(
                &rgba,
                self.size.width,
                self.size.height,
                ColorType::Rgba8.into(),
            )
            .expect("framebuffer PNG encoding should succeed");
        png
    }

    fn copy_from(&mut self, other: &Self) {
        assert_eq!(self.size, other.size);
        self.pixels.copy_from_slice(&other.pixels);
    }
}

impl DrawTarget for Framebuffer {
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if point.x < 0 || point.y < 0 {
                continue;
            }

            let x = point.x as u32;
            let y = point.y as u32;

            if x >= self.size.width || y >= self.size.height {
                continue;
            }

            let index = (y * self.size.width + x) as usize;
            self.pixels[index] = color;
        }

        Ok(())
    }

    /// Fills are written exactly as asked.
    ///
    /// This used to snap fills out to an even-aligned window of at least two
    /// scanlines, mirroring how the round board's driver drove its panel with
    /// no framebuffer to compose in. That emulation actively destroyed
    /// content: a glyph background fill starting on an odd row grows upward
    /// and paints over the row above it, which silently ate the bottom row of
    /// any glyph ending on an even row — visible as bars and stems rendering
    /// about a pixel too thin.
    ///
    /// The Kode Dot has 32 MB of PSRAM, so its driver will compose a full
    /// frame in memory and blit it, and can align the blit window without
    /// disturbing what it contains. Modelling the old constraint here would
    /// mean designing against a limitation this hardware does not have. See
    /// docs/ARCHITECTURE.md.
    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        self.pixels.fill(color);
        Ok(())
    }
}

impl OriginDimensions for Framebuffer {
    fn size(&self) -> Size {
        self.size
    }
}

#[derive(Debug)]
struct NativeSimulator {
    app: App,
    app_framebuffer: Framebuffer,
    output_framebuffer: Framebuffer,
    scratch: Vec<Rgb565>,
    app_frame_dirty: bool,
    output_frame_dirty: bool,
    runtime_worker: RuntimeWorker<SimHifiError>,
    started_at: Instant,
    manual_time_offset_ms: u64,
    tap_highlight: Option<TapHighlight>,
    render_stats: RenderStats,
}

impl NativeSimulator {
    fn new() -> Self {
        let mut app = App::new_on_screen(default_screen());
        let _ = app.update(Event::NetworkStatus(NetworkStatus::Online));

        Self {
            app,
            app_framebuffer: Framebuffer::new(DISPLAY_SIZE),
            output_framebuffer: Framebuffer::new(DISPLAY_SIZE),
            scratch: vec![Rgb565::BLACK; RECOMMENDED_SCRATCH_PIXELS],
            app_frame_dirty: true,
            output_frame_dirty: true,
            runtime_worker: start_runtime_worker(),
            started_at: Instant::now(),
            manual_time_offset_ms: 0,
            tap_highlight: None,
            render_stats: RenderStats::new(),
        }
    }

    fn update(&mut self, event: Event) {
        let outcome = self.app.update(event);
        if let Some(command) = outcome.command {
            self.handle_command(command);
        }
        if outcome.render_requested {
            self.app_frame_dirty = true;
            self.output_frame_dirty = true;
            self.render_stats.record_core_request();
        }
        self.sync_runtime_screen();
    }

    fn handle_command(&mut self, command: Command) {
        let Command::Hifi(command) = command;
        if let Err(error) = self
            .runtime_worker
            .send(RuntimeWorkerRequest::Command(command))
        {
            eprintln!("failed to queue app command {command:?}: {error:?}");
        }
    }

    fn tick(&mut self) {
        let uptime_ms = self.uptime_ms();
        self.drain_runtime_worker();
        self.update(Event::Tick { uptime_ms });
        self.sync_runtime_screen();
        if let Err(error) = self
            .runtime_worker
            .send(RuntimeWorkerRequest::Tick { uptime_ms })
        {
            eprintln!("failed to queue runtime tick: {error:?}");
        }
        self.render_stats.update_sample();
    }

    fn sync_runtime_screen(&self) {
        let request = RuntimeWorkerRequest::SyncScreen {
            screen: self.app.screen(),
            uptime_ms: self.uptime_ms(),
        };
        if let Err(error) = self.runtime_worker.send(request) {
            eprintln!("failed to sync runtime screen: {error:?}");
        }
    }

    fn drain_runtime_worker(&mut self) {
        loop {
            match self.runtime_worker.try_recv() {
                Ok(RuntimeWorkerResponse::Event(event)) => self.update(event),
                Ok(RuntimeWorkerResponse::Error(error)) => {
                    eprintln!("hifi runtime error: {error:?}");
                }
                Ok(RuntimeWorkerResponse::Disconnected) => break,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
    }

    fn advance_by(&mut self, delta_ms: u64) {
        self.manual_time_offset_ms = self.manual_time_offset_ms.saturating_add(delta_ms);
        self.tick();
    }

    fn tap(&mut self, point: TouchPoint) {
        let uptime_ms = self.uptime_ms();
        self.tap_highlight = Some(TapHighlight {
            point,
            started_at_ms: uptime_ms,
        });
        self.output_frame_dirty = true;
        self.update(Event::Tick { uptime_ms });
        self.update(Event::TouchDown(point));
    }

    fn uptime_ms(&self) -> u64 {
        let real_elapsed_ms = self.started_at.elapsed().as_millis() as u64;
        real_elapsed_ms.saturating_add(self.manual_time_offset_ms)
    }

    fn redraw_png_if_needed(&mut self) -> Option<Vec<u8>> {
        if !self.app_frame_dirty && !self.output_frame_dirty && self.tap_highlight.is_none() {
            return None;
        }

        if self.app_frame_dirty {
            self.app
                .render(&mut self.app_framebuffer, &mut self.scratch)
                .expect("app rendering should succeed");
            self.app_frame_dirty = false;
            self.render_stats.record_core_frame_rendered();
        }

        self.output_framebuffer.copy_from(&self.app_framebuffer);
        self.render_tap_highlight();
        self.output_frame_dirty = false;
        self.render_stats.record_simulator_redraw();
        self.render_stats.update_sample();
        Some(self.output_framebuffer.to_png())
    }

    fn render_tap_highlight(&mut self) {
        let Some(tap) = self.tap_highlight else {
            return;
        };

        let Some(alpha) = tap_alpha(self.uptime_ms().saturating_sub(tap.started_at_ms)) else {
            self.tap_highlight = None;
            self.output_frame_dirty = true;
            return;
        };

        let r = 8 + ((23_u16 * alpha as u16) / 255) as u8;
        let g = 14 + ((28_u16 * alpha as u16) / 255) as u8;
        let b = 4 + ((10_u16 * alpha as u16) / 255) as u8;
        Circle::with_center(Point::new(tap.point.x, tap.point.y), 18)
            .into_styled(PrimitiveStyle::with_fill(Rgb565::new(r, g, b)))
            .draw(&mut self.output_framebuffer)
            .expect("simulator overlay rendering should succeed");
    }

    fn debug_text(&self) -> String {
        self.render_stats.debug_text()
    }
}

fn default_screen() -> Screen {
    match std::env::var("APP_SCREEN").as_deref() {
        Ok("stopwatch" | "stop-watch" | "stop_watch") => Screen::Stopwatch,
        Ok("hifi" | "hifi-control" | "hifi_control") => Screen::HifiControl,
        _ => Screen::Launcher,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TapHighlight {
    point: TouchPoint,
    started_at_ms: u64,
}

#[derive(Debug)]
struct RenderStats {
    core_requests_total: u64,
    core_frames_total: u64,
    simulator_redraws_total: u64,
    sample_started_at: Instant,
    sample_core_requests: u64,
    sample_core_frames: u64,
    sample_simulator_redraws: u64,
    core_request_hz: f64,
    core_frame_hz: f64,
    simulator_redraw_hz: f64,
}

impl RenderStats {
    fn new() -> Self {
        Self {
            core_requests_total: 0,
            core_frames_total: 0,
            simulator_redraws_total: 0,
            sample_started_at: Instant::now(),
            sample_core_requests: 0,
            sample_core_frames: 0,
            sample_simulator_redraws: 0,
            core_request_hz: 0.0,
            core_frame_hz: 0.0,
            simulator_redraw_hz: 0.0,
        }
    }

    fn record_core_request(&mut self) {
        self.core_requests_total = self.core_requests_total.saturating_add(1);
        self.sample_core_requests = self.sample_core_requests.saturating_add(1);
    }

    fn record_core_frame_rendered(&mut self) {
        self.core_frames_total = self.core_frames_total.saturating_add(1);
        self.sample_core_frames = self.sample_core_frames.saturating_add(1);
    }

    fn record_simulator_redraw(&mut self) {
        self.simulator_redraws_total = self.simulator_redraws_total.saturating_add(1);
        self.sample_simulator_redraws = self.sample_simulator_redraws.saturating_add(1);
    }

    fn update_sample(&mut self) {
        let elapsed = self.sample_started_at.elapsed();
        let elapsed_secs = elapsed.as_secs_f64();

        if elapsed_secs < 1.0 {
            return;
        }

        self.core_request_hz = self.sample_core_requests as f64 / elapsed_secs;
        self.core_frame_hz = self.sample_core_frames as f64 / elapsed_secs;
        self.simulator_redraw_hz = self.sample_simulator_redraws as f64 / elapsed_secs;
        self.sample_started_at = Instant::now();
        self.sample_core_requests = 0;
        self.sample_core_frames = 0;
        self.sample_simulator_redraws = 0;
    }

    fn debug_text(&self) -> String {
        format!(
            "Debug\ncore requests: {}\ncore req/s: {:.1}\ncore frames: {}\ncore fps: {:.1}\nsim redraws: {}\nsim fps: {:.1}",
            self.core_requests_total,
            self.core_request_hz,
            self.core_frames_total,
            self.core_frame_hz,
            self.simulator_redraws_total,
            self.simulator_redraw_hz
        )
    }
}

#[derive(Debug)]
struct AppDelegateIvars {
    window: OnceCell<Retained<NSWindow>>,
    image_view: OnceCell<Retained<NSImageView>>,
    side_panel: OnceCell<Retained<NSView>>,
    zoom_button: OnceCell<Retained<NSButton>>,
    debug_label: OnceCell<Retained<NSTextField>>,
    tap_recognizer: OnceCell<Retained<NSClickGestureRecognizer>>,
    timer: OnceCell<Retained<NSTimer>>,
    simulator: RefCell<NativeSimulator>,
    zoomed: Cell<bool>,
}

impl Default for AppDelegateIvars {
    fn default() -> Self {
        Self {
            window: OnceCell::new(),
            image_view: OnceCell::new(),
            side_panel: OnceCell::new(),
            zoom_button: OnceCell::new(),
            debug_label: OnceCell::new(),
            tap_recognizer: OnceCell::new(),
            timer: OnceCell::new(),
            simulator: RefCell::new(NativeSimulator::new()),
            zoomed: Cell::new(false),
        }
    }
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = AppDelegateIvars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            let mtm = self.mtm();
            let app = NSApplication::sharedApplication(mtm);
            app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

            let style_mask = NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable;
            let initial_layout = layout_for_backing_scale(2.0, false);

            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    NSRect::new(NSPoint::new(0.0, 0.0), initial_layout.content_size),
                    style_mask,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            unsafe { window.setReleasedWhenClosed(false) };
            window.setTitle(ns_string!("Kode Linn Remote Simulator"));
            window.setDelegate(Some(ProtocolObject::from_ref(self)));

            let layout = layout_for_window(&window, false);
            window.setContentMinSize(layout.content_size);
            window.setContentSize(layout.content_size);
            window.center();

            let content_view = window.contentView().expect("window should have a content view");
            let image = self.render_image();
            let image_view = NSImageView::imageViewWithImage(&image, mtm);
            image_view.setFrame(layout.display_frame);
            image_view.setImageScaling(NSImageScaling::ScaleAxesIndependently);
            let tap_recognizer = unsafe {
                NSClickGestureRecognizer::initWithTarget_action(
                    NSClickGestureRecognizer::alloc(mtm),
                    Some(self.as_ref()),
                    Some(sel!(displayTapped:)),
                )
            };
            tap_recognizer.setNumberOfClicksRequired(1);
            image_view.addGestureRecognizer(&tap_recognizer);
            content_view.addSubview(&image_view);

            let side_panel = NSView::initWithFrame(NSView::alloc(mtm), layout.side_frame);
            content_view.addSubview(&side_panel);

            // Controls stack downward from the top of the side panel.
            let mut stack = ControlStack::new(layout.side_inner_height);

            let zoom_button = add_button(
                &side_panel,
                "Zoom 2x",
                0.0,
                stack.next(BUTTON_HEIGHT, BUTTON_SPACING),
                BUTTON_WIDTH,
                sel!(toggleZoom:),
                self,
                mtm,
            );
            add_button(
                &side_panel,
                "Advance +1s",
                0.0,
                stack.next(BUTTON_HEIGHT, BUTTON_SPACING),
                BUTTON_WIDTH,
                sel!(advanceSecond:),
                self,
                mtm,
            );
            add_network_status_popup(
                &side_panel,
                0.0,
                stack.next(BUTTON_HEIGHT, BUTTON_SPACING * 1.4),
                self,
                mtm,
            );

            // Directional pad, laid out as the cross it is on the hardware.
            // Each key also answers to its arrow key so the pad can be driven
            // from the keyboard while iterating on a layout.
            let pad_center_x = (SIDE_WIDTH - PAD_KEY_WIDTH) / 2.0;
            let pad_up_y = stack.next(PAD_KEY_HEIGHT, PAD_KEY_HEIGHT + 4.0);
            let pad_middle_y = stack.next(PAD_KEY_HEIGHT, PAD_KEY_HEIGHT + 4.0);
            let pad_down_y = stack.next(PAD_KEY_HEIGHT, PAD_KEY_HEIGHT + 16.0);

            add_pad_key(
                &side_panel,
                "Up",
                pad_center_x,
                pad_up_y,
                KEY_ARROW_UP,
                sel!(padUp:),
                self,
                mtm,
            );
            add_pad_key(
                &side_panel,
                "Left",
                pad_center_x - PAD_KEY_WIDTH,
                pad_middle_y,
                KEY_ARROW_LEFT,
                sel!(padLeft:),
                self,
                mtm,
            );
            add_pad_key(
                &side_panel,
                "Right",
                pad_center_x + PAD_KEY_WIDTH,
                pad_middle_y,
                KEY_ARROW_RIGHT,
                sel!(padRight:),
                self,
                mtm,
            );
            add_pad_key(
                &side_panel,
                "Down",
                pad_center_x,
                pad_down_y,
                KEY_ARROW_DOWN,
                sel!(padDown:),
                self,
                mtm,
            );

            // The two control buttons flanking the pad. Return confirms and
            // Escape goes up a level, matching the on-screen labels.
            let control_y = stack.next(BUTTON_HEIGHT, BUTTON_SPACING);
            let select_button = add_button(
                &side_panel,
                "Select \u{21b5}",
                0.0,
                control_y,
                CONTROL_BUTTON_WIDTH,
                sel!(controlSelect:),
                self,
                mtm,
            );
            select_button.setKeyEquivalent(ns_string!("\r"));
            let back_button = add_button(
                &side_panel,
                "Back esc",
                SIDE_WIDTH - CONTROL_BUTTON_WIDTH,
                control_y,
                CONTROL_BUTTON_WIDTH,
                sel!(controlBack:),
                self,
                mtm,
            );
            back_button.setKeyEquivalent(ns_string!("\u{1b}"));

            let debug_label = NSTextField::wrappingLabelWithString(
                &NSString::from_str(&self.debug_text()),
                mtm,
            );
            debug_label.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(SIDE_WIDTH, DEBUG_LABEL_HEIGHT),
            ));
            side_panel.addSubview(&debug_label);

            let timer = unsafe {
                NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                    REFRESH_INTERVAL_SECONDS,
                    self.as_ref(),
                    sel!(timerTick:),
                    None,
                    true,
                )
            };

            window.makeKeyAndOrderFront(None);
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);

            self.ivars()
                .image_view
                .set(image_view)
                .expect("image view should be stored once");
            self.ivars()
                .side_panel
                .set(side_panel)
                .expect("side panel should be stored once");
            self.ivars()
                .zoom_button
                .set(zoom_button)
                .expect("zoom button should be stored once");
            self.ivars()
                .debug_label
                .set(debug_label)
                .expect("debug label should be stored once");
            self.ivars()
                .tap_recognizer
                .set(tap_recognizer)
                .expect("tap recognizer should be stored once");
            self.ivars()
                .timer
                .set(timer)
                .expect("timer should be stored once");
            self.ivars()
                .window
                .set(window)
                .expect("window should be stored once");

        }
    }

    unsafe impl NSWindowDelegate for Delegate {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            if let Some(timer) = self.ivars().timer.get() {
                timer.invalidate();
            }
            NSApplication::sharedApplication(self.mtm()).terminate(None);
        }
    }

    impl Delegate {
        #[unsafe(method(timerTick:))]
        fn timer_tick(&self, _sender: &NSTimer) {
            self.ivars().simulator.borrow_mut().tick();
            self.refresh_image();
        }

        #[unsafe(method(toggleZoom:))]
        fn toggle_zoom(&self, _sender: &AnyObject) {
            self.ivars().zoomed.set(!self.ivars().zoomed.get());
            self.apply_zoom_layout();
        }

        #[unsafe(method(advanceSecond:))]
        fn advance_second(&self, _sender: &AnyObject) {
            self.ivars().simulator.borrow_mut().advance_by(1_000);
            self.refresh_image();
        }

        #[unsafe(method(padUp:))]
        fn pad_up(&self, _sender: &AnyObject) {
            self.send_event(Event::ButtonPressed(Button::Up));
        }

        #[unsafe(method(padDown:))]
        fn pad_down(&self, _sender: &AnyObject) {
            self.send_event(Event::ButtonPressed(Button::Down));
        }

        #[unsafe(method(padLeft:))]
        fn pad_left(&self, _sender: &AnyObject) {
            self.send_event(Event::ButtonPressed(Button::Left));
        }

        #[unsafe(method(padRight:))]
        fn pad_right(&self, _sender: &AnyObject) {
            self.send_event(Event::ButtonPressed(Button::Right));
        }

        #[unsafe(method(controlSelect:))]
        fn control_select(&self, _sender: &AnyObject) {
            self.send_event(Event::ButtonPressed(Button::Select));
        }

        #[unsafe(method(controlBack:))]
        fn control_back(&self, _sender: &AnyObject) {
            self.send_event(Event::ButtonPressed(Button::Back));
        }

        #[unsafe(method(displayTapped:))]
        fn display_tapped(&self, sender: &NSClickGestureRecognizer) {
            let Some(view) = sender.view() else {
                return;
            };

            let point = sender.locationInView(Some(&view));
            let view_frame = view.frame();
            let x = ((point.x / view_frame.size.width) * DISPLAY_SIZE.width as f64).round() as i32;
            let y = (((view_frame.size.height - point.y) / view_frame.size.height)
                * DISPLAY_SIZE.height as f64)
                .round() as i32;

            if x < 0 || y < 0 || x >= DISPLAY_SIZE.width as i32 || y >= DISPLAY_SIZE.height as i32 {
                return;
            }

            self.ivars()
                .simulator
                .borrow_mut()
                .tap(TouchPoint { x, y });
            self.refresh_image();
        }

        #[unsafe(method(networkStatusChanged:))]
        fn network_status_changed(&self, sender: &NSPopUpButton) {
            let status = match sender.indexOfSelectedItem() {
                0 => NetworkStatus::Online,
                1 => NetworkStatus::Connecting,
                2 => NetworkStatus::Offline,
                _ => return,
            };
            self.send_event(Event::NetworkStatus(status));
        }

    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(AppDelegateIvars::default());
        unsafe { msg_send![super(this), init] }
    }

    fn send_event(&self, event: Event) {
        self.ivars().simulator.borrow_mut().update(event);
        self.refresh_image();
    }

    fn refresh_image(&self) {
        if let Some(image) = self.render_image_if_needed() {
            let image_view = self
                .ivars()
                .image_view
                .get()
                .expect("image view should exist after launch");
            image_view.setImage(Some(&image));
        }
        self.refresh_debug_label();
    }

    fn render_image(&self) -> Retained<NSImage> {
        let png = self
            .ivars()
            .simulator
            .borrow_mut()
            .redraw_png_if_needed()
            .expect("initial simulator render should produce an image");
        image_from_png(&png)
    }

    fn render_image_if_needed(&self) -> Option<Retained<NSImage>> {
        let png = self.ivars().simulator.borrow_mut().redraw_png_if_needed()?;
        Some(image_from_png(&png))
    }

    fn refresh_debug_label(&self) {
        let Some(debug_label) = self.ivars().debug_label.get() else {
            return;
        };

        debug_label.setStringValue(&NSString::from_str(&self.debug_text()));
    }

    fn debug_text(&self) -> String {
        self.ivars().simulator.borrow().debug_text()
    }

    fn apply_zoom_layout(&self) {
        let window = self
            .ivars()
            .window
            .get()
            .expect("window should exist after launch");
        let image_view = self
            .ivars()
            .image_view
            .get()
            .expect("image view should exist after launch");
        let side_panel = self
            .ivars()
            .side_panel
            .get()
            .expect("side panel should exist after launch");
        let zoom_button = self
            .ivars()
            .zoom_button
            .get()
            .expect("zoom button should exist after launch");

        let zoomed = self.ivars().zoomed.get();
        let layout = layout_for_window(window, zoomed);
        window.setContentMinSize(layout.content_size);
        window.setContentSize(layout.content_size);
        image_view.setFrame(layout.display_frame);
        side_panel.setFrame(layout.side_frame);

        if zoomed {
            zoom_button.setTitle(&NSString::from_str("Native scale"));
        } else {
            zoom_button.setTitle(&NSString::from_str("Zoom 2x"));
        }
    }
}

fn image_from_png(png: &[u8]) -> Retained<NSImage> {
    let data = NSData::with_bytes(png);
    NSImage::initWithData(NSImage::alloc(), &data).expect("AppKit should decode simulator PNG")
}

#[allow(clippy::too_many_arguments)]
fn add_button(
    content_view: &NSView,
    title: &str,
    x: f64,
    y: f64,
    width: f64,
    action: objc2::runtime::Sel,
    target: &Delegate,
    mtm: MainThreadMarker,
) -> Retained<NSButton> {
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target.as_ref()),
            Some(action),
            mtm,
        )
    };
    button.setFrame(NSRect::new(
        NSPoint::new(x, y),
        NSSize::new(width, BUTTON_HEIGHT),
    ));
    button.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
    content_view.addSubview(&button);
    button
}

/// One key of the directional pad, bound to its matching arrow key.
#[allow(clippy::too_many_arguments)]
fn add_pad_key(
    content_view: &NSView,
    title: &str,
    x: f64,
    y: f64,
    key_equivalent: &str,
    action: objc2::runtime::Sel,
    target: &Delegate,
    mtm: MainThreadMarker,
) -> Retained<NSButton> {
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target.as_ref()),
            Some(action),
            mtm,
        )
    };
    button.setFrame(NSRect::new(
        NSPoint::new(x, y),
        NSSize::new(PAD_KEY_WIDTH, PAD_KEY_HEIGHT),
    ));
    button.setKeyEquivalent(&NSString::from_str(key_equivalent));
    button.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
    content_view.addSubview(&button);
    button
}

fn add_network_status_popup(
    content_view: &NSView,
    x: f64,
    y: f64,
    target: &Delegate,
    mtm: MainThreadMarker,
) -> Retained<NSPopUpButton> {
    let popup = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        NSRect::new(NSPoint::new(x, y), NSSize::new(BUTTON_WIDTH, BUTTON_HEIGHT)),
        false,
    );
    popup.addItemWithTitle(&NSString::from_str("Online"));
    popup.addItemWithTitle(&NSString::from_str("Connecting"));
    popup.addItemWithTitle(&NSString::from_str("Offline"));
    popup.selectItemAtIndex(0);
    unsafe {
        popup.setTarget(Some(target.as_ref()));
        popup.setAction(Some(sel!(networkStatusChanged:)));
    }
    popup.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
    content_view.addSubview(&popup);
    popup
}

/// Lays controls out top-down inside the side panel.
///
/// AppKit's origin is bottom-left, so "next" walks the cursor downward and
/// returns the origin y for a control of the given height.
struct ControlStack {
    cursor_y: f64,
}

impl ControlStack {
    fn new(side_inner_height: f64) -> Self {
        Self {
            cursor_y: side_inner_height,
        }
    }

    fn next(&mut self, height: f64, advance: f64) -> f64 {
        self.cursor_y -= height;
        let y = self.cursor_y;
        self.cursor_y -= advance - height;
        y
    }
}

#[derive(Clone, Copy, Debug)]
struct Layout {
    content_size: NSSize,
    display_frame: NSRect,
    side_frame: NSRect,
    side_inner_height: f64,
}

fn layout_for_window(window: &NSWindow, zoomed: bool) -> Layout {
    layout_for_backing_scale(window.backingScaleFactor().max(1.0), zoomed)
}

fn layout_for_backing_scale(backing_scale: f64, zoomed: bool) -> Layout {
    // The Kode Dot panel is 410 x 502 — portrait, not square — so width and
    // height are tracked separately rather than sharing one dimension.
    let native_width = DISPLAY_SIZE.width as f64 / backing_scale;
    let native_height = DISPLAY_SIZE.height as f64 / backing_scale;
    let zoomed_width = native_width * 2.0;
    let zoomed_height = native_height * 2.0;
    let (display_width, display_height) = if zoomed {
        (zoomed_width, zoomed_height)
    } else {
        (native_width, native_height)
    };

    // The 2x footprint is always reserved so toggling zoom never resizes the
    // window or shuffles the controls.
    let content_width = WINDOW_MARGIN + zoomed_width + SIDE_GAP + SIDE_WIDTH + WINDOW_MARGIN;
    let inner_height = zoomed_height.max(SIDE_MIN_HEIGHT);
    let content_height = WINDOW_MARGIN * 2.0 + inner_height;

    let display_x = WINDOW_MARGIN + (zoomed_width - display_width) / 2.0;
    let display_y = WINDOW_MARGIN + (inner_height - display_height) / 2.0;
    let side_x = WINDOW_MARGIN + zoomed_width + SIDE_GAP;

    Layout {
        content_size: NSSize::new(content_width, content_height),
        display_frame: NSRect::new(
            NSPoint::new(display_x, display_y),
            NSSize::new(display_width, display_height),
        ),
        side_frame: NSRect::new(
            NSPoint::new(side_x, WINDOW_MARGIN),
            NSSize::new(SIDE_WIDTH, inner_height),
        ),
        side_inner_height: inner_height,
    }
}

fn scale_channel(value: u8, max: u8) -> u8 {
    ((value as u16 * 255) / max as u16) as u8
}

fn tap_alpha(age_ms: u64) -> Option<u8> {
    if age_ms <= TAP_FULL_MS {
        Some(255)
    } else if age_ms >= TAP_FULL_MS + TAP_FADE_MS {
        None
    } else {
        let fade_age = age_ms - TAP_FULL_MS;
        let remaining = TAP_FADE_MS - fade_age;
        Some(((remaining * 255) / TAP_FADE_MS) as u8)
    }
}

/// Renders every screen to a PNG and exits, without opening a window.
///
/// Design iteration on hardware that has not shipped yet mostly means looking
/// at layouts, and a window is an awkward thing to diff or attach to a review.
/// This writes the same framebuffer the panel would receive.
fn write_snapshots(directory: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;

    // Each entry is a file stem plus the button presses needed to reach that
    // view from a freshly opened screen.
    let views: &[(&str, Screen, &[Button])] = &[
        ("launcher", Screen::Launcher, &[]),
        ("launcher-focused", Screen::Launcher, &[Button::Down]),
        ("stopwatch", Screen::Stopwatch, &[]),
        ("stopwatch-focused", Screen::Stopwatch, &[Button::Right]),
        ("hifi-status", Screen::HifiControl, &[]),
        (
            "hifi-status-focused",
            Screen::HifiControl,
            &[Button::Right, Button::Right],
        ),
        (
            "hifi-pins",
            Screen::HifiControl,
            &[Button::Down, Button::Down],
        ),
        (
            "hifi-volume",
            Screen::HifiControl,
            &[
                Button::Down,
                Button::Down,
                Button::Down,
                Button::Down,
                Button::Down,
            ],
        ),
    ];

    let mut scratch = vec![Rgb565::BLACK; RECOMMENDED_SCRATCH_PIXELS];

    for (name, screen, presses) in views {
        let mut app = App::new_on_screen(*screen);
        let _ = app.update(Event::NetworkStatus(NetworkStatus::Online));
        let _ = app.update(Event::Tick { uptime_ms: 6_000 });
        for button in *presses {
            let _ = app.update(Event::ButtonPressed(*button));
        }

        let mut framebuffer = Framebuffer::new(DISPLAY_SIZE);
        app.render(&mut framebuffer, &mut scratch)
            .expect("snapshot rendering should succeed");

        let path = directory.join(format!("{name}.png"));
        std::fs::write(&path, framebuffer.to_png())?;
        println!("wrote {}", path.display());
    }

    Ok(())
}

fn main() {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--snapshot") {
        let directory = args.next().unwrap_or_else(|| "target/snapshots".to_owned());
        write_snapshots(Path::new(&directory)).expect("writing snapshots should succeed");
        return;
    }

    let mtm = MainThreadMarker::new().expect("simulator must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    let delegate = Delegate::new(mtm);

    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
}
