//! Число работающих серверов картинкой: для значка в трее и для метки на кнопке в панели задач.
//!
//! Готового системного счётчика у обычных программ Windows нет (он есть только у приложений
//! из Магазина), поэтому метка рисуется здесь, но так, чтобы выглядеть системной: ровный кружок
//! и цифра шрифтом Segoe UI. Цифра растрируется самой Windows в четыре раза крупнее нужного
//! и уменьшается: на 16 точках экрана края остаются гладкими.

use std::{ffi::c_void, mem, ptr};
use windows_sys::Win32::{
    Foundation::RECT,
    Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW, GdiFlush,
        SelectObject, SetBkMode, SetTextColor, BITMAPINFO,
    },
};

/// Сторона обеих картинок.
pub const SIZE: u32 = 32;
/// Во сколько раз крупнее рисуется цифра перед уменьшением.
const SUPER: u32 = 4;

const MINT: [u8; 3] = [125, 226, 195];
const DARK: [u8; 3] = [10, 12, 13];
const GREEN: [u8; 3] = [26, 158, 88];
const WHITE: [u8; 3] = [255, 255, 255];

pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    /// Точки построчно, по четыре байта: красный, зелёный, синий, непрозрачность.
    pub rgba: Vec<u8>,
}

impl Bitmap {
    fn new() -> Bitmap {
        Bitmap { width: SIZE, height: SIZE, rgba: vec![0; (SIZE * SIZE * 4) as usize] }
    }

    fn put(&mut self, x: u32, y: u32, color: [u8; 3], alpha: f32) {
        let i = ((y * self.width + x) * 4) as usize;
        self.rgba[i..i + 3].copy_from_slice(&color);
        self.rgba[i + 3] = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    }

    /// Кладёт цвет поверх уже нарисованного с заданной плотностью, не трогая прозрачность фона.
    fn blend(&mut self, x: u32, y: u32, color: [u8; 3], coverage: f32) {
        let i = ((y * self.width + x) * 4) as usize;
        for c in 0..3 {
            let under = self.rgba[i + c] as f32;
            self.rgba[i + c] = (under + (color[c] as f32 - under) * coverage).round() as u8;
        }
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }
}

/// Что написать: само число до девяти, дальше «9+». Двузначное на 16 точках не читается.
fn label(count: usize) -> String {
    if count > 9 { "9+".into() } else { count.to_string() }
}

/// Плотность текста в каждой точке картинки, от 0 до 1: текст жирным Segoe UI высотой `height`
/// точек, выставленный по центру по своим настоящим границам, а не по строке шрифта.
/// У цифр нет нижних выносных элементов, и по строке они оказались бы выше середины.
fn text_coverage(text: &str, height: u32) -> Vec<f32> {
    let big = SIZE * SUPER;
    let mask = rasterize(text, big, (height * SUPER) as i32);

    // Границы того, что реально закрашено.
    let (mut left, mut top, mut right, mut bottom) = (big, big, 0, 0);
    for y in 0..big {
        for x in 0..big {
            if mask[(y * big + x) as usize] > 24 {
                left = left.min(x);
                right = right.max(x + 1);
                top = top.min(y);
                bottom = bottom.max(y + 1);
            }
        }
    }
    let mut coverage = vec![0.0; (SIZE * SIZE) as usize];
    if right <= left || bottom <= top {
        return coverage;
    }
    // Сдвиг, который ставит закрашенный прямоугольник в середину крупной картинки.
    let shift_x = (big as i32 - (right - left) as i32) / 2 - left as i32;
    let shift_y = (big as i32 - (bottom - top) as i32) / 2 - top as i32;

    for y in 0..SIZE {
        for x in 0..SIZE {
            let mut sum = 0u32;
            for sy in 0..SUPER {
                for sx in 0..SUPER {
                    let from_x = (x * SUPER + sx) as i32 - shift_x;
                    let from_y = (y * SUPER + sy) as i32 - shift_y;
                    if (0..big as i32).contains(&from_x) && (0..big as i32).contains(&from_y) {
                        sum += mask[(from_y as u32 * big + from_x as u32) as usize] as u32;
                    }
                }
            }
            coverage[(y * SIZE + x) as usize] = sum as f32 / (SUPER * SUPER * 255) as f32;
        }
    }
    coverage
}

