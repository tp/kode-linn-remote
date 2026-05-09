use core::fmt;

use board_waveshare_c6::DISPLAY_SIZE;
use embedded_graphics::{
    Pixel, draw_target::DrawTarget, geometry::OriginDimensions, pixelcolor::Rgb565, prelude::*,
    primitives::Rectangle,
};
use esp_hal::{
    Blocking,
    gpio::{
        Level, Output, OutputConfig, OutputPin,
        interconnect::{PeripheralInput, PeripheralOutput},
    },
    spi::{
        Error as SpiError, Mode,
        master::{Address, Command, Config, ConfigError, DataMode, Instance, Spi},
    },
    time::Rate,
};

use crate::wait_millis;

const WIDTH: usize = DISPLAY_SIZE.width as usize;
const HEIGHT: usize = DISPLAY_SIZE.height as usize;

const LCD_SPI_MHZ: u32 = 20;
const LCD_X_GAP: u16 = 6;
const LCD_Y_GAP: u16 = 0;
const MAX_PIXELS_PER_WRITE: usize = 16;
const MIN_WRITE_ROWS: usize = 2;
const WRITE_BUFFER_BYTES: usize = MAX_PIXELS_PER_WRITE * MIN_WRITE_ROWS * 2;

const OPCODE_WRITE_COMMAND: u16 = 0x02;
const OPCODE_WRITE_COLOR: u16 = 0x32;

const CMD_SLEEP_OUT: u8 = 0x11;
const CMD_DISPLAY_ON: u8 = 0x29;
const CMD_COLUMN_ADDRESS: u8 = 0x2a;
const CMD_PAGE_ADDRESS: u8 = 0x2b;
const CMD_MEMORY_WRITE: u8 = 0x2c;
const CMD_MEMORY_ACCESS_CONTROL: u8 = 0x36;
const CMD_PIXEL_FORMAT: u8 = 0x3a;
const CMD_WRITE_DISPLAY_BRIGHTNESS: u8 = 0x51;
const CMD_WRITE_CONTROL_DISPLAY: u8 = 0x53;

#[derive(Debug)]
pub enum DisplayError {
    SpiConfig(ConfigError),
    Spi(SpiError),
}

impl From<ConfigError> for DisplayError {
    fn from(error: ConfigError) -> Self {
        Self::SpiConfig(error)
    }
}

impl From<SpiError> for DisplayError {
    fn from(error: SpiError) -> Self {
        Self::Spi(error)
    }
}

impl fmt::Display for DisplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SpiConfig(error) => write!(formatter, "SPI config error: {error:?}"),
            Self::Spi(error) => write!(formatter, "SPI transfer error: {error:?}"),
        }
    }
}

pub struct AmoledDisplay<'d> {
    spi: Spi<'d, Blocking>,
    cs: Output<'d>,
    reset: Output<'d>,
    pending: PendingRun,
}

struct PendingRun {
    x: u16,
    y: u16,
    len: usize,
    bytes: [u8; WRITE_BUFFER_BYTES],
}

impl PendingRun {
    const fn new() -> Self {
        Self {
            x: 0,
            y: 0,
            len: 0,
            bytes: [0; WRITE_BUFFER_BYTES],
        }
    }
}

impl<'d> AmoledDisplay<'d> {
    pub fn new(
        spi: impl Instance + 'd,
        sclk: impl PeripheralOutput<'d>,
        data0: impl PeripheralInput<'d> + PeripheralOutput<'d>,
        data1: impl PeripheralInput<'d> + PeripheralOutput<'d>,
        data2: impl PeripheralInput<'d> + PeripheralOutput<'d>,
        data3: impl PeripheralInput<'d> + PeripheralOutput<'d>,
        cs: impl OutputPin + 'd,
        reset: impl OutputPin + 'd,
    ) -> Result<Self, DisplayError> {
        let spi = Spi::new(
            spi,
            Config::default()
                .with_frequency(Rate::from_mhz(LCD_SPI_MHZ))
                .with_mode(Mode::_0),
        )?
        .with_sck(sclk)
        .with_sio0(data0)
        .with_sio1(data1)
        .with_sio2(data2)
        .with_sio3(data3);

        let cs = Output::new(cs, Level::High, OutputConfig::default());
        let reset = Output::new(reset, Level::High, OutputConfig::default());
        let mut display = Self {
            spi,
            cs,
            reset,
            pending: PendingRun::new(),
        };
        display.reset_panel();
        display.init_panel()?;
        Ok(display)
    }

