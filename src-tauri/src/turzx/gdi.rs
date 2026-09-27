//! 최소 GDI 캔버스 (Windows 전용, gdi32/user32 FFI).
//!
//! 외부 렌더링 의존성 없이 텍스트(한글 포함)/도형을 320x480 프레임버퍼에 그린다.
//! 결과는 RGB8 (row-major, top-down).
#![allow(dead_code)]

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    #[repr(C)]
    pub struct Rect {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }

    #[repr(C)]
    pub struct Size {
        pub cx: i32,
        pub cy: i32,
    }

    #[repr(C)]
    pub struct BitmapInfoHeader {
        pub size: u32,
        pub width: i32,
        pub height: i32,
        pub planes: u16,
        pub bit_count: u16,
        pub compression: u32,
        pub size_image: u32,
        pub x_pels: i32,
        pub y_pels: i32,
        pub clr_used: u32,
        pub clr_important: u32,
    }

    #[repr(C)]
    pub struct BitmapInfo {
        pub header: BitmapInfoHeader,
        pub colors: [u32; 3],
    }

    pub const DIB_RGB_COLORS: u32 = 0;
    pub const TRANSPARENT: i32 = 1;
    pub const PS_SOLID: i32 = 0;
    pub const DEFAULT_CHARSET: u32 = 1;
    pub const CLEARTYPE_QUALITY: u32 = 5;
    pub const FW_NORMAL: i32 = 400;
    pub const FW_BOLD: i32 = 700;

    #[link(name = "gdi32")]
    extern "system" {
        pub fn CreateCompatibleDC(hdc: *mut c_void) -> *mut c_void;
        pub fn CreateDIBSection(
            hdc: *mut c_void,
            info: *const BitmapInfo,
            usage: u32,
            bits: *mut *mut c_void,
            section: *mut c_void,
            offset: u32,
        ) -> *mut c_void;
        pub fn SelectObject(dc: *mut c_void, object: *mut c_void) -> *mut c_void;
        pub fn DeleteObject(object: *mut c_void) -> i32;
        pub fn DeleteDC(dc: *mut c_void) -> i32;
        pub fn CreateSolidBrush(color: u32) -> *mut c_void;
        pub fn CreatePen(style: i32, width: i32, color: u32) -> *mut c_void;
        pub fn FillRect(dc: *mut c_void, rect: *const Rect, brush: *mut c_void) -> i32;
        pub fn RoundRect(
            dc: *mut c_void,
            left: i32,
            top: i32,
            right: i32,
            bottom: i32,
            width: i32,
            height: i32,
        ) -> i32;
        pub fn Ellipse(dc: *mut c_void, left: i32, top: i32, right: i32, bottom: i32) -> i32;
        pub fn Rectangle(dc: *mut c_void, left: i32, top: i32, right: i32, bottom: i32) -> i32;
        pub fn MoveToEx(dc: *mut c_void, x: i32, y: i32, previous: *mut c_void) -> i32;
        pub fn LineTo(dc: *mut c_void, x: i32, y: i32) -> i32;
        pub fn CreateFontW(
            height: i32,
            width: i32,
            escapement: i32,
            orientation: i32,
            weight: i32,
            italic: u32,
            underline: u32,
            strike_out: u32,
            charset: u32,
            out_precision: u32,
            clip_precision: u32,
            quality: u32,
            pitch_and_family: u32,
            face: *const u16,
        ) -> *mut c_void;
        pub fn SetTextColor(dc: *mut c_void, color: u32) -> u32;
        pub fn SetBkMode(dc: *mut c_void, mode: i32) -> i32;
        pub fn TextOutW(dc: *mut c_void, x: i32, y: i32, text: *const u16, count: i32) -> i32;
        pub fn GetTextExtentPoint32W(
            dc: *mut c_void,
            text: *const u16,
            count: i32,
            size: *mut Size,
        ) -> i32;
        pub fn GetStockObject(index: i32) -> *mut c_void;
    }

    pub const NULL_BRUSH: i32 = 5;
}

#[cfg(windows)]
pub struct Canvas {
    width: i32,
    height: i32,
    bits: *mut u8,
    dc: *mut std::ffi::c_void,
    bitmap: *mut std::ffi::c_void,
    previous: *mut std::ffi::c_void,
    pixels: Vec<u8>,
}

