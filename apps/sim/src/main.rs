#![deny(unsafe_op_in_unsafe_fn)]

use std::{
    cell::{Cell, OnceCell, RefCell},
    convert::Infallible,
    time::Instant,
};

use app_config::AppConfig;
use app_core::{
    App, Command, DISPLAY_SIZE, Event, NetworkStatus, PlaybackState, RECOMMENDED_SCRATCH_PIXELS,
    Screen, TouchPoint,
};
use app_runtime::{AppRuntime, host_tcp::HostTcpConnector, lpec::LpecHifi};
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

const REFRESH_INTERVAL_SECONDS: f64 = 0.1;
const TAP_FULL_MS: u64 = 1_000;
const TAP_FADE_MS: u64 = 700;
const WINDOW_MARGIN: f64 = 24.0;
const SIDE_GAP: f64 = 24.0;
const SIDE_WIDTH: f64 = 210.0;
const BUTTON_WIDTH: f64 = 178.0;
const BUTTON_HEIGHT: f64 = 32.0;
const BUTTON_SPACING: f64 = 44.0;
const HIFI_STATUS_POLL_MS: u64 = 2_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DisplayShape {
    Circle,
    Rectangle,
}

impl DisplayShape {
    fn contains(self, point: TouchPoint, size: Size) -> bool {
        if point.x < 0
            || point.y < 0
            || point.x >= size.width as i32
            || point.y >= size.height as i32
        {
            return false;
        }

        match self {
            Self::Circle => {
                let radius = size.width.min(size.height) as i64 / 2;
                let center_x = size.width as i64 / 2;
                let center_y = size.height as i64 / 2;
                let dx = point.x as i64 - center_x;
                let dy = point.y as i64 - center_y;
                dx * dx + dy * dy <= radius * radius
            }
            Self::Rectangle => true,
        }
    }

    fn alpha_at(self, x: u32, y: u32, size: Size) -> u8 {
        match self {
            Self::Circle => {
                let radius = size.width.min(size.height) as i64 / 2;
                let center_x = size.width as i64 / 2;
                let center_y = size.height as i64 / 2;
                let dx = x as i64 - center_x;
                let dy = y as i64 - center_y;

                if dx * dx + dy * dy <= radius * radius {
                    255
                } else {
                    0
                }
            }
            Self::Rectangle => 255,
        }
    }
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

    fn to_png(&self, display_shape: DisplayShape) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(self.pixels.len() * 4);

        for (index, color) in self.pixels.iter().enumerate() {
            let x = index as u32 % self.size.width;
            let y = index as u32 / self.size.width;
            rgba.push(scale_channel(color.r(), Rgb565::MAX_R));
            rgba.push(scale_channel(color.g(), Rgb565::MAX_G));
            rgba.push(scale_channel(color.b(), Rgb565::MAX_B));
            rgba.push(display_shape.alpha_at(x, y, self.size));
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
    display_shape: DisplayShape,
    runtime: AppRuntime<LpecHifi<HostTcpConnector>>,
    started_at: Instant,
    manual_time_offset_ms: u64,
    last_hifi_status_poll_ms: u64,
    last_hifi_artwork_uri: Option<String>,
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
            display_shape: DisplayShape::Circle,
            runtime: AppRuntime::new(LpecHifi::new(
                HostTcpConnector::new(),
                AppConfig::load_local_or_default().linn_lpec_endpoint,
            )),
            started_at: Instant::now(),
            manual_time_offset_ms: 0,
            last_hifi_status_poll_ms: 0,
            last_hifi_artwork_uri: None,
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
    }

    fn handle_command(&mut self, command: Command) {
        if let Err(error) = self.runtime.handle_command(command) {
            eprintln!("failed to handle app command {command:?}: {error:?}");
        }
    }

    fn tick(&mut self) {
        let uptime_ms = self.uptime_ms();
        self.update(Event::Tick { uptime_ms });
        self.poll_hifi_status_if_needed(uptime_ms);
        self.render_stats.update_sample();
    }

    fn poll_hifi_status_if_needed(&mut self, uptime_ms: u64) {
        if self.app.screen() != Screen::HifiControl
            || uptime_ms.saturating_sub(self.last_hifi_status_poll_ms) < HIFI_STATUS_POLL_MS
        {
            return;
        }

        self.last_hifi_status_poll_ms = uptime_ms;
        match self.runtime.hifi_status() {
            Ok(status) => {
                let artwork_uri = status.album_art_uri.as_str().to_owned();
                let should_load_artwork =
                    status.playback == PlaybackState::Playing && !artwork_uri.is_empty();
                self.update(Event::HifiStatus(status));
                if should_load_artwork {
                    self.load_hifi_artwork_if_needed(&artwork_uri);
                } else if artwork_uri.is_empty() {
                    self.last_hifi_artwork_uri = None;
                }
            }
            Err(error) => eprintln!("failed to read Linn hifi status: {error:?}"),
        }
    }