    fn draw_pixel(&mut self, point: Point, color: Rgb565) -> Result<(), DisplayError> {
        if point.x < 0 || point.y < 0 || point.x >= WIDTH as i32 || point.y >= HEIGHT as i32 {
            return Ok(());
        }

        self.flush_pending()?;

        let x = ((point.x as u16) & !1).min((WIDTH - 2) as u16);
        let y = ((point.y as u16) & !1).min((HEIGHT - 2) as u16);
        let mut bytes = [0_u8; 8];
        let [high, low] = color.into_storage().to_be_bytes();

        for pixel in bytes.chunks_exact_mut(2) {
            pixel[0] = high;
            pixel[1] = low;
        }

        self.set_window(x, y, x + 1, y + 1)?;
        self.write_color(CMD_MEMORY_WRITE, &bytes)
    }

    #[allow(dead_code)]
    fn push_pending_color(&mut self, color: Rgb565) {
        let offset = self.pending.len * 2;
        let [high, low] = color.into_storage().to_be_bytes();
        self.pending.bytes[offset] = high;
        self.pending.bytes[offset + 1] = low;
        self.pending.len += 1;
    }

    fn flush_pending(&mut self) -> Result<(), DisplayError> {
        if self.pending.len == 0 {
            return Ok(());
        }

        let x_start = self.pending.x;
        let y = self.pending.y;
        let x_end = x_start + self.pending.len as u16 - 1;
        let byte_count = self.pending.len * 2;
        let mut bytes = [0_u8; WRITE_BUFFER_BYTES];
        bytes[..byte_count].copy_from_slice(&self.pending.bytes[..byte_count]);
        self.pending.len = 0;

        self.set_window(x_start, y, x_end, y)?;
        self.write_color(CMD_MEMORY_WRITE, &bytes[..byte_count])?;
        Ok(())
    }

    fn fill_rect(&mut self, area: &Rectangle, color: Rgb565) -> Result<(), DisplayError> {
        self.flush_pending()?;

        let x_start = (area.top_left.x.max(0) as usize) & !1;
        let y_start = (area.top_left.y.max(0) as usize) & !1;
        let x_end = align_up_2(
            (area.top_left.x as i64 + area.size.width as i64).clamp(0, WIDTH as i64) as usize,
        );
        let y_end = align_up_2(
            (area.top_left.y as i64 + area.size.height as i64).clamp(0, HEIGHT as i64) as usize,
        );

        if x_start >= x_end || y_start >= y_end {
            return Ok(());
        }

        let mut bytes = [0_u8; WRITE_BUFFER_BYTES];
        let [high, low] = color.into_storage().to_be_bytes();
        for pixel in bytes.chunks_exact_mut(2) {
            pixel[0] = high;
            pixel[1] = low;
        }

        let mut y = y_start;
        while y < y_end {
            let mut x = x_start;
            while x < x_end {
                let run_pixels = (x_end - x).min(MAX_PIXELS_PER_WRITE);
                let byte_count = run_pixels * MIN_WRITE_ROWS * 2;
                self.set_window(
                    x as u16,
                    y as u16,
                    (x + run_pixels - 1) as u16,
                    (y + MIN_WRITE_ROWS - 1) as u16,
                )?;
                self.write_color(CMD_MEMORY_WRITE, &bytes[..byte_count])?;
                x += run_pixels;
            }
            y += MIN_WRITE_ROWS;
        }

        Ok(())
    }

