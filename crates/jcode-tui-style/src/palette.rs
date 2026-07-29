//! Named colour palettes for the transcript.
//!
//! Every transcript colour used to be a literal inside [`crate::theme`], which
//! made the scheme impossible to change without patching the binary. The
//! accessors there now read their RGB triples from the palette selected here,
//! so swapping schemes is a single [`set_palette`] call and no call site
//! changes.
//!
//! Palettes are static presets rather than user-supplied colour tables. The
//! accessors run per rendered cell, so selection is a relaxed atomic load and
//! the presets are `const`.
//!
//! # Contrast
//!
//! The [`Preset::ClaudeHc`] variant exists because contrast is an accessibility
//! constraint, not a matter of taste. Measured against a `#191c21` terminal
//! background, the default palette's dimmed text sits at 2.12:1 — below the
//! WCAG AA floor of 4.5:1 for body text, and well below the 7:1 AAA level that
//! readers with reduced contrast sensitivity need. `ClaudeHc` keeps the hue and
//! saturation of each [`Preset::Claude`] colour and raises only its lightness
//! until it clears 7:1, so the scheme still reads as the same scheme.

use std::sync::atomic::{AtomicU8, Ordering};

/// An RGB triple. Kept as raw components so presets can be `const`; the
/// accessors in [`crate::theme`] convert through [`crate::color::rgb`], which
/// applies terminal colour-capability quantisation.
pub type Rgb = (u8, u8, u8);

/// The transcript colour slots. One field per accessor in [`crate::theme`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub user: Rgb,
    pub ai: Rgb,
    pub tool: Rgb,
    pub file_link: Rgb,
    pub dim: Rgb,
    pub accent: Rgb,
    pub system_message: Rgb,
    pub queued: Rgb,
    pub asap: Rgb,
    pub pending: Rgb,
    pub user_text: Rgb,
    pub user_bg: Rgb,
    pub ai_text: Rgb,
    pub header_icon: Rgb,
    pub header_name: Rgb,
    pub header_session: Rgb,
}

/// The palette jcode has always shipped.
pub const DEFAULT: Palette = Palette {
    user: (138, 180, 248),
    ai: (129, 199, 132),
    tool: (120, 120, 120),
    file_link: (180, 200, 255),
    dim: (80, 80, 80),
    accent: (186, 139, 255),
    system_message: (255, 170, 220),
    queued: (255, 193, 7),
    asap: (110, 210, 255),
    pending: (140, 140, 140),
    user_text: (245, 245, 255),
    user_bg: (35, 40, 50),
    ai_text: (220, 220, 215),
    header_icon: (120, 210, 230),
    header_name: (190, 210, 235),
    header_session: (255, 255, 255),
};

/// Claude Code's scheme, sampled from its rendered output.
pub const CLAUDE: Palette = Palette {
    user: (177, 185, 249),
    ai: (78, 186, 101),
    tool: (153, 153, 153),
    file_link: (177, 185, 249),
    dim: (136, 136, 136),
    accent: (215, 119, 87),
    system_message: (190, 132, 255),
    queued: (235, 156, 47),
    asap: (0, 204, 204),
    pending: (136, 136, 136),
    user_text: (248, 248, 242),
    user_bg: (35, 40, 50),
    ai_text: (248, 248, 242),
    header_icon: (0, 204, 204),
    header_name: (177, 185, 249),
    header_session: (255, 255, 255),
};

/// [`CLAUDE`] with every slot raised to at least 7:1 against a `#191c21`
/// background. Hue and saturation are untouched; only lightness moves, and only
/// for the slots that fell short.
pub const CLAUDE_HC: Palette = Palette {
    ai: (79, 187, 102),
    tool: (166, 166, 166),
    dim: (166, 166, 166),
    accent: (223, 148, 122),
    system_message: (195, 142, 255),
    pending: (166, 166, 166),
    ..CLAUDE
};

/// Selectable palettes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Preset {
    /// jcode's original colours.
    #[default]
    Default,
    /// Claude Code's colours as measured.
    Claude,
    /// Claude Code's colours raised to a 7:1 contrast floor.
    ClaudeHc,
}