/// Рисует текст белым по чёрному средствами Windows и возвращает яркость каждой точки.
/// Если шрифт или холст создать не удалось, возвращает пустую картинку: метка останется без цифры.
fn rasterize(text: &str, size: u32, font_height: i32) -> Vec<u8> {
    const FW_BOLD: i32 = 700;
    const ANTIALIASED_QUALITY: u32 = 4;
    const DEFAULT_CHARSET: u32 = 1;
    const TRANSPARENT: i32 = 1;
    const DT_CENTER_VCENTER_SINGLELINE_NOPREFIX: u32 = 0x1 | 0x4 | 0x20 | 0x800;

    let mut out = vec![0u8; (size * size) as usize];
    let wide: Vec<u16> = text.encode_utf16().collect();
    let face: Vec<u16> = "Segoe UI".encode_utf16().chain(Some(0)).collect();

    // SAFETY: все объекты GDI создаются и освобождаются здесь же; буфер точек принадлежит
    // холсту и читается до его удаления, не дальше размера `size × size × 4`.
    unsafe {
        let dc = CreateCompatibleDC(ptr::null_mut());
        if dc.is_null() {
            return out;
        }
        let mut info: BITMAPINFO = mem::zeroed();
        info.bmiHeader.biSize = mem::size_of_val(&info.bmiHeader) as u32;
        info.bmiHeader.biWidth = size as i32;
        // Отрицательная высота: строки сверху вниз, как в остальном коде.
        info.bmiHeader.biHeight = -(size as i32);
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;

        let mut bits: *mut c_void = ptr::null_mut();
        let canvas = CreateDIBSection(dc, &info, 0, &mut bits, ptr::null_mut(), 0);
        let font = CreateFontW(
            -font_height, 0, 0, 0, FW_BOLD as _, 0, 0, 0,
            DEFAULT_CHARSET as _, 0, 0, ANTIALIASED_QUALITY as _, 0, face.as_ptr(),
        );
        if !canvas.is_null() && !bits.is_null() && !font.is_null() {
            ptr::write_bytes(bits as *mut u8, 0, (size * size * 4) as usize);
            let old_canvas = SelectObject(dc, canvas);
            let old_font = SelectObject(dc, font);
            SetBkMode(dc, TRANSPARENT as _);
            SetTextColor(dc, 0x00FF_FFFF);
            let mut area = RECT { left: 0, top: 0, right: size as i32, bottom: size as i32 };
            DrawTextW(dc, wide.as_ptr(), wide.len() as i32, &mut area, DT_CENTER_VCENTER_SINGLELINE_NOPREFIX as _);
            GdiFlush();

            let pixels = std::slice::from_raw_parts(bits as *const u8, (size * size * 4) as usize);
            for (i, px) in pixels.chunks_exact(4).enumerate() {
                // Текст белый, так что любой канал — это плотность; берём зелёный.
                out[i] = px[1];
            }
            SelectObject(dc, old_font);
            SelectObject(dc, old_canvas);
        }
        if !font.is_null() {
            DeleteObject(font);
        }
        if !canvas.is_null() {
            DeleteObject(canvas);
        }
        DeleteDC(dc);
    }
    out
}

fn write(bitmap: &mut Bitmap, count: usize, height: u32, color: [u8; 3]) {
    let coverage = text_coverage(&label(count), height);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let c = coverage[(y * SIZE + x) as usize];
            if c > 0.0 {
                bitmap.blend(x, y, color, c);
            }
        }
    }
}

/// Значок для трея, пока серверы работают: тот же мятный квадрат приложения, но вместо рисунка
/// сервера на нём число. Маленькая метка в углу значка 16 на 16 была бы нечитаема.
pub fn tray(count: usize) -> Bitmap {
    let mut bitmap = Bitmap::new();
    let (lo, hi, radius) = (1.0_f32, SIZE as f32 - 1.0, 7.0_f32);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // Расстояние до скруглённого квадрата: внутри отрицательное.
            let dx = (lo + radius - px).max(px - (hi - radius)).max(0.0);
            let dy = (lo + radius - py).max(py - (hi - radius)).max(0.0);
            let outside = (dx * dx + dy * dy).sqrt() - radius;
            bitmap.put(x, y, MINT, 0.5 - outside);
        }
    }
    write(&mut bitmap, count, if count > 9 { 21 } else { 29 }, DARK);
    bitmap
}