    fn reset_panel(&mut self) {
        self.reset.set_low();
        wait_millis(10);
        self.reset.set_high();
        wait_millis(150);
    }

    fn init_panel(&mut self) -> Result<(), DisplayError> {
        self.write_command(CMD_MEMORY_ACCESS_CONTROL, &[0x00])?;
        self.write_command(CMD_PIXEL_FORMAT, &[0x55])?;

        self.write_command(CMD_SLEEP_OUT, &[])?;
        wait_millis(80);
        self.write_command(0xc4, &[0x80])?;
        self.write_command(CMD_WRITE_CONTROL_DISPLAY, &[0x20])?;
        wait_millis(1);
        self.write_command(0x63, &[0xff])?;
        wait_millis(1);
        self.write_command(CMD_WRITE_DISPLAY_BRIGHTNESS, &[0x00])?;
        wait_millis(1);
        self.write_command(CMD_DISPLAY_ON, &[])?;
        wait_millis(10);
        self.write_command(CMD_WRITE_DISPLAY_BRIGHTNESS, &[0xff])?;
        self.write_command(CMD_DISPLAY_ON, &[])?;

        Ok(())
    }

    fn set_window(
        &mut self,
        x_start: u16,
        y_start: u16,
        x_end: u16,
        y_end: u16,
    ) -> Result<(), DisplayError> {
        let x_start = x_start + LCD_X_GAP;
        let x_end = x_end + LCD_X_GAP;
        let y_start = y_start + LCD_Y_GAP;
        let y_end = y_end + LCD_Y_GAP;

        self.write_command(
            CMD_COLUMN_ADDRESS,
            &[
                (x_start >> 8) as u8,
                x_start as u8,
                (x_end >> 8) as u8,
                x_end as u8,
            ],
        )?;
        self.write_command(
            CMD_PAGE_ADDRESS,
            &[
                (y_start >> 8) as u8,
                y_start as u8,
                (y_end >> 8) as u8,
                y_end as u8,
            ],
        )?;

        Ok(())
    }

    fn write_command(&mut self, lcd_cmd: u8, params: &[u8]) -> Result<(), DisplayError> {
        self.write_lcd(
            OPCODE_WRITE_COMMAND,
            lcd_cmd,
            params,
            DataMode::SingleTwoDataLines,
        )
    }

    fn write_color(&mut self, lcd_cmd: u8, pixels: &[u8]) -> Result<(), DisplayError> {
        self.write_lcd(OPCODE_WRITE_COLOR, lcd_cmd, pixels, DataMode::Quad)
    }

    fn write_lcd(
        &mut self,
        opcode: u16,
        lcd_cmd: u8,
        data: &[u8],
        data_mode: DataMode,
    ) -> Result<(), DisplayError> {
        let command = Command::_8Bit(opcode, DataMode::SingleTwoDataLines);
        let address = Address::_24Bit((lcd_cmd as u32) << 8, DataMode::SingleTwoDataLines);

        self.cs.set_low();
        let result = self
            .spi
            .half_duplex_write(data_mode, command, address, 0, data)
            .map_err(DisplayError::Spi);
        self.cs.set_high();

        result
    }
}

const fn align_up_2(value: usize) -> usize {
    (value + 1) & !1
}

impl DrawTarget for AmoledDisplay<'_> {
    type Color = Rgb565;
    type Error = DisplayError;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            self.draw_pixel(point, color)?;
        }

        self.flush_pending()
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        self.fill_rect(area, color)
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        self.fill_rect(&Rectangle::new(Point::zero(), DISPLAY_SIZE), color)
    }
}

impl OriginDimensions for AmoledDisplay<'_> {
    fn size(&self) -> Size {
        DISPLAY_SIZE
    }
}
