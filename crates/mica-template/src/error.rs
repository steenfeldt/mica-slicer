// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/PlaceholderParser.cpp :: MyContext::process_error_message
// Changes: Structured error (message, byte offset, line, column) instead of a pre-formatted exception string;
// Changes: Display renders PrusaSlicer's "Parsing error at line N: ..." text with the source line and a caret.

use std::fmt;

/// A syntax or evaluation error in a template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateError {
    /// What went wrong, e.g. `Not a variable name`.
    pub message: String,
    /// Byte offset into the template where the error was detected.
    pub offset: usize,
    /// 1-based line number of `offset`.
    pub line: usize,
    /// 1-based column (in characters) of `offset`.
    pub column: usize,
    /// The template line containing the error.
    pub source_line: String,
}

impl TemplateError {
    pub(crate) fn new(template: &str, offset: usize, message: String) -> Self {
        let mut offset = offset.min(template.len());
        while !template.is_char_boundary(offset) {
            offset -= 1;
        }
        let line_start = template[..offset].rfind('\n').map_or(0, |i| i + 1);
        let line_end = template[offset..]
            .find('\n')
            .map_or(template.len(), |i| offset + i);
        let line = template[..offset].matches('\n').count() + 1;
        let column = template[line_start..offset].chars().count() + 1;
        Self {
            message,
            offset,
            line,
            column,
            source_line: template[line_start..line_end].to_string(),
        }
    }
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Parsing error at line {}: {}", self.line, self.message)?;
        writeln!(f, "{}", self.source_line)?;
        write!(f, "{}^", " ".repeat(self.column - 1))
    }
}

impl std::error::Error for TemplateError {}
