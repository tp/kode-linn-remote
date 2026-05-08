#![deny(unsafe_op_in_unsafe_fn)]

use std::{
    cell::{OnceCell, RefCell},
    convert::Infallible,
    time::Instant,
};

use app_core::{App, DISPLAY_SIZE, Event, NetworkStatus, TouchPoint};
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
    NSView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSData, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString, NSTimer, ns_string,
};

const REFRESH_INTERVAL_SECONDS: f64 = 0.1;
const TAP_FULL_MS: u64 = 1_000;
const TAP_FADE_MS: u64 = 700;

#[derive(Debug)]
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
    framebuffer: Framebuffer,
    started_at: Instant,
    manual_time_offset_ms: u64,
    tap_highlight: Option<TapHighlight>,
}

impl NativeSimulator {
    fn new() -> Self {
        Self {
            app: App::new(),
            framebuffer: Framebuffer::new(DISPLAY_SIZE),
            started_at: Instant::now(),
            manual_time_offset_ms: 0,
            tap_highlight: None,
        }
    }

    fn update(&mut self, event: Event) {
        self.app.update(event);
    }

    fn tick(&mut self) {
        self.update(Event::Tick {
            uptime_ms: self.uptime_ms(),
        });
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
        self.update(Event::Tick { uptime_ms });
        self.update(Event::TouchDown(point));
    }

    fn uptime_ms(&self) -> u64 {
        let real_elapsed_ms = self.started_at.elapsed().as_millis() as u64;
        real_elapsed_ms.saturating_add(self.manual_time_offset_ms)
    }

    fn redraw_png(&mut self) -> Vec<u8> {
        self.app
            .render(&mut self.framebuffer)
            .expect("app rendering should succeed");
        self.render_tap_highlight();
        self.framebuffer.to_png()
    }

    fn render_tap_highlight(&mut self) {
        let Some(tap) = self.tap_highlight else {
            return;
        };

        let Some(alpha) = tap_alpha(self.uptime_ms().saturating_sub(tap.started_at_ms)) else {
            self.tap_highlight = None;
            return;
        };

        let r = 8 + ((23_u16 * alpha as u16) / 255) as u8;
        let g = 14 + ((28_u16 * alpha as u16) / 255) as u8;
        let b = 4 + ((10_u16 * alpha as u16) / 255) as u8;
        Circle::with_center(Point::new(tap.point.x, tap.point.y), 18)
            .into_styled(PrimitiveStyle::with_fill(Rgb565::new(r, g, b)))
            .draw(&mut self.framebuffer)
            .expect("simulator overlay rendering should succeed");
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TapHighlight {
    point: TouchPoint,
    started_at_ms: u64,
}

#[derive(Debug)]
struct AppDelegateIvars {
    window: OnceCell<Retained<NSWindow>>,
    image_view: OnceCell<Retained<NSImageView>>,
    tap_recognizer: OnceCell<Retained<NSClickGestureRecognizer>>,
    timer: OnceCell<Retained<NSTimer>>,
    simulator: RefCell<NativeSimulator>,
}

impl Default for AppDelegateIvars {
    fn default() -> Self {
        Self {
            window: OnceCell::new(),
            image_view: OnceCell::new(),
            tap_recognizer: OnceCell::new(),
            timer: OnceCell::new(),
            simulator: RefCell::new(NativeSimulator::new()),
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
        fn did_finish_launching(&self, notification: &NSNotification) {
            let mtm = self.mtm();
            let app = notification
                .object()
                .expect("launch notification should have an app object")
                .downcast::<NSApplication>()
                .expect("launch notification object should be NSApplication");

            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(760.0, 580.0)),
                    NSWindowStyleMask::Titled
                        | NSWindowStyleMask::Closable
                        | NSWindowStyleMask::Miniaturizable
                        | NSWindowStyleMask::Resizable,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            unsafe { window.setReleasedWhenClosed(false) };
            window.setTitle(ns_string!("ESP32-C6 Home Tools Simulator"));
            window.center();
            window.setContentMinSize(NSSize::new(720.0, 560.0));
            window.setDelegate(Some(ProtocolObject::from_ref(self)));

            let content_view = window.contentView().expect("window should have a content view");
            let image = self.render_image();
            let image_view = NSImageView::imageViewWithImage(&image, mtm);
            image_view.setFrame(NSRect::new(
                NSPoint::new(24.0, 74.0),
                NSSize::new(466.0, 466.0),
            ));
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

            add_button(
                &content_view,
                "Advance +1s",
                524.0,
                496.0,
                sel!(advanceSecond:),
                self,
                mtm,
            );
            add_button(
                &content_view,
                "Boot button",
                524.0,
                452.0,
                sel!(bootButton:),
                self,
                mtm,
            );
            add_button(
                &content_view,
                "User button",
                524.0,
                408.0,
                sel!(userButton:),
                self,
                mtm,
            );
            add_button(
                &content_view,
                "Network offline",
                524.0,
                344.0,
                sel!(networkOffline:),
                self,
                mtm,
            );
            add_button(
                &content_view,
                "Network connecting",
                524.0,
                300.0,
                sel!(networkConnecting:),
                self,
                mtm,
            );
            add_button(
                &content_view,
                "Network online",
                524.0,
                256.0,
                sel!(networkOnline:),
                self,
                mtm,
            );

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

            self.ivars()
                .image_view
                .set(image_view)
                .expect("image view should be stored once");
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

            app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
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
            let x = point.x.round() as i32;
            let y = (DISPLAY_SIZE.height as f64 - point.y).round() as i32;

            if x < 0 || y < 0 || x >= DISPLAY_SIZE.width as i32 || y >= DISPLAY_SIZE.height as i32 {
                return;
            }

            self.ivars()
                .simulator
                .borrow_mut()
                .tap(TouchPoint { x, y });
            self.refresh_image();
        }

        #[unsafe(method(networkOffline:))]
        fn network_offline(&self, _sender: &AnyObject) {
            self.send_event(Event::NetworkStatus(NetworkStatus::Offline));
        }

        #[unsafe(method(networkConnecting:))]
        fn network_connecting(&self, _sender: &AnyObject) {
            self.send_event(Event::NetworkStatus(NetworkStatus::Connecting));
        }

        #[unsafe(method(networkOnline:))]
        fn network_online(&self, _sender: &AnyObject) {
            self.send_event(Event::NetworkStatus(NetworkStatus::Online));
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
        let image = self.render_image();
        let image_view = self
            .ivars()
            .image_view
            .get()
            .expect("image view should exist after launch");
        image_view.setImage(Some(&image));
    }

    fn render_image(&self) -> Retained<NSImage> {
        let png = self.ivars().simulator.borrow_mut().redraw_png();
        let data = NSData::with_bytes(&png);
        NSImage::initWithData(NSImage::alloc(), &data).expect("AppKit should decode simulator PNG")
    }
}

fn add_button(
    content_view: &NSView,
    title: &'static str,
    x: f64,
    y: f64,
    action: objc2::runtime::Sel,
    target: &Delegate,
    mtm: MainThreadMarker,
) {
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target.as_ref()),
            Some(action),
            mtm,
        )
    };
    button.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(178.0, 32.0)));
    button.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
    content_view.addSubview(&button);
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
