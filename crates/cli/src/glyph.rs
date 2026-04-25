//! Single-source-of-truth glyph + verdict vocabulary for CLI rendering.
//!
//! P0.4 audit: pick five glyphs, document them, use them everywhere. New
//! renderers must call into this module rather than hard-coding symbols so
//! the legend stays internalizable in one pass:
//!
//! | Glyph | Meaning                                                 |
//! |-------|---------------------------------------------------------|
//! | `✓`   | matched / passed                                        |
//! | `✗`   | failed / assertion broken / step or edge absent in side |
//! | `↯`   | divergence — changed between traces                     |
//! | `⊘`   | absent — step or edge missing on one side               |
//! | `·`   | informational note (masked, skipped, lost-tie)          |

// Plain glyphs are consumed inline in `format!` strings; the colored helpers
// are reserved for the structured renderer landing in Chunks B/C. Allowing
// dead_code keeps the legend a single source of truth without forcing every
// callsite to migrate in one PR.
use colored::{ColoredString, Colorize};

#[allow(dead_code)]
pub const PASS: &str = "✓";
#[allow(dead_code)]
pub const FAIL: &str = "✗";
pub const DIVERGED: &str = "↯";
pub const ABSENT: &str = "⊘";
pub const NOTE: &str = "·";

#[allow(dead_code)]
pub fn pass() -> ColoredString {
    PASS.green().bold()
}

#[allow(dead_code)]
pub fn fail() -> ColoredString {
    FAIL.red().bold()
}

#[allow(dead_code)]
pub fn diverged() -> ColoredString {
    DIVERGED.yellow().bold()
}

#[allow(dead_code)]
pub fn absent() -> ColoredString {
    ABSENT.dimmed()
}

#[allow(dead_code)]
pub fn note() -> ColoredString {
    NOTE.dimmed()
}

/// Machine-readable summary line schema version. Bump on breaking changes
/// (renamed/removed fields). Sinks (P1.8/P1.9) must ignore unknown keys.
pub const SUMMARY_SCHEMA_VERSION: u32 = 1;

/// Prefix for the single-line JSON summary emitted by `ace run` / `ace diff`.
/// Sinks can `grep '^ACE_SUMMARY: '` from any command's stdout without
/// parsing the full stream.
pub const SUMMARY_PREFIX: &str = "ACE_SUMMARY: ";

#[cfg(test)]
mod tests {
    use super::*;

    /// Lint test: no other glyphs should leak into the diff/render renderer
    /// source. P0.4 task 9 audit — keeping the legend a single source of
    /// truth means the next renderer change can't drift the vocabulary.
    /// `⚠` and `⋯` are explicitly forbidden (replaced by `↯` and `·`).
    #[test]
    fn forbidden_glyphs_absent_from_renderers() {
        let files = [include_str!("diff.rs"), include_str!("render.rs")];
        for src in files {
            for forbidden in ['⚠', '⋯'] {
                assert!(
                    !src.contains(forbidden),
                    "renderer source contains forbidden glyph {forbidden:?}; route through glyph::*"
                );
            }
        }
    }

    #[test]
    fn legend_has_five_entries_and_unique_glyphs() {
        // Just a sanity check that the legend hasn't drifted into 8 random
        // symbols. PASS / FAIL / DIVERGED / ABSENT / NOTE — no others.
        let glyphs = [PASS, FAIL, DIVERGED, ABSENT, NOTE];
        let unique: std::collections::HashSet<_> = glyphs.iter().collect();
        assert_eq!(unique.len(), glyphs.len(), "legend has duplicates");
    }
}
