use core::fmt::Write;
use embedded_graphics::{
    mono_font::{ascii::FONT_8X13, MonoTextStyleBuilder},
    pixelcolor::BinaryColor,
    prelude::*,
    text::{Baseline, Text},
};
use embedded_hal::i2c::I2c;
use ssd1306::{
    mode::BufferedGraphicsMode, prelude::*, size::DisplaySize128x64, I2CDisplayInterface,
    Ssd1306,
};

pub struct OledScreen<I2C> {
    display: Ssd1306<
        I2CInterface<I2C>,
        DisplaySize128x64,
        BufferedGraphicsMode<DisplaySize128x64>,
    >,
}

impl<I2C> OledScreen<I2C>
where
    I2C: I2c,
{
    pub fn new(i2c: I2C) -> Result<Self, ()> {
        let interface = I2CDisplayInterface::new(i2c);
        let mut display = Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
            .into_buffered_graphics_mode();

        display.init().map_err(|_| ())?;

        let mut screen = Self { display };
        screen.clear();
        Ok(screen)
    }

    pub fn clear(&mut self) {
        self.display.clear_buffer();
        let _ = self.display.flush();
    }

    pub fn show_boot(&mut self, step: &str, detail: &str) {
        self.display.clear_buffer();

        let style = MonoTextStyleBuilder::new()
            .font(&FONT_8X13)
            .text_color(BinaryColor::On)
            .build();

        let mut buf_title = [b' '; 16];
        let mut buf_detail = [b' '; 16];

        let mut w1 = BufferWriter::new(&mut buf_title);
        let _ = write!(w1, "PLANTY");

        let mut w2 = BufferWriter::new(&mut buf_detail);
        let _ = write!(w2, "{}", step);

        let mut buf3 = [b' '; 16];
        let mut w3 = BufferWriter::new(&mut buf3);
        let _ = write!(w3, "{}", detail);

        Text::with_baseline(w1.as_str(), Point::new(0, 1), style, Baseline::Top)
            .draw(&mut self.display)
            .ok();

        Text::with_baseline(w2.as_str(), Point::new(0, 24), style, Baseline::Top)
            .draw(&mut self.display)
            .ok();

        Text::with_baseline(w3.as_str(), Point::new(0, 48), style, Baseline::Top)
            .draw(&mut self.display)
            .ok();

        let _ = self.display.flush();
    }

    pub fn show_metrics(&mut self, raw: u16, percentage: u8, wifi_ok: bool, mqtt_ok: bool) {
        self.display.clear_buffer();

        let style = MonoTextStyleBuilder::new()
            .font(&FONT_8X13)
            .text_color(BinaryColor::On)
            .build();

        let mut buf1 = [b' '; 16];
        let mut buf2 = [b' '; 16];
        let mut buf3 = [b' '; 16];

        let state = if percentage < 30 { "DRY!" } else { "OK  " };

        let mut writer1 = BufferWriter::new(&mut buf1);
        let _ = write!(writer1, "Raw: {:>4}", raw);

        let mut writer2 = BufferWriter::new(&mut buf2);
        let _ = write!(writer2, "Hum: {:>3}% [{}]", percentage, state);

        let mut writer3 = BufferWriter::new(&mut buf3);
        let w = if wifi_ok { "OK" } else { "! " };
        let m = if mqtt_ok { "OK" } else { "! " };
        let _ = write!(writer3, "W:{}  M:{}", w, m);

        Text::with_baseline(writer1.as_str(), Point::new(0, 1), style, Baseline::Top)
            .draw(&mut self.display)
            .ok();

        Text::with_baseline(writer2.as_str(), Point::new(0, 24), style, Baseline::Top)
            .draw(&mut self.display)
            .ok();

        Text::with_baseline(writer3.as_str(), Point::new(0, 48), style, Baseline::Top)
            .draw(&mut self.display)
            .ok();

        let _ = self.display.flush();
    }
}

struct BufferWriter<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> BufferWriter<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        buf.fill(b' ');
        Self { buf, pos: 0 }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(self.buf).unwrap_or("")
    }
}

impl<'a> Write for BufferWriter<'a> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let rem = self.buf.len().saturating_sub(self.pos);
        let to_write = bytes.len().min(rem);
        if to_write > 0 {
            self.buf[self.pos..self.pos + to_write].copy_from_slice(&bytes[..to_write]);
            self.pos += to_write;
        }
        Ok(())
    }
}
