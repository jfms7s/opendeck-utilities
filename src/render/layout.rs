//! The Window Layout key tile.

/// What a Window Layout key shows.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutView {
    /// The big line: the chosen layout's label ("1/3"), "Cycle", or
    /// "Linux only".
    pub title: String,
    /// Column widths to draw, left to right, as fractions of the screen
    /// that sum to 1. Empty: no diagram (cycle mode, unsupported).
    pub columns: Vec<f64>,
    /// The line under it: what a press does.
    pub hint: String,
}
