#![deny(unsafe_op_in_unsafe_fn)]

use std::{
    cell::{OnceCell, RefCell},
    convert::Infallible,
};

use app_core::{App, DISPLAY_SIZE, Event, NetworkStatus, TouchPoint};
use embedded_graphics::{
    Pixel,
    draw_target::DrawTarget,
    geometry::OriginDimensions,
    pixelcolor::{Rgb565, RgbColor},
    prelude::*,
};
use image::{ColorType, ImageEncoder, codecs::png::PngEncoder};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSAutoresizingMaskOptions,
    NSBackingStoreType, NSButton, NSImage, NSImageScaling, NSImageView, NSView, NSWindow,
    NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSData, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString, ns_string,
};

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
    uptime_ms: u64,
}

impl NativeSimulator {
    fn new() -> Self {
        Self {
            app: App::new(),
            framebuffer: Framebuffer::new(DISPLAY_SIZE),
            uptime_ms: 0,
        }
    }

    fn update(&mut self, event: Event) {
        self.app.update(event);
    }

    fn tick(&mut self) {
        self.uptime_ms = self.uptime_ms.saturating_add(1_000);
        self.update(Event::Tick {
            uptime_ms: self.uptime_ms,
        });
    }

    fn redraw_png(&mut self) -> Vec<u8> {
        self.app
            .render(&mut self.framebuffer)
            .expect("app rendering should succeed");
        self.framebuffer.to_png()
    }
}

#[derive(Debug)]
struct AppDelegateIvars {
    window: OnceCell<Retained<NSWindow>>,
    image_view: OnceCell<Retained<NSImageView>>,
    simulator: RefCell<NativeSimulator>,
}

impl Default for AppDelegateIvars {
    fn default() -> Self {
        Self {
            window: OnceCell::new(),
            image_view: OnceCell::new(),
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
            content_view.addSubview(&image_view);

            add_button(&content_view, "Tick +1s", 524.0, 496.0, sel!(tick:), self, mtm);
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
                "Touch center",
                524.0,
                344.0,
                sel!(touchCenter:),
                self,
                mtm,
            );
            add_button(
                &content_view,
                "Touch top-left",
                524.0,
                300.0,
                sel!(touchTopLeft:),
                self,
                mtm,
            );
            add_button(
                &content_view,
                "Touch bottom-right",
                524.0,
                256.0,
                sel!(touchBottomRight:),
                self,
                mtm,
            );
            add_button(
                &content_view,
                "Release touch",
                524.0,
                212.0,
                sel!(releaseTouch:),
                self,
                mtm,
            );
            add_button(
                &content_view,
                "Network offline",
                524.0,
                148.0,
                sel!(networkOffline:),
                self,
                mtm,
            );
            add_button(
                &content_view,
                "Network connecting",
                524.0,
                104.0,
                sel!(networkConnecting:),
                self,
                mtm,
            );
            add_button(
                &content_view,
                "Network online",
                524.0,
                60.0,
                sel!(networkOnline:),
                self,
                mtm,
            );

            window.makeKeyAndOrderFront(None);

            self.ivars()
                .image_view
                .set(image_view)
                .expect("image view should be stored once");
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
            NSApplication::sharedApplication(self.mtm()).terminate(None);
        }
    }

    impl Delegate {
        #[unsafe(method(tick:))]
        fn tick(&self, _sender: &AnyObject) {
            self.ivars().simulator.borrow_mut().tick();
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

        #[unsafe(method(touchCenter:))]
        fn touch_center(&self, _sender: &AnyObject) {
            self.send_event(Event::TouchDown(TouchPoint { x: 233, y: 233 }));
        }

        #[unsafe(method(touchTopLeft:))]
        fn touch_top_left(&self, _sender: &AnyObject) {
            self.send_event(Event::TouchDown(TouchPoint { x: 72, y: 72 }));
        }

        #[unsafe(method(touchBottomRight:))]
        fn touch_bottom_right(&self, _sender: &AnyObject) {
            self.send_event(Event::TouchDown(TouchPoint { x: 394, y: 394 }));
        }

        #[unsafe(method(releaseTouch:))]
        fn release_touch(&self, _sender: &AnyObject) {
            self.send_event(Event::TouchUp);
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

fn main() {
    let mtm = MainThreadMarker::new().expect("simulator must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    let delegate = Delegate::new(mtm);

    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
}
