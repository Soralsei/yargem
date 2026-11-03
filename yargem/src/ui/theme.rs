use egui::{Color32, Visuals};
use yargem_lib::debug::protocol::{AccessKind, StopReason};

/// Debugger colours by role.
///
/// Built from the active `Visuals` on every use, so it follows the dark/light
/// switch. Roles egui already has (selection, code background, weak text...)
/// are read from `Visuals`; only the debugger-specific ones are defined here.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    // Run state and stop reasons: status dots, breakpoint gutter
    pub running: Color32,
    pub paused: Color32,
    pub breakpoint: Color32,
    pub watchpoint: Color32,

    // Watchpoint access badges
    pub access_read: Color32,
    pub access_write: Color32,

    /// Disassembly and hex view background
    pub code_bg: Color32,
    /// Every other list row
    pub stripe_bg: Color32,
    /// Current PC row, set flags, last written byte, "stopped here" tag
    pub highlight_bg: Color32,
    pub highlight_text: Color32,
    /// Stop reason card and the list row that caused the stop
    pub stop_bg: Color32,
    /// Cleared flags, tag borders
    pub outline: Color32,

    pub text_strong: Color32,
    pub text_weak: Color32,
    pub error: Color32,
}

impl Theme {
    pub fn of(ui: &egui::Ui) -> Self {
        Self::from_visuals(ui.visuals())
    }

    pub fn from_visuals(v: &Visuals) -> Self {
        let (running, paused, breakpoint, watchpoint) = if v.dark_mode {
            (
                Color32::from_rgb(0x3F, 0xB9, 0x50),
                Color32::from_rgb(0x5A, 0xAA, 0xFF),
                Color32::from_rgb(0xE5, 0x48, 0x4D),
                Color32::from_rgb(0xFF, 0x8F, 0x00),
            )
        } else {
            (
                Color32::from_rgb(0x1A, 0x7F, 0x37),
                Color32::from_rgb(0x09, 0x69, 0xDA),
                Color32::from_rgb(0xCD, 0x2B, 0x31),
                Color32::from_rgb(0xBC, 0x4C, 0x00),
            )
        };

        Self {
            running,
            paused,
            breakpoint,
            watchpoint,
            access_read: paused,
            access_write: watchpoint,
            code_bg: v.extreme_bg_color,
            stripe_bg: v.faint_bg_color,
            highlight_bg: v.selection.bg_fill,
            highlight_text: v.selection.stroke.color,
            // Translucent selection: a tint of the panel behind it in either mode
            stop_bg: v.selection.bg_fill.gamma_multiply(0.3),
            outline: v.widgets.noninteractive.bg_stroke.color,
            text_strong: v.strong_text_color(),
            text_weak: v.weak_text_color(),
            error: v.error_fg_color,
        }
    }

    pub fn stop(&self, reason: &StopReason) -> Color32 {
        match reason {
            StopReason::Breakpoint(_) => self.breakpoint,
            StopReason::Watchpoint(_) => self.watchpoint,
            StopReason::Step | StopReason::UserPause => self.paused,
        }
    }

    pub fn access(&self, kind: AccessKind) -> Color32 {
        match kind {
            AccessKind::Read => self.access_read,
            AccessKind::Write => self.access_write,
        }
    }
}
