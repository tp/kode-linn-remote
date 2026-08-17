use app_core::{Command, Event, HIFI_URI_LEN, HifiCommand, PlaybackState, Screen};
use heapless::String;

use crate::{AppRuntime, HifiController, RuntimeError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DriverError<E> {
    Command(RuntimeError<E>),
    Status(RuntimeError<E>),
    Artwork(RuntimeError<E>),
    Pins(RuntimeError<E>),
}

#[derive(Debug)]
pub struct HifiDriver<Hifi> {
    runtime: AppRuntime<Hifi>,
    poll_interval_ms: u64,
    active: bool,
    last_status_poll_ms: u64,
    status_poll_requested: bool,
    pins_fetched: bool,
    last_artwork_uri: String<HIFI_URI_LEN>,
    pending_artwork_uri: String<HIFI_URI_LEN>,
    /// Most recent uptime seen on a tick.
    ///
    /// A skip arrives on the command channel, which carries no clock, but the
    /// prediction it triggers needs one to know when to stop believing itself.
    /// Ticks are frequent (50 ms in the simulator) against a multi-second
    /// window, so the last one is close enough.
    last_uptime_ms: u64,
}

impl<Hifi> HifiDriver<Hifi> {
    pub fn new(hifi: Hifi, poll_interval_ms: u64) -> Self {
        Self {
            runtime: AppRuntime::new(hifi),
            poll_interval_ms,
            active: false,
            last_status_poll_ms: 0,
            status_poll_requested: false,
            pins_fetched: false,
            last_artwork_uri: String::new(),
            pending_artwork_uri: String::new(),
            last_uptime_ms: 0,
        }
    }

    pub fn set_screen(&mut self, screen: Screen, uptime_ms: u64) {
        self.set_active(screen == Screen::HifiControl, uptime_ms);
    }

    pub fn set_active(&mut self, active: bool, uptime_ms: u64) {
        if self.active == active {
            return;
        }

        self.active = active;
        if active {
            self.last_status_poll_ms = uptime_ms.saturating_sub(self.poll_interval_ms);
            self.status_poll_requested = true;
        } else {
            self.pending_artwork_uri.clear();
            self.status_poll_requested = false;
        }
    }

    pub const fn active(&self) -> bool {
        self.active
    }

    pub fn request_status_poll(&mut self) {
        self.status_poll_requested = true;
    }

    pub fn forget_current_track_artwork(&mut self) {
        self.last_artwork_uri.clear();
        self.pending_artwork_uri.clear();
    }

    pub fn into_runtime(self) -> AppRuntime<Hifi> {
        self.runtime
    }
}

impl<Hifi> HifiDriver<Hifi>
where
    Hifi: HifiController,
{
    /// Moves to the next or previous track, showing it immediately when we can
    /// say what it is.
    ///
    /// Returns the status to display now, or `None` when there is nothing to
    /// predict from — in which case this falls back to what it always did:
    /// clear the track and wait for the device.
    pub fn mark_track_changed(&mut self, forward: bool) -> Option<Event> {
        self.status_poll_requested = true;

        if let Some(status) = self
            .runtime
            .hifi_mut()
            .predict_skip(forward, self.last_uptime_ms)
        {
            // Queue the predicted artwork the way a real status would, so the
            // picture follows the words rather than trailing a round trip
            // behind them.
            let artwork_uri = status.album_art_uri.clone();
            if !artwork_uri.is_empty() && self.last_artwork_uri.as_str() != artwork_uri.as_str() {
                self.pending_artwork_uri.clear();
                let _ = self.pending_artwork_uri.push_str(artwork_uri.as_str());
            }
            return Some(Event::HifiStatus(status));
        }

        self.forget_current_track_artwork();
        self.runtime.hifi_mut().mark_track_changed();
        None
    }

    pub fn handle_command(
        &mut self,
        command: HifiCommand,
    ) -> Result<Option<Event>, DriverError<Hifi::Error>> {
        if !self.active {
            return Ok(None);
        }

        let defer_status = matches!(command, HifiCommand::PreviousTrack | HifiCommand::NextTrack);

        self.runtime
            .handle_command(Command::Hifi(command))
            .map_err(DriverError::Command)?;
        if defer_status {
            let forward = matches!(command, HifiCommand::NextTrack);
            return Ok(self.mark_track_changed(forward));
        }
        self.refresh_status()
    }

    pub fn poll_status_if_due(
        &mut self,
        uptime_ms: u64,
    ) -> Result<Option<Event>, DriverError<Hifi::Error>> {
        if !self.active {
            return Ok(None);
        }

        self.last_uptime_ms = uptime_ms;
        let due = self.status_poll_requested
            || uptime_ms.saturating_sub(self.last_status_poll_ms) >= self.poll_interval_ms;
        if !due {
            return Ok(None);
        }

        self.status_poll_requested = false;
        self.last_status_poll_ms = uptime_ms;
        self.last_uptime_ms = uptime_ms;
        self.runtime.hifi_mut().set_clock(uptime_ms);
        let result = self.refresh_status();
        if result.is_err() {
            self.status_poll_requested = true;
        }
        result
    }

    pub fn fetch_pins_if_needed(&mut self) -> Result<Option<Event>, DriverError<Hifi::Error>> {
        if !self.active || self.pins_fetched {
            return Ok(None);
        }

        self.pins_fetched = true;
        let pins = self.runtime.hifi_pins().map_err(DriverError::Pins)?;
        Ok(Some(Event::HifiPins(pins)))
    }

    pub fn load_pending_artwork(&mut self) -> Result<Option<Event>, DriverError<Hifi::Error>> {
        if !self.active || self.pending_artwork_uri.is_empty() {
            return Ok(None);
        }

        let mut uri = String::<HIFI_URI_LEN>::new();
        let _ = uri.push_str(self.pending_artwork_uri.as_str());
        self.pending_artwork_uri.clear();

        self.last_artwork_uri.clear();
        let _ = self.last_artwork_uri.push_str(uri.as_str());

        let artwork = self
            .runtime
            .hifi_artwork(uri.as_str())
            .map_err(DriverError::Artwork)?;
        Ok(Some(Event::HifiArtwork(artwork)))
    }

    fn refresh_status(&mut self) -> Result<Option<Event>, DriverError<Hifi::Error>> {
        let status = self.runtime.hifi_status().map_err(DriverError::Status)?;
        let artwork_uri = status.album_art_uri.clone();
        let should_load_artwork =
            status.playback == PlaybackState::Playing && !artwork_uri.as_str().is_empty();

        if should_load_artwork
            && self.last_artwork_uri.as_str() != artwork_uri.as_str()
            && self.pending_artwork_uri.as_str() != artwork_uri.as_str()
        {
            self.pending_artwork_uri.clear();
            let _ = self.pending_artwork_uri.push_str(artwork_uri.as_str());
        } else if artwork_uri.as_str().is_empty() {
            self.last_artwork_uri.clear();
            self.pending_artwork_uri.clear();
        }

        if status_needs_followup(&status) {
            self.status_poll_requested = true;
        }

        Ok(Some(Event::HifiStatus(status)))
    }
}

fn status_needs_followup(status: &app_core::HifiStatus) -> bool {
    matches!(
        status.playback,
        PlaybackState::Unknown | PlaybackState::Buffering
    )
}

#[cfg(feature = "std")]
pub mod worker {
    use std::{
        sync::mpsc::{self, Receiver, Sender, TryRecvError},
        thread,
    };

    use app_core::{Command, Event, HifiCommand, Screen};

    use super::{DriverError, HifiDriver};
    use crate::{AppRuntime, HifiController};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum Request {
        Command(HifiCommand),
        SyncScreen { screen: Screen, uptime_ms: u64 },
        Tick { uptime_ms: u64 },
        RequestStatusPoll,
        TrackChanged { forward: bool },
    }

    // `Event` carries a whole `HifiStatus`, so it dwarfs the other variants.
    // Boxing it would move the allocation onto the channel's hot path to save
    // stack this worker has plenty of, and `Event` has to stay usable from the
    // `no_std` side of the crate.
    #[allow(clippy::large_enum_variant)]
    #[derive(Debug)]
    pub enum Response<E> {
        Event(Event),
        Error(DriverError<E>),
        Disconnected,
    }

    #[derive(Debug)]
    pub struct Worker<E> {
        background_requests: Sender<Request>,
        command_requests: Sender<HifiCommand>,
        responses: Receiver<Response<E>>,
    }

    impl<E> Worker<E> {
        pub fn send(&self, request: Request) -> Result<(), mpsc::SendError<Request>> {
            match request {
                Request::Command(command) => self
                    .command_requests
                    .send(command)
                    .map_err(|error| mpsc::SendError(Request::Command(error.0))),
                request => self.background_requests.send(request),
            }
        }

        pub fn try_recv(&self) -> Result<Response<E>, TryRecvError> {
            self.responses.try_recv()
        }
    }

    pub fn start<Hifi>(driver: HifiDriver<Hifi>, command_hifi: Hifi) -> Worker<Hifi::Error>
    where
        Hifi: HifiController + Send + 'static,
        Hifi::Error: Send + 'static,
    {
        let (background_tx, background_rx) = mpsc::channel();
        let (command_tx, command_rx) = mpsc::channel();
        let (responses_tx, responses_rx) = mpsc::channel();

        let command_background_tx = background_tx.clone();
        let command_responses_tx = responses_tx.clone();
        thread::spawn(move || {
            run_commands(
                AppRuntime::new(command_hifi),
                command_rx,
                command_background_tx,
                command_responses_tx,
            );
        });

        thread::spawn(move || {
            run_background(driver, background_rx, responses_tx);
        });

        Worker {
            background_requests: background_tx,
            command_requests: command_tx,
            responses: responses_rx,
        }
    }

    fn run_commands<Hifi>(
        mut runtime: AppRuntime<Hifi>,
        commands: Receiver<HifiCommand>,
        background_requests: Sender<Request>,
        responses: Sender<Response<Hifi::Error>>,
    ) where
        Hifi: HifiController,
    {
        while let Ok(command) = commands.recv() {
            match runtime.handle_command(Command::Hifi(command)) {
                Ok(()) => {
                    if matches!(command, HifiCommand::PreviousTrack | HifiCommand::NextTrack) {
                        let forward = matches!(command, HifiCommand::NextTrack);
                        let _ = background_requests.send(Request::TrackChanged { forward });
                    }
                    let _ = background_requests.send(Request::RequestStatusPoll);
                }
                Err(error) => {
                    let _ = responses.send(Response::Error(DriverError::Command(error)));
                }
            }
        }
    }

    fn run_background<Hifi>(
        mut driver: HifiDriver<Hifi>,
        requests: Receiver<Request>,
        responses: Sender<Response<Hifi::Error>>,
    ) where
        Hifi: HifiController,
    {
        while let Ok(request) = requests.recv() {
            match request {
                Request::Command(_) => {}
                Request::SyncScreen { screen, uptime_ms } => {
                    driver.set_screen(screen, uptime_ms);
                }
                Request::Tick { uptime_ms } => {
                    send_result(&responses, driver.poll_status_if_due(uptime_ms));
                    send_result(&responses, driver.load_pending_artwork());
                    send_result(&responses, driver.fetch_pins_if_needed());
                }
                Request::RequestStatusPoll => {
                    driver.request_status_poll();
                }
                Request::TrackChanged { forward } => {
                    // A prediction is an event in its own right: it is what
                    // makes the screen change on the press rather than on the
                    // reply.
                    if let Some(event) = driver.mark_track_changed(forward) {
                        let _ = responses.send(Response::Event(event));
                    }
                }
            }
        }

        let _ = responses.send(Response::Disconnected);
    }

    fn send_result<E>(
        responses: &Sender<Response<E>>,
        result: Result<Option<Event>, DriverError<E>>,
    ) {
        match result {
            Ok(Some(event)) => {
                let _ = responses.send(Response::Event(event));
            }
            Ok(None) => {}
            Err(error) => {
                let _ = responses.send(Response::Error(error));
            }
        }
    }
}
