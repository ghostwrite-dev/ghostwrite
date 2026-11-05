//! What's on the terminal's screen, copied out of the emulator to draw: its
//! cells, with colors as the program asked for them (the theme decides
//! what the sixteen and the defaults look like), and the cursor.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{self, NamedColor, Rgb};

pub struct Screen {
    /// The cells with something on them (a char, a color behind, a line
    /// under), by line from the top of the view, then column. A wide
    /// char is one cell, two columns wide.
    pub cells: Vec<Cell>,
    pub cursor: Option<Cursor>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub line: usize,
    pub col: usize,
    pub c: char,
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub dim: bool,
    pub hidden: bool,
    /// Its colors swapped.
    pub inverse: bool,
    /// Two columns wide.
    pub wide: bool,
    /// The mouse selected it.
    pub selected: bool,
}

/// A color as the program asked for it: the terminal's own, one of the
/// sixteen (which the theme colors), or exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Foreground,
    Background,
    Cursor,
    DimForeground,
    /// One of the sixteen: black, red, … white, then their bright ones.
    Ansi(u8),
    /// One of the first eight, dimmed.
    DimAnsi(u8),
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    HollowBlock,
    Beam,
    Underline,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cursor {
    pub line: usize,
    pub col: usize,
    pub shape: CursorShape,
    /// The char under it.
    pub c: char,
}

impl Screen {
    pub(crate) fn of<T: EventListener>(term: &Term<T>) -> Self {
        let content = term.renderable_content();
        let (offset, colors, selection) = (content.display_offset as i32, content.colors, content.selection);
        let mut cells = vec![];
        for indexed in content.display_iter {
            let cell = indexed.cell;
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            let selected = selection.is_some_and(|s| s.contains(indexed.point));
            let (fg, bg) = (color(cell.fg, colors), color(cell.bg, colors));
            let underline = cell.flags.intersects(Flags::ALL_UNDERLINES);
            if cell.c == ' ' && bg == Color::Background && !underline && !selected && !cell.flags.contains(Flags::INVERSE) {
                continue;
            }
            cells.push(Cell {
                line: (indexed.point.line.0 + offset) as usize,
                col: indexed.point.column.0,
                c: cell.c,
                fg,
                bg,
                bold: cell.flags.contains(Flags::BOLD),
                italic: cell.flags.contains(Flags::ITALIC),
                underline,
                strike: cell.flags.contains(Flags::STRIKEOUT),
                dim: cell.flags.contains(Flags::DIM),
                hidden: cell.flags.contains(Flags::HIDDEN),
                inverse: cell.flags.contains(Flags::INVERSE),
                wide: cell.flags.contains(Flags::WIDE_CHAR),
                selected,
            });
        }
        let at = content.cursor.point;
        let line = at.line.0 + offset;
        let shape = match content.cursor.shape {
            ansi::CursorShape::Hidden => None,
            ansi::CursorShape::Beam => Some(CursorShape::Beam),
            ansi::CursorShape::Underline => Some(CursorShape::Underline),
            ansi::CursorShape::HollowBlock => Some(CursorShape::HollowBlock),
            ansi::CursorShape::Block => Some(CursorShape::Block),
        };
        let cursor = shape.filter(|_| line >= 0 && (line as usize) < term.grid().screen_lines()).map(|shape| Cursor {
            line: line as usize,
            col: at.column.0,
            shape,
            c: term.grid()[at.line][at.column].c,
        });
        Self { cells, cursor }
    }
}

/// What `color` is: one the program set (`colors`, by escape codes) is exact.
fn color(color: ansi::Color, colors: &Colors) -> Color {
    let rgb = |c: Rgb| Color::Rgb(c.r, c.g, c.b);
    match color {
        ansi::Color::Spec(c) => rgb(c),
        ansi::Color::Indexed(i) => colors[i as usize].map_or_else(|| indexed(i), rgb),
        ansi::Color::Named(name) => {
            if let Some(c) = colors[name as usize] {
                return rgb(c);
            }
            match name {
                NamedColor::Background => Color::Background,
                NamedColor::Cursor => Color::Cursor,
                NamedColor::DimForeground => Color::DimForeground,
                n if (n as usize) < 16 => Color::Ansi(n as u8),
                n if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&(n as usize)) => {
                    Color::DimAnsi((n as usize - NamedColor::DimBlack as usize) as u8)
                }
                _ => Color::Foreground,
            }
        }
    }
}

/// One of xterm's 256 colors: the sixteen, a 6×6×6 cube, then grays.
pub(crate) fn indexed(i: u8) -> Color {
    let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
    match i {
        0..16 => Color::Ansi(i),
        16..232 => {
            let i = i - 16;
            Color::Rgb(level(i / 36), level(i / 6 % 6), level(i % 6))
        }
        _ => {
            let gray = 8 + 10 * (i - 232);
            Color::Rgb(gray, gray, gray)
        }
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    use super::*;

    #[test]
    fn cells_keep_the_colors_asked_for() {
        let mut term = Term::new(Config::default(), &crate::Size { cols: 10, lines: 3 }, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, b"a\x1b[31mb\x1b[38;5;196mc\x1b[1;7md");
        let screen = Screen::of(&term);
        let colors: Vec<(char, Color)> = screen.cells.iter().map(|c| (c.c, c.fg)).collect();
        assert_eq!(colors, [('a', Color::Foreground), ('b', Color::Ansi(1)), ('c', Color::Rgb(255, 0, 0)), ('d', Color::Rgb(255, 0, 0))]);
        assert!(screen.cells[3].bold && screen.cells[3].inverse);
        assert_eq!(screen.cursor.as_ref().map(|c| (c.line, c.col)), Some((0, 4)));
    }
}
