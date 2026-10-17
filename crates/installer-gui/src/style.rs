//! Visual constants and widget styles. Sizes are deliberately generous for
//! readability.

use iced::widget::{button, container};
use iced::{Border, Color, Theme};

pub const TITLE: f32 = 28.0;
pub const HEADING: f32 = 22.0;
pub const BODY: f32 = 16.0;
pub const SMALL: f32 = 14.0;
pub const MONO: f32 = 13.0;

pub const WINDOW_W: f32 = 680.0;
pub const WINDOW_H: f32 = 560.0;

/// Primary call-to-action button, with a focus ring when keyboard-focused.
pub fn primary(focused: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| with_focus(button::primary(theme, status), theme, focused)
}

pub fn secondary(focused: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| with_focus(button::secondary(theme, status), theme, focused)
}

pub fn danger(focused: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| with_focus(button::danger(theme, status), theme, focused)
}

pub fn link(focused: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| with_focus(button::text(theme, status), theme, focused)
}

fn with_focus(mut style: button::Style, theme: &Theme, focused: bool) -> button::Style {
    style.border = Border {
        radius: 8.0.into(),
        ..style.border
    };
    if focused {
        let palette = theme.extended_palette();
        style.border = Border {
            color: palette.primary.strong.color,
            width: 3.0,
            radius: 8.0.into(),
        };
    }
    style
}

pub fn card(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.background.weak.color.into()),
        border: Border {
            color: palette.background.strong.color,
            width: 1.0,
            radius: 10.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn log_box(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.background.base.color.into()),
        border: Border {
            color: palette.background.strong.color,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn muted(theme: &Theme) -> Color {
    theme.extended_palette().background.strong.text
}

pub fn success(theme: &Theme) -> Color {
    theme.extended_palette().success.base.color
}

pub fn error(theme: &Theme) -> Color {
    theme.extended_palette().danger.base.color
}
