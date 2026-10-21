//! Semantic design tokens. Both variants are built from the same token set; views read tokens
//! through `cx.theme()` and never hard-code colors.

use gpui::{App, Global, Hsla, Pixels, SharedString, px, rgba};
use pipkin_core::{TextSize, Theme as ThemeChoice};

#[derive(Clone, Debug)]
pub struct Colors {
    pub bg_app: Hsla,
    pub bg_pane: Hsla,
    pub bg_surface: Hsla,
    pub bg_elevated: Hsla,
    pub bg_input: Hsla,
    /// Raised cards inside a pane: tool rows, file lists.
    pub bg_card: Hsla,
    /// The changes pane, a shade apart from the transcript.
    pub bg_changes: Hsla,
    pub bg_hover: Hsla,
    pub bg_active: Hsla,
    pub bg_selected: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_faint: Hsla,
    /// Amber for text, icons and rings: darker in the light theme so it stays readable.
    pub accent: Hsla,
    /// The brand amber for fills (primary buttons, the active marker), with `accent_text` on it.
    pub accent_fill: Hsla,
    pub accent_text: Hsla,
    pub accent_bg: Hsla,
    pub text_selection: Hsla,
    pub success: Hsla,
    /// Active work; distinct from amber attention and green completion.
    pub activity: Hsla,
    pub warning: Hsla,
    pub danger: Hsla,
    pub danger_bg: Hsla,
    pub code_bg: Hsla,
    pub syn_keyword: Hsla,
    pub syn_string: Hsla,
    pub syn_comment: Hsla,
    pub syn_number: Hsla,
    pub syn_type: Hsla,
    pub syn_function: Hsla,
    pub diff_add_bg: Hsla,
    pub diff_remove_bg: Hsla,
    pub diff_add_text: Hsla,
    pub diff_remove_text: Hsla,
    pub diff_hunk_bg: Hsla,
    pub scrim: Hsla,
}

fn c(hex: u32) -> Hsla {
    rgba(hex).into()
}

impl Colors {
    /// Pipkin's brand palette (see the brand guide): Midnight, Slate, Graphite, Amber, with the
    /// website demo's blue-tinted panes.
    pub fn dark() -> Self {
        Colors {
            bg_app: c(0x080f1aff),
            bg_pane: c(0x0e1b2aff),
            bg_surface: c(0x101e2eff),
            bg_elevated: c(0x1c2b3eff),
            bg_input: c(0x101d2dff),
            bg_card: c(0x142235ff),
            bg_changes: c(0x0d1926ff),
            bg_hover: c(0xffffff0d),
            bg_active: c(0xffffff16),
            bg_selected: c(0x283449ff),
            border: c(0x24364aff),
            border_strong: c(0x42617fff),
            text: c(0xedf1f5ff),
            text_muted: c(0xacb7c5ff),
            text_faint: c(0x8b96a7ff),
            accent: c(0xf5b942ff),
            accent_fill: c(0xf5b942ff),
            accent_text: c(0x080f1aff),
            accent_bg: c(0xf59e0b24),
            text_selection: c(0xf59e0b50),
            success: c(0x8bdda6ff),
            activity: c(0x37d4c7ff),
            warning: c(0xfb6f1aff),
            danger: c(0xff6b63ff),
            danger_bg: c(0xff6b631f),
            code_bg: c(0x0c1724ff),
            syn_keyword: c(0xc792eaff),
            syn_string: c(0x9ece8aff),
            syn_comment: c(0x7a8699ff),
            syn_number: c(0xfb9a4bff),
            syn_type: c(0x7dcfe0ff),
            syn_function: c(0xf5b84aff),
            diff_add_bg: c(0x15392fff),
            diff_remove_bg: c(0x4a242bff),
            diff_add_text: c(0xb4f0cfff),
            diff_remove_text: c(0xff938fff),
            diff_hunk_bg: c(0xffffff08),
            scrim: c(0x080f1acc),
        }
    }

    pub fn light() -> Self {
        Colors {
            bg_app: c(0xe9ecf0ff),
            bg_pane: c(0xf6f7f7ff),
            bg_surface: c(0xf8fafcff),
            bg_elevated: c(0xffffffff),
            bg_input: c(0xffffffff),
            bg_card: c(0xeef1f5ff),
            bg_changes: c(0xf3f5f8ff),
            bg_hover: c(0x0f172a0d),
            bg_active: c(0x0f172a16),
            bg_selected: c(0xe4e8eeff),
            border: c(0xe5e7ebff),
            border_strong: c(0xc8ced8ff),
            text: c(0x080f1aff),
            text_muted: c(0x465266ff),
            text_faint: c(0x6b7585ff),
            accent: c(0xa95a00ff),
            accent_fill: c(0xf5b942ff),
            accent_text: c(0x080f1aff),
            accent_bg: c(0xf59e0b26),
            text_selection: c(0xf59e0b55),
            success: c(0x1b7a4aff),
            activity: c(0x087e78ff),
            warning: c(0xb4500aff),
            danger: c(0xc23a33ff),
            danger_bg: c(0xc23a3314),
            code_bg: c(0xeef1f5ff),
            syn_keyword: c(0x8a3fb8ff),
            syn_string: c(0x2f7d32ff),
            syn_comment: c(0x6b7585ff),
            syn_number: c(0xb25a0eff),
            syn_type: c(0x1a7a8cff),
            syn_function: c(0x9a5200ff),
            diff_add_bg: c(0x62d99433),
            diff_remove_bg: c(0xff6b6330),
            diff_add_text: c(0x14663dff),
            diff_remove_text: c(0xb02a24ff),
            diff_hunk_bg: c(0x0f172a08),
            scrim: c(0x080f1a66),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub choice: ThemeChoice,
    pub colors: Colors,
    pub scale: f32,
    pub reduced_motion: bool,
}

impl Global for Theme {}

pub const SPACE: f32 = 4.0;

impl Theme {
    pub fn new(choice: ThemeChoice, size: TextSize, reduced_motion: bool) -> Self {
        Theme {
            choice,
            colors: match choice {
                ThemeChoice::Dark => Colors::dark(),
                ThemeChoice::Light => Colors::light(),
            },
            scale: size.scale(),
            reduced_motion,
        }
    }

    /// Chrome text: menus, lists, labels.
    pub fn ui_size(&self) -> Pixels {
        px(14.0 * self.scale)
    }

    pub fn small_size(&self) -> Pixels {
        px(12.0 * self.scale)
    }

    /// Conversation prose.
    pub fn body_size(&self) -> Pixels {
        px(15.5 * self.scale)
    }

    pub fn body_line_height(&self) -> Pixels {
        px((15.5 * 1.55 * self.scale).round())
    }

    pub fn code_size(&self) -> Pixels {
        px(13.0 * self.scale)
    }

    pub fn code_line_height(&self) -> Pixels {
        px((13.0 * 1.55 * self.scale).round())
    }

    pub fn control_height(&self) -> Pixels {
        px((30.0 * self.scale.max(1.0)).round())
    }

    pub fn ui_font(&self) -> SharedString {
        crate::assets::UI_FONT.into()
    }

    pub fn mono_font(&self) -> SharedString {
        crate::assets::MONO_FONT.into()
    }
}

pub trait ActiveTheme {
    fn theme(&self) -> &Theme;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Theme {
        self.global::<Theme>()
    }
}