impl Preset {
    pub fn palette(self) -> &'static Palette {
        match self {
            Self::Default => &DEFAULT,
            Self::Claude => &CLAUDE,
            Self::ClaudeHc => &CLAUDE_HC,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Claude => "claude",
            Self::ClaudeHc => "claude-hc",
        }
    }

    /// Every selectable preset, for pickers and help text.
    pub const ALL: [Self; 3] = [Self::Default, Self::Claude, Self::ClaudeHc];

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "" | "default" | "jcode" => Some(Self::Default),
            "claude" | "claude-code" | "claudecode" => Some(Self::Claude),
            "claude-hc"
            | "claude_hc"
            | "claudehc"
            | "claude-high-contrast"
            | "high-contrast"
            | "hc" => Some(Self::ClaudeHc),
            _ => None,
        }
    }

    fn from_id(id: u8) -> Self {
        match id {
            1 => Self::Claude,
            2 => Self::ClaudeHc,
            _ => Self::Default,
        }
    }

    fn id(self) -> u8 {
        match self {
            Self::Default => 0,
            Self::Claude => 1,
            Self::ClaudeHc => 2,
        }
    }
}

static PALETTE: AtomicU8 = AtomicU8::new(0);

/// Select the global palette. Called at startup once config is loaded, and
/// again when the user switches at runtime.
pub fn set_palette(preset: Preset) {
    PALETTE.store(preset.id(), Ordering::Relaxed);
}

/// The active preset.
pub fn palette_preset() -> Preset {
    Preset::from_id(PALETTE.load(Ordering::Relaxed))
}

/// The active palette. Hot: called once per coloured span.
pub fn palette() -> &'static Palette {
    palette_preset().palette()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG 2.x relative luminance.
    fn luminance((r, g, b): Rgb) -> f64 {
        fn channel(c: u8) -> f64 {
            let c = c as f64 / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
    }

    fn contrast(fg: Rgb, bg: Rgb) -> f64 {
        let (a, b) = (luminance(fg), luminance(bg));
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// The background the high-contrast preset is tuned against.
    const REFERENCE_BG: Rgb = (25, 28, 33);

    fn slots(p: &Palette) -> [(&'static str, Rgb); 15] {
        [
            ("user", p.user),
            ("ai", p.ai),
            ("tool", p.tool),
            ("file_link", p.file_link),
            ("dim", p.dim),
            ("accent", p.accent),
            ("system_message", p.system_message),
            ("queued", p.queued),
            ("asap", p.asap),
            ("pending", p.pending),
            ("user_text", p.user_text),
            ("ai_text", p.ai_text),
            ("header_icon", p.header_icon),
            ("header_name", p.header_name),
            ("header_session", p.header_session),
        ]
    }

    #[test]
    fn high_contrast_preset_clears_wcag_aaa() {
        for (name, color) in slots(&CLAUDE_HC) {
            let ratio = contrast(color, REFERENCE_BG);
            assert!(
                ratio >= 7.0,
                "claude-hc slot `{name}` is {ratio:.2}:1, below the 7:1 floor"
            );
        }
    }

    /// The corrections must not drift off-hue, otherwise the palette stops
    /// reading as the scheme it is derived from.
    #[test]
    fn high_contrast_preset_only_lifts_lightness() {
        for ((name, hc), (_, base)) in slots(&CLAUDE_HC).into_iter().zip(slots(&CLAUDE)) {
            if hc == base {
                continue;
            }
            assert!(
                luminance(hc) > luminance(base),
                "claude-hc slot `{name}` should be lighter than its claude source"
            );
            // Channel ordering encodes the hue well enough to catch a swap.
            let order = |(r, g, b): Rgb| (r >= g, g >= b, r >= b);
            assert_eq!(
                order(hc),
                order(base),
                "claude-hc slot `{name}` changed hue relative to claude"
            );
        }
    }

    #[test]
    fn preset_labels_round_trip_through_parse() {
        for preset in Preset::ALL {
            assert_eq!(Preset::parse(preset.label()), Some(preset));
        }
        assert_eq!(Preset::parse("nope"), None);
    }

    #[test]
    fn preset_ids_round_trip() {
        for preset in Preset::ALL {
            assert_eq!(Preset::from_id(preset.id()), preset);
        }
    }
}