    fn load_hifi_artwork_if_needed(&mut self, uri: &str) {
        if self.last_hifi_artwork_uri.as_deref() == Some(uri) {
            return;
        }

        self.last_hifi_artwork_uri = Some(uri.to_owned());
        match self.runtime.hifi_artwork(uri) {
            Ok(artwork) => self.update(Event::HifiArtwork(artwork)),
            Err(error) => eprintln!("failed to load Linn artwork {uri:?}: {error:?}"),
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

    fn set_display_shape(&mut self, display_shape: DisplayShape) {
        if self.display_shape != display_shape {
            self.display_shape = display_shape;
            self.output_frame_dirty = true;
        }
    }

    fn contains_visible_point(&self, point: TouchPoint) -> bool {
        self.display_shape.contains(point, DISPLAY_SIZE)
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
        Some(self.output_framebuffer.to_png(self.display_shape))
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
            window.setTitle(ns_string!("ESP32-C6 Home Tools Simulator"));
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

            let zoom_button = add_button(
                &side_panel,
                "Zoom 2x",
                0.0,
                layout.side_inner_height - BUTTON_HEIGHT,
                sel!(toggleZoom:),
                self,
                mtm,
            );
            add_button(
                &side_panel,
                "Advance +1s",
                0.0,
                layout.side_inner_height - BUTTON_HEIGHT - BUTTON_SPACING,
                sel!(advanceSecond:),
                self,
                mtm,
            );
            add_button(
                &side_panel,
                "Boot button",
                0.0,
                layout.side_inner_height - BUTTON_HEIGHT - BUTTON_SPACING * 2.0,
                sel!(bootButton:),
                self,
                mtm,
            );
            add_button(
                &side_panel,
                "User button",
                0.0,
                layout.side_inner_height - BUTTON_HEIGHT - BUTTON_SPACING * 3.0,
                sel!(userButton:),
                self,
                mtm,
            );
            add_network_status_popup(
                &side_panel,
                0.0,
                layout.side_inner_height - BUTTON_HEIGHT - BUTTON_SPACING * 4.4,
                self,
                mtm,
            );
            add_display_shape_popup(
                &side_panel,
                0.0,
                layout.side_inner_height - BUTTON_HEIGHT - BUTTON_SPACING * 5.4,
                self,
                mtm,
            );

            let debug_label = NSTextField::wrappingLabelWithString(
                &NSString::from_str(&self.debug_text()),
                mtm,
            );
            debug_label.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(SIDE_WIDTH, 148.0),
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

        #[unsafe(method(bootButton:))]
        fn boot_button(&self, _sender: &AnyObject) {
            self.send_event(Event::ButtonPressed(app_core::Button::Boot));
        }

        #[unsafe(method(userButton:))]
        fn user_button(&self, _sender: &AnyObject) {
            self.send_event(Event::ButtonPressed(app_core::Button::User));
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

            if !self
                .ivars()
                .simulator
                .borrow()
                .contains_visible_point(TouchPoint { x, y })
            {
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

        #[unsafe(method(displayShapeChanged:))]
        fn display_shape_changed(&self, sender: &NSPopUpButton) {
            self.set_display_shape_from_index(sender.indexOfSelectedItem());
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

    fn set_display_shape_from_index(&self, index: isize) {
        let display_shape = match index {
            0 => DisplayShape::Circle,
            1 => DisplayShape::Rectangle,
            _ => return,
        };
        self.ivars()
            .simulator
            .borrow_mut()
            .set_display_shape(display_shape);
        self.refresh_image();
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

fn add_button(
    content_view: &NSView,
    title: &'static str,
    x: f64,
    y: f64,
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
        NSSize::new(BUTTON_WIDTH, BUTTON_HEIGHT),
    ));
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

fn add_display_shape_popup(
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
    popup.addItemWithTitle(&NSString::from_str("Circle"));
    popup.addItemWithTitle(&NSString::from_str("Rectangle"));
    popup.selectItemAtIndex(0);
    unsafe {
        popup.setTarget(Some(target.as_ref()));
        popup.setAction(Some(sel!(displayShapeChanged:)));
    }
    popup.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
    content_view.addSubview(&popup);
    popup
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
    let native_display_points = DISPLAY_SIZE.width as f64 / backing_scale;
    let zoomed_display_points = native_display_points * 2.0;
    let display_points = if zoomed {
        zoomed_display_points
    } else {
        native_display_points
    };
    let content_width =
        WINDOW_MARGIN + zoomed_display_points + SIDE_GAP + SIDE_WIDTH + WINDOW_MARGIN;
    let content_height = WINDOW_MARGIN * 2.0 + zoomed_display_points;
    let reserved_display_x = WINDOW_MARGIN;
    let reserved_display_y = WINDOW_MARGIN;
    let display_x = reserved_display_x + (zoomed_display_points - display_points) / 2.0;
    let display_y = reserved_display_y + (zoomed_display_points - display_points) / 2.0;
    let side_height = content_height - WINDOW_MARGIN * 2.0;
    let side_x = WINDOW_MARGIN + zoomed_display_points + SIDE_GAP;

    Layout {
        content_size: NSSize::new(content_width, content_height),
        display_frame: NSRect::new(
            NSPoint::new(display_x, display_y),
            NSSize::new(display_points, display_points),
        ),
        side_frame: NSRect::new(
            NSPoint::new(side_x, WINDOW_MARGIN),
            NSSize::new(SIDE_WIDTH, side_height),
        ),
        side_inner_height: side_height,
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

fn main() {
    let mtm = MainThreadMarker::new().expect("simulator must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    let delegate = Delegate::new(mtm);

    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
}