/// Метка поверх кнопки приложения в панели задач: зелёный кружок с белым числом,
/// как счётчики у других программ. Тонкий тёмный край отделяет его от мятного значка под ним.
pub fn overlay(count: usize) -> Bitmap {
    let mut bitmap = Bitmap::new();
    let center = SIZE as f32 / 2.0;
    let (radius, rim) = (center, 1.5_f32);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (dx, dy) = (x as f32 + 0.5 - center, y as f32 + 0.5 - center);
            let distance = (dx * dx + dy * dy).sqrt();
            bitmap.put(x, y, DARK, radius - distance + 0.5);
            // Заливка поверх тёмного круга чуть меньшего радиуса: остаётся ровный тёмный край.
            let fill = radius - rim - distance + 0.5;
            if fill > 0.0 {
                bitmap.blend(x, y, GREEN, fill.min(1.0));
            }
        }
    }
    write(&mut bitmap, count, if count > 9 { 19 } else { 25 }, WHITE);
    bitmap
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Точки, цвет которых заметно отличается от заливки: это и есть цифра.
    fn ink(bitmap: &Bitmap, fill: [u8; 3]) -> Vec<(u32, u32)> {
        let mut points = Vec::new();
        // Края не смотрим: там сглаживание формы и тёмный ободок метки.
        for y in 4..SIZE - 4 {
            for x in 4..SIZE - 4 {
                let p = bitmap.pixel(x, y);
                let far = (0..3).map(|c| (p[c] as i32 - fill[c] as i32).abs()).max().unwrap();
                if far > 90 {
                    points.push((x, y));
                }
            }
        }
        points
    }

    fn middle(points: &[(u32, u32)]) -> (f32, f32) {
        let xs = points.iter().map(|p| p.0);
        let ys = points.iter().map(|p| p.1);
        (
            (xs.clone().min().unwrap() + xs.max().unwrap() + 1) as f32 / 2.0,
            (ys.clone().min().unwrap() + ys.max().unwrap() + 1) as f32 / 2.0,
        )
    }

    #[test]
    fn pictures_have_the_declared_size() {
        for bitmap in [tray(3), overlay(3)] {
            assert_eq!((bitmap.width, bitmap.height), (SIZE, SIZE));
            assert_eq!(bitmap.rgba.len(), (SIZE * SIZE * 4) as usize);
        }
    }

    #[test]
    fn corners_are_transparent_and_shape_is_filled() {
        let t = tray(1);
        assert_eq!(t.pixel(0, 0)[3], 0);
        assert_eq!(t.pixel(SIZE - 1, SIZE - 1)[3], 0);
        assert_eq!(t.pixel(3, 16), [MINT[0], MINT[1], MINT[2], 255]);

        let o = overlay(1);
        assert_eq!(o.pixel(0, 0)[3], 0);
        // Самый край тёмный, дальше зелёная заливка.
        assert_eq!(&o.pixel(16, 0)[..3], &DARK);
        assert_eq!(o.pixel(4, 16), [GREEN[0], GREEN[1], GREEN[2], 255]);
    }

    #[test]
    fn digit_is_drawn_large_and_centered() {
        for count in [1, 4, 8, 12] {
            for (bitmap, fill, min_height) in [(tray(count), MINT, 16.0), (overlay(count), GREEN, 12.0)] {
                let points = ink(&bitmap, fill);
                assert!(points.len() > 30, "число {count}: цифра не нарисована");
                let (cx, cy) = middle(&points);
                assert!((cx - 16.0).abs() <= 1.5 && (cy - 16.0).abs() <= 1.5, "число {count}: центр ({cx}, {cy})");
                let top = points.iter().map(|p| p.1).min().unwrap();
                let bottom = points.iter().map(|p| p.1).max().unwrap();
                let height = (bottom - top + 1) as f32;
                if count <= 9 {
                    assert!(height >= min_height, "число {count}: высота цифры {height}");
                }
            }
        }
    }

    #[test]
    fn every_count_looks_different() {
        let pictures: Vec<Vec<u8>> = (1..=10).map(|n| overlay(n).rgba).collect();
        for a in 0..pictures.len() {
            for b in (a + 1)..pictures.len() {
                assert_ne!(pictures[a], pictures[b], "{} и {} нарисованы одинаково", a + 1, b + 1);
            }
        }
    }

    #[test]
    fn ten_and_more_is_nine_plus() {
        assert_eq!(label(9), "9");
        assert_eq!(label(10), "9+");
        assert_eq!(tray(10).rgba, tray(250).rgba);
        assert_ne!(tray(9).rgba, tray(10).rgba);
    }

    /// Выгружает картинки сырыми точками, чтобы посмотреть на них глазами:
    /// `LSM_BADGE_DUMP=<папка> cargo test -p lsm-core dump_pictures -- --ignored`.
    #[test]
    #[ignore]
    fn dump_pictures() {
        let dir = std::path::PathBuf::from(std::env::var("LSM_BADGE_DUMP").expect("задайте LSM_BADGE_DUMP"));
        for count in [1, 3, 8, 12] {
            std::fs::write(dir.join(format!("tray-{count}.rgba")), tray(count).rgba).unwrap();
            std::fs::write(dir.join(format!("overlay-{count}.rgba")), overlay(count).rgba).unwrap();
        }
    }
}
