use app_core::{Event, TouchPoint};
use board_waveshare_c6::DISPLAY_SIZE;
use esp_hal::{Blocking, i2c::master::I2c};

use crate::wait_millis;

pub const FT6146_ADDRESS: u8 = 0x38;

const REG_TOUCH_DATA: u8 = 0x02;
const TOUCH_DATA_LEN: usize = 5;
const TOUCH_COUNT_MASK: u8 = 0x0f;
const COORD_HIGH_MASK: u8 = 0x0f;

#[derive(Debug)]
pub struct TouchController {
    pressed: bool,
}

impl TouchController {
    pub const fn new() -> Self {
        Self { pressed: false }
    }

    pub fn poll(
        &mut self,
        i2c: &mut I2c<'_, Blocking>,
    ) -> Result<Option<Event>, esp_hal::i2c::master::Error> {
        let sample = read_sample(i2c)?;

        match (self.pressed, sample) {
            (false, Some(point)) => {
                self.pressed = true;
                Ok(Some(Event::TouchDown(point)))
            }
            (true, None) => {
                self.pressed = false;
                Ok(Some(Event::TouchUp))
            }
            _ => Ok(None),
        }
    }
}

fn read_sample(
    i2c: &mut I2c<'_, Blocking>,
) -> Result<Option<TouchPoint>, esp_hal::i2c::master::Error> {
    let mut data = [0_u8; TOUCH_DATA_LEN];
    i2c.write(FT6146_ADDRESS, &[REG_TOUCH_DATA])?;
    wait_millis(1);
    i2c.read(FT6146_ADDRESS, &mut data)?;

    let touch_count = data[0] & TOUCH_COUNT_MASK;
    if touch_count == 0 {
        return Ok(None);
    }

    let x = (((data[1] & COORD_HIGH_MASK) as u16) << 8) | data[2] as u16;
    let y = (((data[3] & COORD_HIGH_MASK) as u16) << 8) | data[4] as u16;

    Ok(Some(TouchPoint {
        x: clamp_axis(x, DISPLAY_SIZE.width),
        y: clamp_axis(y, DISPLAY_SIZE.height),
    }))
}

fn clamp_axis(value: u16, limit: u32) -> i32 {
    value.min(limit.saturating_sub(1) as u16) as i32
}