#[cfg(windows)]
fn colorref(color: (u8, u8, u8)) -> u32 {
    (color.0 as u32) | ((color.1 as u32) << 8) | ((color.2 as u32) << 16)
}

#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

#[cfg(windows)]
impl Canvas {
    pub fn new(width: u16, height: u16) -> Self {
        use win::*;
        unsafe {
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            let info = BitmapInfo {
                header: BitmapInfoHeader {
                    size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                    width: width as i32,
                    height: -(height as i32), // top-down
                    planes: 1,
                    bit_count: 32,
                    compression: 0,
                    size_image: 0,
                    x_pels: 0,
                    y_pels: 0,
                    clr_used: 0,
                    clr_important: 0,
                },
                colors: [0; 3],
            };
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            let previous = SelectObject(dc, bitmap);
            let size = width as usize * height as usize * 3;
            Canvas {
                width: width as i32,
                height: height as i32,
                bits: bits as *mut u8,
                dc,
                bitmap,
                previous,
                pixels: vec![0; size],
            }
        }
    }

    fn fill_raw(&mut self, color: (u8, u8, u8)) {
        use win::*;
        unsafe {
            let brush = CreateSolidBrush(colorref(color));
            let rect = Rect { left: 0, top: 0, right: self.width, bottom: self.height };
            FillRect(self.dc, &rect, brush);
            DeleteObject(brush);
        }
    }

    pub fn rect(&mut self, x: i32, y: i32, width: i32, height: i32, color: (u8, u8, u8)) {
        use win::*;
        unsafe {
            let brush = CreateSolidBrush(colorref(color));
            let rect = Rect { left: x, top: y, right: x + width, bottom: y + height };
            FillRect(self.dc, &rect, brush);
            DeleteObject(brush);
        }
    }

    pub fn round_rect(&mut self, x: i32, y: i32, width: i32, height: i32, radius: i32, color: (u8, u8, u8)) {
        use win::*;
        unsafe {
            let brush = CreateSolidBrush(colorref(color));
            let pen = CreatePen(PS_SOLID, 1, colorref(color));
            let previous_brush = SelectObject(self.dc, brush);
            let previous_pen = SelectObject(self.dc, pen);
            let _ = RoundRect(self.dc, x, y, x + width, y + height, radius * 2, radius * 2);
            SelectObject(self.dc, previous_brush);
            SelectObject(self.dc, previous_pen);
            DeleteObject(brush);
            DeleteObject(pen);
        }
    }

