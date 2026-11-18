use slint::Color;
use vt100::{Color as VtColor, Parser};

#[derive(Clone, Debug)]
pub struct ScreenCell {
    pub text: String,
    pub foreground: Color,
    pub background: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

pub fn new_parser(cols: u16, rows: u16) -> Parser {
    Parser::new(
        std::num::NonZeroU16::new(rows.max(1)).expect("non-zero rows"),
        std::num::NonZeroU16::new(cols.max(1)).expect("non-zero cols"),
        2_000,
    )
}

pub fn snapshot(parser: &Parser) -> Vec<ScreenCell> {
    let (rows, cols) = parser.screen().size();
    let (rows, cols) = (rows.get(), cols.get());
    let cursor = parser.screen().cursor_position();
    let show_cursor = !parser.screen().hide_cursor();
    let mut cells = Vec::with_capacity(usize::from(rows) * usize::from(cols));
    for row in 0..rows {
        for col in 0..cols {
            let cell = parser.screen().cell(row, col);
            let (fg, bg) = cell
                .map(|cell| (cell.fgcolor(), cell.bgcolor()))
                .unwrap_or((VtColor::Default, VtColor::Default));
            let cursor_here = show_cursor && cursor == (row, col);
            let mut fg = fg;
            let mut bg = bg;
            if cell.is_some_and(|cell| cell.inverse()) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if cursor_here {
                std::mem::swap(&mut fg, &mut bg);
            }
            let dim = cell.is_some_and(|cell| cell.dim());
            let fg_color = to_slint_color(fg, true);
            let bg_color = to_slint_color(bg, false);
            cells.push(ScreenCell {
                text: cell
                    .map(|cell| cell.contents().to_owned())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| " ".into()),
                foreground: if dim {
                    blend(fg_color, bg_color, 0.55)
                } else {
                    fg_color
                },
                background: bg_color,
                bold: cell.is_some_and(|cell| cell.bold()),
                italic: cell.is_some_and(|cell| cell.italic()),
                underline: cell.is_some_and(|cell| cell.underline()),
            });
        }
    }
    cells
}

fn blend(foreground: Color, background: Color, amount: f32) -> Color {
    let f = [
        foreground.alpha(),
        foreground.red(),
        foreground.green(),
        foreground.blue(),
    ];
    let b = [
        background.alpha(),
        background.red(),
        background.green(),
        background.blue(),
    ];
    let mix = |a: u8, b: u8| (f32::from(a) * amount + f32::from(b) * (1.0 - amount)) as u8;
    Color::from_argb_u8(f[0], mix(f[1], b[1]), mix(f[2], b[2]), mix(f[3], b[3]))
}

fn to_slint_color(color: VtColor, foreground: bool) -> Color {
    match color {
        VtColor::Default => {
            if foreground {
                Color::from_argb_u8(255, 229, 231, 235)
            } else {
                Color::from_argb_u8(255, 16, 18, 24)
            }
        }
        VtColor::Rgb(r, g, b) => Color::from_argb_u8(255, r, g, b),
        VtColor::Idx(index) => {
            let (r, g, b) = if index < 16 {
                const BASIC: [(u8, u8, u8); 16] = [
                    (0, 0, 0),
                    (205, 49, 49),
                    (13, 188, 121),
                    (229, 229, 16),
                    (36, 114, 200),
                    (188, 63, 188),
                    (17, 168, 205),
                    (229, 229, 229),
                    (102, 102, 102),
                    (241, 76, 76),
                    (35, 209, 139),
                    (245, 245, 67),
                    (59, 142, 234),
                    (214, 112, 214),
                    (41, 184, 219),
                    (255, 255, 255),
                ];
                BASIC[usize::from(index)]
            } else if index < 232 {
                let n = index - 16;
                let component = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                (component(n / 36), component((n / 6) % 6), component(n % 6))
            } else {
                let gray = 8 + (index - 232) * 10;
                (gray, gray, gray)
            };
            Color::from_argb_u8(255, r, g, b)
        }
    }
}
