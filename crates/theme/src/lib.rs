//! Colour and metric tokens for MD Viewer, shared by every crate.
//!
//! Brand values come from `design/tokens.json` through `generated.rs`. Light-theme values and
//! the ones tokens.json lacks (`--code-text`, `--mark`, `--mark-current`) are copied by hand from
//! the v0.2 style sheet. Colours are plain 0xRRGGBB so this crate needs no GPUI dependency.

mod generated;
pub use generated::*;

/// Which colour scheme is active (`data-theme` in the web app).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Dark,
    Light,
}

/// The CSS custom properties of one theme, as 0xRRGGBB.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub bg: u32,
    pub bg_bar: u32,
    pub bg_soft: u32,
    pub bg_code: u32,
    pub bg_hover: u32,
    pub text: u32,
    pub text_strong: u32,
    pub muted: u32,
    pub border: u32,
    pub accent: u32,
    pub code_text: u32,
    pub mark: u32,
    pub mark_current: u32,
}

/// style.css lines 20-35. `--bg*` and `--text` come from the brand tokens.
pub const DARK: Palette = Palette {
    bg: BRAND_BLACK,
    bg_bar: BRAND_BLACK,
    bg_soft: BRAND_BLACK,
    bg_code: BRAND_BLACK,
    bg_hover: BRAND_LINE,
    text: BRAND_TEXT,
    text_strong: BRAND_TEXT,
    muted: BRAND_MUTED,
    border: BRAND_LINE,
    accent: BRAND_BEAK,
    code_text: 0x79c0ff,
    mark: 0x654000,
    mark_current: 0xdb7700,
};

/// style.css lines 38-53.
pub const LIGHT: Palette = Palette {
    bg: 0xffffff,
    bg_bar: 0xffffff,
    bg_soft: 0xffffff,
    bg_code: 0xffffff,
    bg_hover: 0xeeeeee,
    text: 0x171717,
    text_strong: 0x000000,
    muted: 0x626262,
    border: 0xd0d0d0,
    accent: 0xa34f00,
    code_text: 0x0550ae,
    mark: 0xffe0b8,
    mark_current: 0xf0af60,
};

impl Mode {
    pub fn palette(self) -> &'static Palette {
        match self {
            Mode::Dark => &DARK,
            Mode::Light => &LIGHT,
        }
    }
    pub fn toggled(self) -> Mode {
        match self {
            Mode::Dark => Mode::Light,
            Mode::Light => Mode::Dark,
        }
    }
}

/// Layout metrics in px, from the v0.2 style sheet.
pub mod metrics {
    /// `#bar` height (81-90).
    pub const BAR_HEIGHT: f32 = 48.0;
    /// `#bar` horizontal padding (86); 4px in the narrow branch (613).
    pub const BAR_PADDING_X: f32 = 6.0;
    /// `[data-platform=mac] #bar` left padding (594), room for the traffic lights.
    pub const MAC_BAR_INSET: f32 = 88.0;
    /// `@media (max-width: 819px)` (611).
    pub const NARROW_MAX_WIDTH: f32 = 819.0;
    /// `.icon` (152-163, 584): 32 wide, 36 tall inside the bar; svg 18px, stroke 1.7.
    pub const ICON_W: f32 = 32.0;
    pub const ICON_H: f32 = 36.0;
    pub const ICON_SVG: f32 = 18.0;
    pub const ICON_STROKE: f32 = 1.7;
    /// `.icon.small` (176-177).
    pub const ICON_SMALL: f32 = 26.0;
    pub const ICON_SMALL_SVG: f32 = 16.0;
    /// `#window-controls .icon` (590-591).
    pub const WINCTL_W: f32 = 40.0;
    pub const WINCTL_SVG: f32 = 15.0;
    /// `.tab` (105-150, 585-587): min 90, basis 200, 12px left and 4px right padding, 2px underline.
    pub const TAB_BASIS: f32 = 200.0;
    pub const TAB_MIN: f32 = 90.0;
    pub const TAB_X: f32 = 20.0;
    pub const TAB_X_SVG: f32 = 14.0;
    /// `#toc` (301-308) and `.toc-item` (311-329).
    pub const TOC_WIDTH: f32 = 270.0;
    pub const TOC_ROW: f32 = 28.0;
    pub const TOC_FONT: f32 = 13.0;
    pub const TOC_INDENT: f32 = 14.0;
    pub const TOC_PAD_LEFT: f32 = 14.0;
    /// `#findbar` (181-211) and `#webbar` (214-233).
    pub const FIND_TOP: f32 = 54.0;
    pub const FIND_RIGHT: f32 = 14.0;
    pub const FIND_INPUT_W: f32 = 220.0;
    /// `.popover` (236-248).
    pub const POPOVER_W: f32 = 300.0;
    pub const POPOVER_TOP: f32 = 54.0;
    pub const POPOVER_RIGHT: f32 = 10.0;
    pub const ROW_MIN_H: f32 = 38.0;
    /// `#toast` and `#webbar` distance from the bottom (215-218, 368-372).
    pub const TOAST_BOTTOM: f32 = 22.0;
    /// `::-webkit-scrollbar` (384-387): 12px wide, thumb with a 3px transparent border.
    pub const SCROLLBAR: f32 = 12.0;
    pub const SCROLLBAR_THUMB_INSET: f32 = 3.0;
    /// `body::after` outline (71-78).
    pub const OUTLINE: f32 = 1.0;
}
