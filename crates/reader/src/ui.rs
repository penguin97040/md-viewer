//! The on-screen side of the reader: GPUI shaping, the pane, find and the find field.

pub mod content;
pub mod crash;
pub mod field;
pub mod images;
pub mod pane;
pub mod script;
pub mod shaper;

use std::borrow::Cow;

/// Registers the faces the reader adds to the app's bundled fonts. Call once at start-up.
pub fn init(cx: &mut gpui::App) {
    let fonts: Vec<Cow<'static, [u8]>> = vec![
        Cow::Borrowed(include_bytes!("../../../assets/fonts/Geist-Italic.ttf")),
        Cow::Borrowed(include_bytes!("../../../assets/fonts/Geist-SemiBoldItalic.ttf")),
        Cow::Borrowed(include_bytes!("../../../assets/fonts/GeistMono-SemiBold.ttf")),
        Cow::Borrowed(include_bytes!("../../../assets/fonts/GeistMono-Italic.ttf")),
        Cow::Borrowed(include_bytes!("../../../assets/fonts/GeistMono-SemiBoldItalic.ttf")),
        Cow::Borrowed(include_bytes!("../../../assets/fonts/GeistSerif-Regular.ttf")),
        Cow::Borrowed(include_bytes!("../../../assets/fonts/MDVSerif-Bold.ttf")),
        Cow::Borrowed(include_bytes!("../../../assets/fonts/MDVSerif-Italic.ttf")),
        Cow::Borrowed(include_bytes!("../../../assets/fonts/MDVSerif-BoldItalic.ttf")),
    ];
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        eprintln!("could not load the reader's fonts: {e}");
    }
    std::thread::spawn(crate::highlight::prewarm);
}
