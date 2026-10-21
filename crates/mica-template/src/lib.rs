// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/PlaceholderParser.cpp :: PlaceholderParser::process
// Source: prusaslicer@b028299c77 src/libslic3r/PlaceholderParser.cpp :: PlaceholderParser::evaluate_boolean_expression
// Changes: Entry points take a mutable `Context` (variables, outputs, globals, RNG) instead of the
// Changes: PlaceholderParser object plus config_override / config_outputs / ContextData arguments, and
// Changes: return a structured `TemplateError` instead of throwing PlaceholderParserError.

//! PrusaSlicer-compatible G-code template language ("PlaceholderParser").
//!
//! Custom G-code (start/end G-code, layer change G-code, filament G-code, …) in printer profiles is
//! written in this language. [`process`] expands a template against a [`Context`] of variables:
//!
//! ```
//! use mica_template::{process, Context, Var};
//!
//! let mut ctx = Context::new();
//! ctx.set("first_layer_temperature", Var::ints([215]));
//! ctx.set("layer_z", 0.2);
//! let out = process("M104 S[first_layer_temperature] ; Z={layer_z * 2}", &mut ctx).unwrap();
//! assert_eq!(out, "M104 S215 ; Z=0.4");
//! ```
//!
//! # Language
//!
//! * Text is copied verbatim. `[name]` expands a config variable the legacy way (vectors are indexed
//!   by the current extruder, `[name_2]` and `[name[idx_var]]` pick an element; floats use the same
//!   formatting as below, bools print as `1`/`0`). Every `[` in the text must start such an expansion.
//! * `{ ... }` contains statements separated by `;`. The value of an expression statement is output.
//!   `{if cond}text{elsif cond}text{else}text{endif}` selects text; inside code the same is written
//!   `if cond then ...; elsif cond then ...; else ...; endif`.
//! * Values are bool, int, float and string, or vectors of them; elements of nullable vectors may be
//!   nil. `int op int` stays int (`/` and `%` truncate), anything with a float is float, `+` with a
//!   string concatenates. `==` on floats has a `1e-8` tolerance. Floats print like C++
//!   `std::ostringstream << double`, i.e. `%g` with 6 significant digits ([`format_double`]).
//! * Operators, loosest first: `?:`, `or`/`||`, `and`/`&&`, `==` `!=` `=~ /re/` `!~ /re/`,
//!   `<` `>` `<=` `>=`, `+` `-`, `*` `/` `%`, unary `-` `+` `not` `!`. Regular expressions must match
//!   the whole string (C++ `regex_match`); `.` also matches newlines.
//! * Functions: `min`, `max`, `random(lo, hi)`, `digits(v, width[, decimals])`, `zdigits(…)`, `int`,
//!   `round`, `is_nil(var[i])`, `one_of(s, "a", ~"regex", /regex/, …)`, `empty(vec)`, `size(vec)`,
//!   `interpolate_table(x, (x0, y0), (x1, y1), …)`, `repeat(n, value)` (in assignments).
//! * Variables: `local name = …` (this call only), `global name = …` (persists in the `Context`),
//!   assignment to existing writable variables (`name = v`, `vec[i] = v`, `vec = (a, b, c)`,
//!   `vec = other_vec`, `vec = repeat(n, v)`). Assigned values are converted to the variable's type.
//!
//! # PrusaSlicer quirks that are kept on purpose
//!
//! Templates that work in PrusaSlicer produce the same output here, and templates that fail there
//! fail here too:
//!
//! * A statement that starts with a variable followed by `=` is an assignment, so `{a == b ? 1 : 2}`
//!   is a syntax error; profiles write `{(a == b ? 1 : 2)}`. Likewise `x = (1 + 2) * 3` is an error
//!   because `(1 + 2)` is taken as a one-element initializer list.
//! * `<>` is listed as "not equal" but always fails to parse (`<` is taken as "less than" first).
//! * Inactive branches are parsed but not evaluated, so unknown variables there are fine — except that
//!   `local x = y + 1` in an inactive branch is a syntax error (the parser takes `y` as a vector copy).
//! * `elsif` conditions are evaluated even after an earlier branch was taken.
//! * Reading `vec[i]` with `i` past the end reads element 0; writing past the end is an error.
//! * Legacy `[name]` expansion only sees config variables, not outputs, globals or locals.
//! * `interpolate_table` arguments are single operands (`(x+1, y)` is an error); a one-point table
//!   only accepts x within `EPSILON` (1e-4) below or above the point, not the point itself.
//! * Copying an empty non-string vector (`local v = empty_vec`) fails as "nil".

mod context;
mod error;
mod format;
mod parser;

pub use context::{Context, RandomSource, SplitMix64, Value, ValueType, Var};
pub use error::TemplateError;
pub use format::format_double;

/// Expands a template.
///
/// Assignments to output variables ([`Context::set_output`]) and `global` variables stay in `ctx`;
/// `local` variables are dropped when the call returns. On error nothing is output, but assignments
/// made before the error remain in `ctx` (as in PrusaSlicer).
pub fn process(template: &str, ctx: &mut Context) -> Result<String, TemplateError> {
    parser::Parser::new(template, ctx)
        .parse_template()
        .map_err(|e| TemplateError::new(template, e.pos, e.msg))
}

/// Evaluates a boolean expression such as a `compatible_printers_condition`
/// (`printer_notes=~/.*PRINTER_VENDOR_PRUSA3D.*/ and nozzle_diameter[0]==0.6`).
///
/// An empty string (or anything that is not an expression) evaluates to `false`, like PrusaSlicer.
pub fn evaluate_boolean(expression: &str, ctx: &mut Context) -> Result<bool, TemplateError> {
    parser::Parser::new(expression, ctx)
        .parse_boolean()
        .map_err(|e| TemplateError::new(expression, e.pos, e.msg))
}
