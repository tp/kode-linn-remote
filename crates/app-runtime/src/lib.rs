#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use app_core::{Command, HifiArtwork, HifiCommand, HifiPins, HifiStatus};

pub mod hifi;
#[cfg(feature = "std")]
pub mod host_tcp;
pub mod lpec;
pub mod net;
pub mod playlist;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeError<E> {
    Hifi(E),
}

pub trait HifiController {
    type Error;

    fn handle_command(&mut self, command: HifiCommand) -> Result<(), Self::Error>;
    fn status(&mut self) -> Result<HifiStatus, Self::Error>;
    fn artwork(&mut self, uri: &str) -> Result<HifiArtwork, Self::Error>;
    fn pins(&mut self) -> Result<HifiPins, Self::Error>;
    fn mark_track_changed(&mut self) {}

    /// Hands the controller the current uptime.
    ///
    /// Controllers that reason about time — expiring an optimistic skip, say —
    /// have no clock of their own, and `app-runtime` is `no_std`, so the
    /// platform's notion of now has to arrive from outside.
    fn set_clock(&mut self, _now_ms: u64) {}

    /// Moves to the neighbouring track without waiting for the device.
    ///
    /// Returns the status to show immediately, or `None` when the controller
    /// cannot honestly say what comes next — which is the default, and leaves
    /// callers with the behaviour they had before.
    fn predict_skip(&mut self, _forward: bool, _now_ms: u64) -> Option<HifiStatus> {
        None
    }
}

#[derive(Debug)]
pub struct AppRuntime<Hifi> {
    hifi: Hifi,
}

impl<Hifi> AppRuntime<Hifi> {
    pub const fn new(hifi: Hifi) -> Self {
        Self { hifi }
    }

    pub fn into_hifi(self) -> Hifi {
        self.hifi
    }

    pub fn hifi_mut(&mut self) -> &mut Hifi {
        &mut self.hifi
    }
}

impl<Hifi> AppRuntime<Hifi>
where
    Hifi: HifiController,
{
    pub fn handle_command(&mut self, command: Command) -> Result<(), RuntimeError<Hifi::Error>> {
        match command {
            Command::Hifi(command) => self
                .hifi
                .handle_command(command)
                .map_err(RuntimeError::Hifi),
        }
    }

    pub fn hifi_status(&mut self) -> Result<HifiStatus, RuntimeError<Hifi::Error>> {
        self.hifi.status().map_err(RuntimeError::Hifi)
    }

    pub fn hifi_artwork(&mut self, uri: &str) -> Result<HifiArtwork, RuntimeError<Hifi::Error>> {
        self.hifi.artwork(uri).map_err(RuntimeError::Hifi)
    }

    pub fn hifi_pins(&mut self) -> Result<HifiPins, RuntimeError<Hifi::Error>> {
        self.hifi.pins().map_err(RuntimeError::Hifi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatches_hifi_pin_command_to_controller() {
        let mut runtime = AppRuntime::new(FakeHifi::default());

        runtime
            .handle_command(Command::Hifi(HifiCommand::InvokePinId { id: 4711 }))
            .unwrap();

        assert_eq!(
            runtime.into_hifi().commands.as_slice(),
            [HifiCommand::InvokePinId { id: 4711 }]
        );
    }

    #[test]
    fn dispatches_hifi_set_volume_command_to_controller() {
        let mut runtime = AppRuntime::new(FakeHifi::default());

        runtime
            .handle_command(Command::Hifi(HifiCommand::SetVolume { volume: 42 }))
            .unwrap();

        assert_eq!(
            runtime.into_hifi().commands.as_slice(),
            [HifiCommand::SetVolume { volume: 42 }]
        );
    }

    #[test]
    fn dispatches_hifi_playback_command_to_controller() {
        let mut runtime = AppRuntime::new(FakeHifi::default());

        runtime
            .handle_command(Command::Hifi(HifiCommand::TogglePlayback))
            .unwrap();

        assert_eq!(
            runtime.into_hifi().commands.as_slice(),
            [HifiCommand::TogglePlayback]
        );
    }

    #[test]
    fn dispatches_hifi_track_commands_to_controller() {
        let mut runtime = AppRuntime::new(FakeHifi::default());

        runtime
            .handle_command(Command::Hifi(HifiCommand::PreviousTrack))
            .unwrap();
        runtime
            .handle_command(Command::Hifi(HifiCommand::NextTrack))
            .unwrap();

        assert_eq!(
            runtime.into_hifi().commands.as_slice(),
            [HifiCommand::PreviousTrack, HifiCommand::NextTrack]
        );
    }

    #[derive(Default)]
    struct FakeHifi {
        commands: heapless::Vec<HifiCommand, 4>,
    }

    impl HifiController for FakeHifi {
        type Error = core::convert::Infallible;

        fn handle_command(&mut self, command: HifiCommand) -> Result<(), Self::Error> {
            self.commands.push(command).unwrap();
            Ok(())
        }

        fn status(&mut self) -> Result<HifiStatus, Self::Error> {
            Ok(HifiStatus::waiting())
        }

        fn artwork(&mut self, uri: &str) -> Result<HifiArtwork, Self::Error> {
            Ok(HifiArtwork::new(uri).unwrap())
        }

        fn pins(&mut self) -> Result<HifiPins, Self::Error> {
            Ok(HifiPins::new())
        }
    }
}
