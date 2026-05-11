#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use app_core::{Command, HifiArtwork, HifiCommand, HifiPins, HifiStatus};

#[cfg(feature = "std")]
pub mod host_tcp;
pub mod lpec;
pub mod net;

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
