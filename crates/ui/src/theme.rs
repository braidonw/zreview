//! The design system, as named constants.
//!
//! Transcribed from the `ZReview Design System` Claude Design project (v1, dark).
//! 42 ad-hoc hex values collapsed into 44 tokens across seven roles. The design was
//! authored against this codebase's constraints — every value here is a `u32` RGB
//! literal or an integer pixel count, with no alpha, gradients, or shadows, so
//! nothing needed translating out of CSS.
//!
//! Two rules from the design that matter more than any individual value:
//!
//! - **Step down in colour before you step down in size.** The four text levels are
//!   the primary hierarchy device. They exist to replace what used to be 46
//!   separate uses of `text_xs`, where anything less important simply got smaller
//!   until there were only two levels left.
//! - **Surfaces are an ordered ramp, not a set of greys.** If two things must
//!   differ, they differ by one step. The seven near-identical slates that used to
//!   differ by three points of lightness collapse into this ramp.
//!
//! Hue is reserved, deliberately. Accent is interaction only — focus, selection,
//! the primary button, links — and never status. Severity colours are high-chroma
//! and used a handful of times per screen. Violet belongs entirely to "a model
//! proposed this", and nothing else in the application may use it; that exclusivity
//! is what makes the provenance mark readable at eight pixels.

/// Background levels, deepest first.
pub mod surface {
    /// The diff well and its gutters — where content lives.
    pub const INSET: u32 = 0x0c_1116;
    /// Window background, file sidebar, findings panel.
    pub const BASE: u32 = 0x0f_141a;
    /// Submit bar, panel headers.
    pub const RAISED: u32 = 0x16_1c23;
    /// Composer, confirmation sheet, secondary buttons.
    pub const OVERLAY: u32 = 0x1c_232c;
    /// Pointer hover on any row. Never the only cue for anything.
    pub const HOVER: u32 = 0x20_2832;
    /// The keyboard cursor's row — blue-tinted, to pair with the accent rail.
    pub const SELECTED: u32 = 0x1a_2534;
}

/// Four levels, in descending prominence.
pub mod text {
    /// Anything the reviewer wrote or must read: diff content, comment bodies.
    pub const PRIMARY: u32 = 0xe6_ebf1;
    /// Supporting detail that is still meant to be read.
    pub const SECONDARY: u32 = 0xa4_afbc;
    /// Labels and metadata.
    pub const TERTIARY: u32 = 0x6f_7b89;
    /// Present but not competing — unchanged line numbers.
    pub const FAINT: u32 = 0x4e_5966;
    /// On an accent fill.
    pub const ON_ACCENT: u32 = 0x08_101c;
}

/// Three weights. Structure comes from surface steps first; a border is for when
/// two things at the same level must not merge.
pub mod border {
    pub const SUBTLE: u32 = 0x1a_212a;
    pub const DEFAULT: u32 = 0x26_3039;
    pub const STRONG: u32 = 0x36_424e;
    pub const FOCUS: u32 = 0x4c_8dff;
}

/// Interaction only: focus, selection, primary action, links, open-thread counts.
pub mod accent {
    pub const BASE: u32 = 0x4c_8dff;
    pub const HOVER: u32 = 0x6b_a1ff;
    /// A tinted fill behind accent-coloured text.
    pub const DIM: u32 = 0x22_355a;
    pub const TEXT: u32 = 0xa9_c8ff;
}

/// Low-chroma diff fills, meant to read as landscape at forty rows a screen.
///
/// They share no hue with severity on purpose: an added line is not a success and
/// a removed line is not an error.
pub mod diff {
    pub mod add {
        pub const BG: u32 = 0x0f_2a1c;
        /// Word-level fill, for intra-line changes.
        pub const BG_STRONG: u32 = 0x1b_4a30;
        pub const FG: u32 = 0xcd_e7d6;
        /// The `+` marker.
        pub const MARK: u32 = 0x5f_a97c;
    }
    pub mod del {
        pub const BG: u32 = 0x2c_171c;
        pub const BG_STRONG: u32 = 0x58_252f;
        pub const FG: u32 = 0xed_d2d7;
        /// The `-` marker.
        pub const MARK: u32 = 0xc4_737f;
    }
    pub mod hunk {
        pub const BG: u32 = 0x13_1a26;
        pub const FG: u32 = 0x7d_8da6;
    }
}

/// High-chroma signal. Each has a dim fill for tinted backgrounds and a text value
/// that clears 4.5:1 on [`surface::BASE`].
pub mod severity {
    pub const ERROR: u32 = 0xff_6a4d;
    pub const ERROR_DIM: u32 = 0x3a_1710;
    pub const ERROR_TEXT: u32 = 0xff_b3a0;

    pub const WARNING: u32 = 0xe2_a33c;
    pub const WARNING_DIM: u32 = 0x32_2412;
    pub const WARNING_TEXT: u32 = 0xf3_ce86;

    pub const INFO: u32 = 0x7f_a8d9;
    pub const INFO_DIM: u32 = 0x16_232f;
    pub const INFO_TEXT: u32 = 0xb6_cfe8;

    pub const SUCCESS: u32 = 0x46_c08a;
    pub const SUCCESS_DIM: u32 = 0x0f_2a20;
    pub const SUCCESS_TEXT: u32 = 0x93_ddbb;
}

/// One hue, reserved entirely for "a model proposed this".
///
/// Nothing else in the application may use violet. That exclusivity is the whole
/// mechanism: it is what lets an eight-pixel diamond mean something.
pub mod proposed {
    pub const BASE: u32 = 0xb4_8ce0;
    pub const DIM: u32 = 0x24_1a32;
    pub const TEXT: u32 = 0xd6_bdf0;
}

/// Type, split by whether alignment is load-bearing.
///
/// Monospace is confined to content that must stay column-aligned — diff rows,
/// paths, line numbers, SHAs, keybindings. Everything a human reads as prose —
/// comment bodies, findings, warnings, confirmation copy — is the system sans face,
/// which is what most of the chrome had wrong when it was all `SF Mono`.
pub mod font {
    /// Diff content, paths, numbers, keys.
    pub const MONO: &str = "SF Mono";
    /// Prose: comment bodies, findings, dialogs.
    pub const SANS: &str = ".SystemUIFont";
}

/// Pixel sizes, from the design's type scale.
pub mod size {
    /// Diff rows and inline code.
    pub const CODE: f32 = 12.5;
    /// Comment and finding bodies.
    pub const BODY: f32 = 13.0;
    /// Metadata and labels beside body text.
    pub const META: f32 = 12.0;
    /// Uppercase section labels, keycaps, counts.
    pub const LABEL: f32 = 11.0;
    /// Panel and section headings.
    pub const HEADING: f32 = 17.0;
    /// The one number on a screen that must be read first.
    pub const DISPLAY: f32 = 24.0;
}

/// One diff row. The design fixes this at 20px so forty rows fit a 900px window.
pub const ROW_HEIGHT: f32 = 20.0;

/// Each line-number gutter.
pub const GUTTER_WIDTH: f32 = 46.0;

/// The accent rail down the left of a row a comment is anchored to.
///
/// Two pixels, inside the gutter, so it never shifts the content column — a diff
/// whose text moves sideways when a comment appears is unreadable.
pub const RAIL_WIDTH: f32 = 2.0;

/// Line height for prose set at [`size::BODY`].
pub const BODY_LINE_HEIGHT: f32 = 20.0;