    pub fn circle(&mut self, cx: i32, cy: i32, radius: i32, color: (u8, u8, u8)) {
        use win::*;
        unsafe {
            let brush = CreateSolidBrush(colorref(color));
            let pen = CreatePen(PS_SOLID, 1, colorref(color));
            let previous_brush = SelectObject(self.dc, brush);
            let previous_pen = SelectObject(self.dc, pen);
            let _ = Ellipse(self.dc, cx - radius, cy - radius, cx + radius, cy + radius);
            SelectObject(self.dc, previous_brush);
            SelectObject(self.dc, previous_pen);
            DeleteObject(brush);
            DeleteObject(pen);
        }
    }

    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: (u8, u8, u8)) {
        use win::*;
        unsafe {
            let pen = CreatePen(PS_SOLID, 1, colorref(color));
            let previous = SelectObject(self.dc, pen);
            MoveToEx(self.dc, x0, y0, std::ptr::null_mut());
            LineTo(self.dc, x1, y1);
            SelectObject(self.dc, previous);
            DeleteObject(pen);
        }
    }

    pub fn text_width(&self, text: &str, size: i32, bold: bool) -> i32 {
        use win::*;
        unsafe {
            let font = self.font(size, bold);
            let previous = SelectObject(self.dc, font);
            let utf16 = wide(text);
            let mut size_struct = Size { cx: 0, cy: 0 };
            GetTextExtentPoint32W(self.dc, utf16.as_ptr(), utf16.len() as i32, &mut size_struct);
            SelectObject(self.dc, previous);
            DeleteObject(font);
            size_struct.cx
        }
    }

    unsafe fn font(&self, size: i32, bold: bool) -> *mut std::ffi::c_void {
        use win::*;
        let face = wide("Malgun Gothic");
        CreateFontW(
            -size,
            0,
            0,
            0,
            if bold { FW_BOLD } else { FW_NORMAL },
            0,
            0,
            0,
            DEFAULT_CHARSET,
            0,
            0,
            CLEARTYPE_QUALITY,
            0,
            face.as_ptr(),
        )
    }

    /// 텍스트를 (x, y) 기준 상단-왼쪽으로 그린다.
    pub fn text(&mut self, text: &str, x: i32, y: i32, size: i32, color: (u8, u8, u8), bold: bool) {
        use win::*;
        unsafe {
            let font = self.font(size, bold);
            let previous = SelectObject(self.dc, font);
            SetBkMode(self.dc, TRANSPARENT);
            SetTextColor(self.dc, colorref(color));
            let utf16 = wide(text);
            TextOutW(self.dc, x, y, utf16.as_ptr(), utf16.len() as i32);
            SelectObject(self.dc, previous);
            DeleteObject(font);
        }
    }

    pub fn text_right(&mut self, text: &str, right: i32, y: i32, size: i32, color: (u8, u8, u8), bold: bool) {
        let width = self.text_width(text, size, bold);
        self.text(text, right - width, y, size, color, bold);
    }

    /// 진행 바(둥근 모서리).
    pub fn bar(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        percent: f64,
        foreground: (u8, u8, u8),
        background: (u8, u8, u8),
    ) {
        let radius = (height / 2).max(1);
        self.round_rect(x, y, width, height, radius, background);
        let clamped = percent.clamp(0.0, 100.0);
        let filled = ((width as f64) * clamped / 100.0).round() as i32;
        if filled > 1 {
            self.round_rect(x, y, filled.max(radius * 2), height, radius, foreground);
        }
    }

    /// GDI 출력을 RGB 버퍼로 확정한다.
    pub fn finish(&mut self) -> &[u8] {
        unsafe {
            let count = (self.width as usize) * (self.height as usize);
            for index in 0..count {
                let offset = index * 4;
                let bgra = std::slice::from_raw_parts(self.bits.add(offset), 4);
                self.pixels[index * 3] = bgra[2];
                self.pixels[index * 3 + 1] = bgra[1];
                self.pixels[index * 3 + 2] = bgra[0];
            }
        }
        &self.pixels
    }
}

#[cfg(windows)]
impl Drop for Canvas {
    fn drop(&mut self) {
        use win::*;
        unsafe {
            SelectObject(self.dc, self.previous);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

/// 비 Windows(테스트 등)에서는 단색 캔버스로 대체한다.
#[cfg(not(windows))]
pub struct Canvas {
    width: i32,
    height: i32,
    pixels: Vec<u8>,
}

#[cfg(not(windows))]
impl Canvas {
    pub fn new(width: u16, height: u16) -> Self {
        Canvas {
            width: width as i32,
            height: height as i32,
            pixels: vec![0; width as usize * height as usize * 3],
        }
    }

    pub fn rect(&mut self, _x: i32, _y: i32, _w: i32, _h: i32, _color: (u8, u8, u8)) {}
    pub fn round_rect(&mut self, _x: i32, _y: i32, _w: i32, _h: i32, _r: i32, _c: (u8, u8, u8)) {}
    pub fn circle(&mut self, _cx: i32, _cy: i32, _r: i32, _c: (u8, u8, u8)) {}
    pub fn line(&mut self, _x0: i32, _y0: i32, _x1: i32, _y1: i32, _c: (u8, u8, u8)) {}
    pub fn text(&mut self, _t: &str, _x: i32, _y: i32, _s: i32, _c: (u8, u8, u8), _b: bool) {}
    pub fn text_right(&mut self, _t: &str, _r: i32, _y: i32, _s: i32, _c: (u8, u8, u8), _b: bool) {}
    pub fn text_width(&self, text: &str, size: i32, _bold: bool) -> i32 {
        (text.chars().count() as i32) * (size / 2)
    }
    pub fn bar(&mut self, _x: i32, _y: i32, _w: i32, _h: i32, _p: f64, _f: (u8, u8, u8), _b: (u8, u8, u8)) {}
    pub fn finish(&mut self) -> &[u8] {
        &self.pixels
    }
}
