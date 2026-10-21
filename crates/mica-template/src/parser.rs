// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/PlaceholderParser.cpp :: client::macro_processor
// Source: prusaslicer@b028299c77 src/libslic3r/PlaceholderParser.cpp :: client::expr
// Source: prusaslicer@b028299c77 src/libslic3r/PlaceholderParser.cpp :: client::MyContext
// Source: prusaslicer@b028299c77 src/libslic3r/PlaceholderParser.cpp :: client::InterpolateTableContext
// Changes: The Boost.Spirit grammar is hand-translated into a recursive descent parser that, like Spirit,
// Changes: evaluates while it parses. Spirit's ">" expectation points become hard errors and ">>"/"|"
// Changes: become backtracking, so PrusaSlicer's parse quirks are kept (see the crate docs). Inactive
// Changes: if/else/ternary branches are parsed with a suppression depth and evaluate to "empty" values,
// Changes: as with TYPE_EMPTY. Ints are i64 (C++: int) with wrapping arithmetic; Boost.Regex is replaced
// Changes: by the regex crate; random() uses the Context's RandomSource; FloatOrPercent, Percent, Point
// Changes: and Enum option types are not modelled (callers pass them as floats or strings).

//! The template interpreter.

use std::collections::HashMap;

use crate::context::{Context, Scope, Value, ValueType, Var};
use crate::format::{escape_string_cstyle, format_double, printf_fixed, printf_int};

/// Internal error: byte position + message.
#[derive(Debug)]
pub(crate) struct PErr {
    pub pos: usize,
    pub msg: String,
}

type R<T> = Result<T, PErr>;

fn err<T>(pos: usize, msg: impl Into<String>) -> R<T> {
    Err(PErr {
        pos,
        msg: msg.into(),
    })
}

/// Reserved words; they cannot be used as identifiers. Note that `then` is not one of them.
const KEYWORDS: &[&str] = &[
    "and",
    "digits",
    "zdigits",
    "empty",
    "if",
    "int",
    "is_nil",
    "local",
    "else",
    "elsif",
    "endif",
    "false",
    "global",
    "interpolate_table",
    "min",
    "max",
    "random",
    "repeat",
    "round",
    "not",
    "one_of",
    "or",
    "size",
    "true",
];

/// PrusaSlicer's `EPSILON` (libslic3r.h), used by `interpolate_table`.
const EPSILON: f64 = 1e-4;

// ------------------------------------------------------------------------------------------------
// Expression values
// ------------------------------------------------------------------------------------------------

/// Value of an expression. `Empty` is produced inside a branch that is being skipped.
#[derive(Clone, Debug, PartialEq)]
enum Ev {
    Empty,
    Bool(bool),
    Int(i64),
    Double(f64),
    Str(String),
}

/// An evaluated expression with the byte range it was parsed from (for error positions).
#[derive(Clone, Debug)]
struct Expr {
    v: Ev,
    start: usize,
    end: usize,
}

impl Expr {
    fn new(v: Ev, start: usize, end: usize) -> Self {
        Self { v, start, end }
    }

    fn is_empty(&self) -> bool {
        matches!(self.v, Ev::Empty)
    }

    fn numeric(&self) -> bool {
        matches!(self.v, Ev::Int(_) | Ev::Double(_))
    }

    fn is_double(&self) -> bool {
        matches!(self.v, Ev::Double(_))
    }

    fn as_d(&self) -> f64 {
        match self.v {
            Ev::Int(i) => i as f64,
            Ev::Double(d) => d,
            _ => 0.0,
        }
    }

    /// C++ `int(d)`: truncation.
    fn as_i(&self) -> i64 {
        match self.v {
            Ev::Int(i) => i,
            Ev::Double(d) => d as i64,
            _ => 0,
        }
    }

    /// C++ `int(std::round(d))`: half away from zero.
    fn as_i_rounded(&self) -> i64 {
        match self.v {
            Ev::Int(i) => i,
            Ev::Double(d) => d.round() as i64,
            _ => 0,
        }
    }

    /// `expr::to_string`.
    fn text(&self) -> String {
        match &self.v {
            Ev::Empty => String::new(),
            Ev::Bool(b) => if *b { "true" } else { "false" }.to_string(),
            Ev::Int(i) => i.to_string(),
            Ev::Double(d) => format_double(*d),
            Ev::Str(s) => s.clone(),
        }
    }

    fn fail<T>(&self, msg: impl Into<String>) -> R<T> {
        err(self.start, msg)
    }

    fn check_numeric(&self, msg: &str) -> R<()> {
        if self.numeric() {
            Ok(())
        } else {
            self.fail(msg)
        }
    }
}

fn add(lhs: &mut Expr, rhs: &Expr) -> R<()> {
    match &mut lhs.v {
        Ev::Empty => {}
        Ev::Str(s) => s.push_str(&rhs.text()),
        _ => {
            if let Ev::Str(r) = &rhs.v {
                lhs.v = Ev::Str(lhs.text() + r);
            } else {
                let msg = "Cannot add non-numeric types.";
                lhs.check_numeric(msg)?;
                rhs.check_numeric(msg)?;
                lhs.v = match (&lhs.v, &rhs.v) {
                    (Ev::Int(a), Ev::Int(b)) => Ev::Int(a.wrapping_add(*b)),
                    _ => Ev::Double(lhs.as_d() + rhs.as_d()),
                };
            }
        }
    }
    lhs.end = rhs.end;
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum ArithOp {
    Sub,
    Mul,
    Div,
    Mod,
}

fn arith(lhs: &mut Expr, rhs: &Expr, op: ArithOp) -> R<()> {
    if lhs.is_empty() {
        return Ok(());
    }
    match op {
        ArithOp::Sub => {
            let msg = "Cannot subtract non-numeric types.";
            lhs.check_numeric(msg)?;
            rhs.check_numeric(msg)?;
        }
        ArithOp::Mul => {
            let msg = "Cannot multiply with non-numeric type.";
            lhs.check_numeric(msg)?;
            rhs.check_numeric(msg)?;
        }
        ArithOp::Div | ArithOp::Mod => {
            lhs.check_numeric("Cannot divide a non-numeric type.")?;
            rhs.check_numeric("Cannot divide with a non-numeric type.")?;
            let zero = match rhs.v {
                Ev::Int(i) => i == 0,
                _ => rhs.as_d() == 0.0,
            };
            if zero {
                return rhs.fail("Division by zero");
            }
        }
    }
    lhs.v = match (&lhs.v, &rhs.v) {
        (Ev::Int(a), Ev::Int(b)) => Ev::Int(match op {
            ArithOp::Sub => a.wrapping_sub(*b),
            ArithOp::Mul => a.wrapping_mul(*b),
            ArithOp::Div => a.wrapping_div(*b),
            ArithOp::Mod => a.wrapping_rem(*b),
        }),
        _ => {
            let (a, b) = (lhs.as_d(), rhs.as_d());
            Ev::Double(match op {
                ArithOp::Sub => a - b,
                ArithOp::Mul => a * b,
                ArithOp::Div => a / b,
                ArithOp::Mod => a % b, // std::fmod
            })
        }
    };
    lhs.end = rhs.end;
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum CmpOp {
    Eq,
    Lt,
    Gt,
}

/// `expr::compare_op`: the result replaces `lhs`.
fn compare(lhs: &mut Expr, rhs: &Expr, op: CmpOp, invert: bool) -> R<()> {
    if lhs.is_empty() {
        return Ok(());
    }
    let value = if lhs.numeric() && rhs.numeric() {
        let any_double = lhs.is_double() || rhs.is_double();
        let (a, b) = (lhs.as_d(), rhs.as_d());
        match (op, any_double) {
            (CmpOp::Eq, true) => (a - b).abs() < 1e-8,
            (CmpOp::Eq, false) => lhs.as_i() == rhs.as_i(),
            (CmpOp::Lt, true) => a < b,
            (CmpOp::Lt, false) => lhs.as_i() < rhs.as_i(),
            (CmpOp::Gt, true) => a > b,
            (CmpOp::Gt, false) => lhs.as_i() > rhs.as_i(),
        }
    } else if let (Ev::Bool(a), Ev::Bool(b)) = (&lhs.v, &rhs.v) {
        if op != CmpOp::Eq {
            return lhs.fail("Cannot compare the types.");
        }
        a == b
    } else if matches!(lhs.v, Ev::Str(_)) || matches!(rhs.v, Ev::Str(_)) {
        let (a, b) = (lhs.text(), rhs.text());
        match op {
            CmpOp::Eq => a == b,
            CmpOp::Lt => a < b,
            CmpOp::Gt => a > b,
        }
    } else {
        return lhs.fail("Cannot compare the types.");
    };
    lhs.v = Ev::Bool(value != invert);
    lhs.end = rhs.end;
    Ok(())
}

fn logical(lhs: &mut Expr, rhs: &Expr, or: bool) -> R<()> {
    if lhs.is_empty() {
        return Ok(());
    }
    match (&lhs.v, &rhs.v) {
        (Ev::Bool(a), Ev::Bool(b)) => {
            lhs.v = Ev::Bool(if or { *a || *b } else { *a && *b });
            lhs.end = rhs.end;
            Ok(())
        }
        _ => lhs.fail("Cannot apply logical operation to non-boolean operators."),
    }
}

/// `min()` / `max()`; the result replaces `a`.
fn min_max(a: &mut Expr, b: &Expr, min: bool) -> R<()> {
    if a.is_empty() {
        return Ok(());
    }
    a.check_numeric("Not a numeric type.")?;
    b.check_numeric("Not a numeric type.")?;
    a.v = if a.is_double() || b.is_double() {
        let (x, y) = (a.as_d(), b.as_d());
        // std::min / std::max argument order semantics.
        Ev::Double(if min {
            if y < x { y } else { x }
        } else if x < y {
            y
        } else {
            x
        })
    } else {
        let (x, y) = (a.as_i(), b.as_i());
        Ev::Int(if min { x.min(y) } else { x.max(y) })
    };
    Ok(())
}

/// `digits()` / `zdigits()`; the result replaces `a`.
fn digits(a: &mut Expr, width: &Expr, decimals: &Expr, zero_pad: bool) -> R<()> {
    if a.is_empty() {
        return Ok(());
    }
    a.check_numeric("Not a numeric type.")?;
    if !matches!(width.v, Ev::Int(_)) {
        return width.fail("digits: second parameter must be integer");
    }
    let has_decimals = !decimals.is_empty();
    if has_decimals && !matches!(decimals.v, Ev::Int(_)) {
        return decimals.fail("digits: third parameter must be integer");
    }
    let ndigits = width.as_i().clamp(0, 64) as usize;
    let s = if has_decimals {
        let ndecimals = decimals.as_i().clamp(0, 64) as usize;
        printf_fixed(a.as_d(), ndigits, ndecimals, zero_pad)
    } else {
        printf_int(a.as_i_rounded(), ndigits, zero_pad)
    };
    a.v = Ev::Str(s);
    Ok(())
}

/// Assignment conversion of `rhs` to an option of type `ty`.
fn convert(ty: ValueType, rhs: &Expr) -> R<Value> {
    Ok(match ty {
        ValueType::Float => {
            rhs.check_numeric("Right side is not a numeric expression")?;
            Value::Float(rhs.as_d())
        }
        ValueType::Int => {
            rhs.check_numeric("Right side is not a numeric expression")?;
            Value::Int(rhs.as_i())
        }
        ValueType::Str => Value::Str(rhs.text()),
        ValueType::Bool => match rhs.v {
            Ev::Bool(b) => Value::Bool(b),
            _ => return rhs.fail("Right side is not a boolean expression"),
        },
    })
}

fn ev_type(v: &Ev) -> Option<ValueType> {
    match v {
        Ev::Empty => None,
        Ev::Bool(_) => Some(ValueType::Bool),
        Ev::Int(_) => Some(ValueType::Int),
        Ev::Double(_) => Some(ValueType::Float),
        Ev::Str(_) => Some(ValueType::Str),
    }
}

/// Element read from a vector of type `ty` (ints stored in a float vector read as floats).
fn element_to_ev(ty: ValueType, v: &Value) -> Ev {
    match (ty, v) {
        (ValueType::Float, Value::Int(i)) => Ev::Double(*i as f64),
        (_, v) => value_to_ev(v),
    }
}

fn value_to_ev(v: &Value) -> Ev {
    match v {
        Value::Nil => Ev::Empty,
        Value::Bool(b) => Ev::Bool(*b),
        Value::Int(i) => Ev::Int(*i),
        Value::Float(f) => Ev::Double(*f),
        Value::Str(s) => Ev::Str(s.clone()),
    }
}

/// `ConfigOption::serialize()` of a scalar (legacy `[name]` expansion).
fn serialize_scalar(v: &Value) -> String {
    match v {
        Value::Nil => "nil".to_string(),
        Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => format_double(*f),
        Value::Str(s) => escape_string_cstyle(s),
    }
}

/// `ConfigOptionVector::vserialize()[i]` (legacy `[name]` / `[name_N]` expansion).
fn serialize_element(v: &Value) -> String {
    match v {
        Value::Nil => "nil".to_string(),
        Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => format_double(*f),
        Value::Str(s) => s.clone(),
    }
}

/// `ConfigOption::is_nil()`: a nil scalar, or a vector whose elements are all nil (true for an empty
/// vector). String vectors are never nil.
fn all_nil(var: &Var) -> bool {
    match var {
        Var::Scalar(v) => v.is_nil(),
        Var::Vector(ValueType::Str, _) => false,
        Var::Vector(_, values) => values.iter().all(Value::is_nil),
    }
}

// ------------------------------------------------------------------------------------------------
// Variable references
// ------------------------------------------------------------------------------------------------

/// A resolved variable reference (`OptWithPos`). `scope` is `None` while skipping.
#[derive(Clone, Debug)]
struct VarRef {
    name: String,
    scope: Option<Scope>,
    index: Option<i64>,
    start: usize,
    end: usize,
}

/// Target of `local name = …` / `global name = …` (`NewOldVariable`).
#[derive(Clone, Debug)]
struct NewVar {
    name: String,
    global: bool,
    existing: Option<Scope>,
    start: usize,
    end: usize,
}

impl NewVar {
    fn as_ref(&self) -> VarRef {
        VarRef {
            name: self.name.clone(),
            scope: self.existing,
            index: None,
            start: self.start,
            end: self.end,
        }
    }
}

/// What may follow a statement without a separating `;`.
#[derive(Clone, Copy)]
enum Term {
    /// `{ ... }`: the closing brace.
    Brace,
    /// `then ...`: `elsif`, `else` or `endif`.
    IfBranch,
    /// `else ...`: `endif`.
    ElseBranch,
}

// ------------------------------------------------------------------------------------------------
// Parser / evaluator
// ------------------------------------------------------------------------------------------------

pub(crate) struct Parser<'s, 'c> {
    src: &'s str,
    b: &'s [u8],
    pos: usize,
    ctx: &'c mut Context,
    locals: HashMap<String, Var>,
    /// Depth of nested inactive branches (`m_depth_suppressed`).
    suppressed: u32,
}

impl<'s, 'c> Parser<'s, 'c> {
    pub(crate) fn new(src: &'s str, ctx: &'c mut Context) -> Self {
        Self {
            src,
            b: src.as_bytes(),
            pos: 0,
            ctx,
            locals: HashMap::new(),
            suppressed: 0,
        }
    }

    /// Full macro mode: `text_block > eoi`.
    pub(crate) fn parse_template(&mut self) -> R<String> {
        let out = self.text_block()?;
        self.expect_eoi()?;
        Ok(out)
    }

    /// Boolean mode: `conditional_expression > eoi`. A template that is not an expression at all
    /// yields `false` without an error, like PrusaSlicer.
    pub(crate) fn parse_boolean(&mut self) -> R<bool> {
        let Some(e) = self.conditional_expression()? else {
            return Ok(false);
        };
        let value = match e.v {
            Ev::Bool(b) => b,
            _ => return e.fail("Not a boolean expression"),
        };
        self.expect_eoi()?;
        Ok(value)
    }

    // ---- branch suppression ----

    fn skipping(&self) -> bool {
        self.suppressed > 0
    }

    fn block_enter(&mut self, condition: bool) {
        if self.skipping() || !condition {
            self.suppressed += 1;
        }
    }

    /// Leaves a branch; returns true if its output is the one to keep.
    fn block_exit(&mut self, condition: bool) -> bool {
        if self.skipping() {
            self.suppressed -= 1;
            false
        } else {
            condition
        }
    }

    // ---- lexical helpers ----

    /// The skipper: spaces, tabs and newlines; any non-ASCII byte in code is an error.
    fn skip_ws(&mut self) -> R<()> {
        while let Some(&c) = self.b.get(self.pos) {
            if c >= 0x80 {
                return err(
                    self.pos,
                    "Non-ASCII7 characters are only allowed inside text blocks and string literals, not inside code blocks.",
                );
            }
            if matches!(c, b' ' | b'\t' | b'\r' | b'\n') {
                self.pos += 1;
            } else {
                break;
            }
        }
        Ok(())
    }

    fn peek_pos(&mut self) -> R<usize> {
        let save = self.pos;
        self.skip_ws()?;
        let p = self.pos;
        self.pos = save;
        Ok(p)
    }

    fn expected<T>(&mut self, what: &str) -> R<T> {
        let p = self.peek_pos()?;
        err(p, what)
    }

    fn lit(&mut self, c: u8) -> R<bool> {
        let save = self.pos;
        self.skip_ws()?;
        if self.b.get(self.pos) == Some(&c) {
            self.pos += 1;
            Ok(true)
        } else {
            self.pos = save;
            Ok(false)
        }
    }

    fn lit_str(&mut self, s: &str) -> R<bool> {
        let save = self.pos;
        self.skip_ws()?;
        if self.b[self.pos..].starts_with(s.as_bytes()) {
            self.pos += s.len();
            Ok(true)
        } else {
            self.pos = save;
            Ok(false)
        }
    }

    /// Lookahead for a character (`&lit(c)`).
    fn peek_lit(&mut self, c: u8) -> R<bool> {
        let save = self.pos;
        let r = self.lit(c)?;
        self.pos = save;
        Ok(r)
    }

    fn expect_lit(&mut self, c: u8) -> R<()> {
        if self.lit(c)? {
            Ok(())
        } else {
            self.expected(&format!("Expecting '{}'.", c as char))
        }
    }

    /// A keyword not followed by an identifier character (`distinct`).
    fn kw(&mut self, word: &str) -> R<bool> {
        let save = self.pos;
        self.skip_ws()?;
        let rest = &self.b[self.pos..];
        if rest.starts_with(word.as_bytes())
            && !rest
                .get(word.len())
                .is_some_and(|&c| c.is_ascii_alphanumeric() || c == b'_')
        {
            self.pos += word.len();
            Ok(true)
        } else {
            self.pos = save;
            Ok(false)
        }
    }

    fn expect_eoi(&mut self) -> R<()> {
        self.skip_ws()?;
        if self.pos < self.b.len() {
            return err(self.pos, "Unknown syntax error");
        }
        Ok(())
    }

    /// `identifier`: `(alpha | '_') (alnum | '_')*`, not a keyword. Returns its byte range.
    fn identifier(&mut self) -> R<Option<(usize, usize)>> {
        let save = self.pos;
        self.skip_ws()?;
        let start = self.pos;
        match self.b.get(self.pos) {
            Some(c) if c.is_ascii_alphabetic() || *c == b'_' => self.pos += 1,
            _ => {
                self.pos = save;
                return Ok(None);
            }
        }
        while self
            .b
            .get(self.pos)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        {
            self.pos += 1;
        }
        if KEYWORDS.contains(&&self.src[start..self.pos]) {
            self.pos = save;
            return Ok(None);
        }
        Ok(Some((start, self.pos)))
    }

    /// Byte length of the UTF-8 sequence starting at `pos`.
    fn char_len(&self, pos: usize) -> usize {
        self.src[pos..].chars().next().map_or(1, char::len_utf8)
    }

    /// A delimited literal (`"..."` or `/.../`) with backslash escapes. Returns the full range.
    fn delimited(&mut self, delim: u8) -> R<Option<(usize, usize)>> {
        let save = self.pos;
        self.skip_ws()?;
        if self.b.get(self.pos) != Some(&delim) {
            self.pos = save;
            return Ok(None);
        }
        let start = self.pos;
        self.pos += 1;
        loop {
            match self.b.get(self.pos) {
                None => return err(self.pos, format!("Expecting '{}'.", delim as char)),
                Some(&c) if c == delim => {
                    self.pos += 1;
                    return Ok(Some((start, self.pos)));
                }
                Some(b'\\') => {
                    self.pos += 1;
                    if self.pos >= self.b.len() {
                        return err(self.pos, "Expecting a character.");
                    }
                    self.pos += self.char_len(self.pos);
                }
                Some(_) => self.pos += self.char_len(self.pos),
            }
        }
    }

    // ---- variables ----

    fn lookup(&self, name: &str) -> Option<Scope> {
        self.ctx
            .resolve(name)
            .or_else(|| self.locals.contains_key(name).then_some(Scope::Local))
    }

    fn var(&self, scope: Scope, name: &str) -> &Var {
        match scope {
            Scope::Local => &self.locals[name],
            s => &self.ctx.scope_map(s).expect("non-local scope")[name],
        }
    }

    fn var_mut(&mut self, scope: Scope, name: &str) -> &mut Var {
        match scope {
            Scope::Local => self.locals.get_mut(name).expect("resolved local"),
            s => self
                .ctx
                .scope_map_mut(s)
                .expect("non-local scope")
                .get_mut(name)
                .expect("resolved variable"),
        }
    }

    fn ref_var(&self, r: &VarRef) -> &Var {
        self.var(r.scope.expect("resolved reference"), &r.name)
    }

    fn ref_var_mut(&mut self, r: &VarRef) -> &mut Var {
        self.var_mut(r.scope.expect("resolved reference"), &r.name)
    }

    fn check_writable(&self, r: &VarRef) -> R<()> {
        if r.scope == Some(Scope::Config) {
            return err(r.start, "Cannot modify a read-only variable");
        }
        Ok(())
    }

    /// `variable_reference`: `variable ('[' > additive_expression > ']')?`.
    fn variable_reference(&mut self) -> R<Option<VarRef>> {
        let Some((start, end)) = self.identifier()? else {
            return Ok(None);
        };
        let name = &self.src[start..end];
        let scope = if self.skipping() {
            None
        } else {
            match self.lookup(name) {
                Some(s) => Some(s),
                None => return err(start, "Not a variable name"),
            }
        };
        let mut r = VarRef {
            name: name.to_string(),
            scope,
            index: None,
            start,
            end,
        };
        if self.lit(b'[')? {
            let Some(index_expr) = self.additive_expression()? else {
                return self.expected("Expecting an expression.");
            };
            let index = match index_expr.v {
                Ev::Empty => 0,
                Ev::Int(i) => i,
                _ => {
                    return index_expr
                        .fail("Non-integer index is not allowed to address a vector variable.");
                }
            };
            self.expect_lit(b']')?;
            if !self.skipping() {
                if !self.ref_var(&r).is_vector() {
                    return err(start, "Cannot index a scalar variable");
                }
                if index < 0 {
                    return err(start, "Referencing a vector variable with a negative index");
                }
                r.index = Some(index);
            }
            r.end = self.pos;
        }
        Ok(Some(r))
    }

    fn expect_variable_reference(&mut self) -> R<VarRef> {
        match self.variable_reference()? {
            Some(r) => Ok(r),
            None => self.expected("Expecting a variable reference."),
        }
    }

    /// `variable_value`: reads a scalar or an indexed vector element.
    fn variable_value(&self, r: &VarRef) -> R<Expr> {
        if self.skipping() {
            return Ok(Expr::new(Ev::Empty, r.start, r.end));
        }
        let v = match self.ref_var(r) {
            Var::Scalar(Value::Nil) => {
                return err(
                    r.start,
                    "Trying to reference an undefined (nil) optional variable",
                );
            }
            Var::Scalar(v) => value_to_ev(v),
            Var::Vector(ty, values) => {
                let Some(index) = r.index else {
                    return err(
                        r.start,
                        "Referencing a vector variable when scalar is expected",
                    );
                };
                if values.is_empty() {
                    return err(r.start, "Indexing an empty vector variable");
                }
                let idx = if index as usize >= values.len() {
                    0
                } else {
                    index as usize
                };
                if values[idx].is_nil() {
                    return err(
                        r.start,
                        "Trying to reference an undefined (nil) element of vector of optional values",
                    );
                }
                element_to_ev(*ty, &values[idx])
            }
        };
        Ok(Expr::new(v, r.start, r.end))
    }

    // ---- text ----

    /// `text_block`: free text, `{macros}` and legacy `[name]` expansions.
    fn text_block(&mut self) -> R<String> {
        let mut out = String::new();
        loop {
            let start = self.pos;
            while self.pos < self.b.len() && self.b[self.pos] != b'[' && self.b[self.pos] != b'{' {
                self.pos += 1;
            }
            if self.pos > start {
                out.push_str(&self.src[start..self.pos]);
                continue;
            }
            match self.b.get(self.pos) {
                None => break,
                Some(b'{') => {
                    // Backtracks if no macro follows, so that an enclosing `{if}` can see `{else}` etc.
                    let save = self.pos;
                    self.pos += 1;
                    let (s, n) = self.macro_items(Term::Brace)?;
                    if n == 0 {
                        self.pos = save;
                        break;
                    }
                    self.expect_lit(b'}')?;
                    out.push_str(&s);
                }
                Some(_) => {
                    self.pos += 1;
                    let s = self.legacy_variable_expansion()?;
                    self.expect_lit(b']')?;
                    out.push_str(&s);
                }
            }
        }
        Ok(out)
    }

    /// `legacy_variable_expansion`: `name` or `name[index_name]` (after the opening `[`).
    fn legacy_variable_expansion(&mut self) -> R<String> {
        let save = self.pos;
        if let Some((s, e)) = self.identifier()? {
            if self.peek_lit(b']')? {
                return self.legacy_expand(s, e);
            }
            self.pos = save;
        }
        let Some((s, e)) = self.identifier()? else {
            return self.expected("Expecting a legacy variable expansion format");
        };
        self.expect_lit(b'[')?;
        let Some((is, ie)) = self.identifier()? else {
            return self.expected("Expecting an identifier.");
        };
        self.expect_lit(b']')?;
        self.legacy_expand_indexed(s, e, is, ie)
    }

    /// `[name]` or `[name_N]`: only read-only (config) variables are visible.
    fn legacy_expand(&mut self, s: usize, e: usize) -> R<String> {
        if self.skipping() {
            return Ok(String::new());
        }
        let name = &self.src[s..e];
        let mut idx = self.ctx.current_extruder();
        let mut var = self.ctx.config_var(name);
        if var.is_none() {
            // Legacy vector indexing: name_N.
            if let Some(u) = name.rfind('_') {
                var = self.ctx.config_var(&name[..u]);
                if let Some(v) = var {
                    if !v.is_vector() {
                        return err(s, "Trying to index a scalar variable");
                    }
                    // strtol(): an empty suffix parses as 0, anything but digits is an error.
                    let suffix = &name[u + 1..];
                    if !suffix.bytes().all(|c| c.is_ascii_digit()) {
                        return err(s + u + 1, "Invalid vector index");
                    }
                    idx = suffix
                        .parse()
                        .unwrap_or(if suffix.is_empty() { 0 } else { usize::MAX });
                }
            }
        }
        let Some(var) = var else {
            return err(s, "Variable does not exist");
        };
        match var {
            Var::Scalar(Value::Nil) => err(
                s,
                "Trying to reference an undefined (nil) optional variable",
            ),
            Var::Scalar(v) => Ok(serialize_scalar(v)),
            Var::Vector(ty, values) => {
                if values.is_empty() {
                    return err(s, "Indexing an empty vector variable");
                }
                if idx >= values.len() {
                    idx = 0;
                }
                if values[idx].is_nil() {
                    return err(
                        s,
                        "Trying to reference an undefined (nil) element of vector of optional values",
                    );
                }
                Ok(serialize_element(&element_value(*ty, &values[idx])))
            }
        }
    }

    /// `[name[index_name]]`.
    fn legacy_expand_indexed(&mut self, s: usize, e: usize, is: usize, ie: usize) -> R<String> {
        if self.skipping() {
            return Ok(String::new());
        }
        let mut name = &self.src[s..e];
        let mut var = self.ctx.config_var(name);
        if var.is_none() && name.ends_with('_') {
            name = &name[..name.len() - 1];
            var = self.ctx.config_var(name);
        }
        let Some(var) = var else {
            return err(s, "Variable does not exist");
        };
        let Var::Vector(ty, values) = var else {
            return err(s, "Trying to index a scalar variable");
        };
        if values.is_empty() {
            return err(s, "Indexing an empty vector variable");
        }
        let Some(index_var) = self.ctx.config_var(&self.src[is..ie]) else {
            return err(s, "Variable does not exist");
        };
        let Var::Scalar(Value::Int(index)) = index_var else {
            return err(s, "Indexing variable has to be integer");
        };
        if *index < 0 {
            return err(s, "Negative vector index");
        }
        let idx = if *index as usize >= values.len() {
            0
        } else {
            *index as usize
        };
        if values[idx].is_nil() {
            return err(
                s,
                "Trying to reference an undefined (nil) element of vector of optional values",
            );
        }
        Ok(serialize_element(&element_value(*ty, &values[idx])))
    }

    // ---- statements ----

    /// `macros`, `if_macros`, `else_macros`: blocks and `;`-separated statements.
    /// Returns the output and the number of items parsed.
    fn macro_items(&mut self, term: Term) -> R<(String, usize)> {
        let mut out = String::new();
        let mut n = 0;
        loop {
            if let Some(s) = self.block()? {
                out.push_str(&s);
                n += 1;
                continue;
            }
            if let Some(s) = self.statement()? {
                if !self.semicolons()? && !self.at_term(term)? {
                    return self.expected("Expecting ';' or the end of the block.");
                }
                out.push_str(&s);
                n += 1;
                continue;
            }
            if self.semicolons()? {
                n += 1;
                continue;
            }
            break;
        }
        Ok((out, n))
    }

    /// `+lit(';')`.
    fn semicolons(&mut self) -> R<bool> {
        let mut any = false;
        while self.lit(b';')? {
            any = true;
        }
        Ok(any)
    }

    fn at_term(&mut self, term: Term) -> R<bool> {
        let save = self.pos;
        let r = match term {
            Term::Brace => self.lit(b'}')?,
            Term::IfBranch => self.kw("elsif")? || self.kw("else")? || self.kw("endif")?,
            Term::ElseBranch => self.kw("endif")?,
        };
        self.pos = save;
        Ok(r)
    }

    /// `block`: `if ... endif`.
    fn block(&mut self) -> R<Option<String>> {
        if self.kw("if")? {
            Ok(Some(self.if_else_output()?))
        } else {
            Ok(None)
        }
    }

    /// `if_else_output`: everything after the `if` keyword up to and including `endif`.
    fn if_else_output(&mut self) -> R<String> {
        let mut out = String::new();
        let mut not_yet_consumed = true;

        let condition = self.bool_expr_eval()?;
        self.block_enter(condition);
        let s = self.if_branch_body(false)?;
        if self.block_exit(condition) && not_yet_consumed {
            out = s;
            not_yet_consumed = false;
        }

        while self.kw("elsif")? {
            // Evaluated (and may fail) even when an earlier branch was taken, like PrusaSlicer.
            let condition = self.bool_expr_eval()?;
            self.block_enter(condition && not_yet_consumed);
            let s = self.if_branch_body(false)?;
            if self.block_exit(condition) && not_yet_consumed {
                out = s;
                not_yet_consumed = false;
            }
        }

        if self.kw("else")? {
            let condition = not_yet_consumed;
            self.block_enter(condition);
            let s = self.if_branch_body(true)?;
            if self.block_exit(condition) && not_yet_consumed {
                out = s;
            }
        }

        if !self.kw("endif")? {
            return self.expected("Expecting {elsif}, {else} or {endif}.");
        }
        Ok(out)
    }

    /// `if_text_block | if_macros` (or `| else_macros` for the else branch).
    fn if_branch_body(&mut self, else_branch: bool) -> R<String> {
        if self.lit(b'}')? {
            let s = self.text_block()?;
            self.expect_lit(b'{')?;
            return Ok(s);
        }
        if else_branch {
            return Ok(self.macro_items(Term::ElseBranch)?.0);
        }
        if self.kw("then")? {
            return Ok(self.macro_items(Term::IfBranch)?.0);
        }
        self.expected("Not an {if}{else}{endif} macro: expecting '}' or 'then'.")
    }

    /// `bool_expr_eval`.
    fn bool_expr_eval(&mut self) -> R<bool> {
        let Some(e) = self.conditional_expression()? else {
            return self.expected("Expecting a boolean expression.");
        };
        match e.v {
            Ev::Empty => Ok(false),
            Ev::Bool(b) => Ok(b),
            _ => e.fail("Not a boolean expression"),
        }
    }

    /// `statement`: assignment, new variable, or an expression whose value is output.
    fn statement(&mut self) -> R<Option<String>> {
        if self.assignment_statement()? {
            return Ok(Some(String::new()));
        }
        if self.new_variable_statement()? {
            return Ok(Some(String::new()));
        }
        if let Some(e) = self.conditional_expression()? {
            return Ok(Some(e.text()));
        }
        Ok(None)
    }

    /// `assignment_statement`: `variable_reference '=' rhs`. A single `=` commits, so a statement
    /// such as `{a == b}` starting with a variable is a syntax error (as in PrusaSlicer).
    fn assignment_statement(&mut self) -> R<bool> {
        let save = self.pos;
        let Some(lhs) = self.variable_reference()? else {
            return Ok(false);
        };
        if !self.lit(b'=')? {
            self.pos = save;
            return Ok(false);
        }
        if let Some(il) = self.initializer_list()? {
            self.vector_assign_initializer_list(&lhs, &il)?;
            return Ok(true);
        }
        if lhs.index.is_none() && lhs.scope.is_some() && self.ref_var(&lhs).is_vector() {
            let save = self.pos;
            if let Some(rhs) = self.variable_reference()? {
                self.copy_vector(&lhs, &rhs)?;
                return Ok(true);
            }
            self.pos = save;
        }
        if let Some(rhs) = self.conditional_expression()? {
            self.assign_scalar_expression(&lhs, &rhs)?;
            return Ok(true);
        }
        if let Some((count, value)) = self.repeat()? {
            if !self.skipping() {
                self.check_writable(&lhs)?;
                if !self.ref_var(&lhs).is_vector() {
                    return value.fail("Cannot assign a vector value to a scalar variable.");
                }
                self.assign_with_count(&lhs, &count, &value)?;
            }
            return Ok(true);
        }
        self.expected("Expecting an expression, an initializer list or repeat().")
    }

    /// `new_variable_statement`: `(local | global) identifier '=' rhs`.
    fn new_variable_statement(&mut self) -> R<bool> {
        let global = if self.kw("local")? {
            false
        } else if self.kw("global")? {
            true
        } else {
            return Ok(false);
        };
        let Some((start, end)) = self.identifier()? else {
            return self.expected("Expecting an identifier.");
        };
        let nv = self.new_old_variable(global, start, end)?;
        self.expect_lit(b'=')?;

        if let Some(il) = self.initializer_list()? {
            self.vector_new_from_initializer_list(&nv, &il)?;
            return Ok(true);
        }
        let could_be_vector = match nv.existing {
            None => true,
            Some(scope) => self.var(scope, &nv.name).is_vector(),
        };
        if could_be_vector {
            // While skipping this alternative always "succeeds", so `local x = y + 1` inside an
            // inactive branch is a syntax error in PrusaSlicer too.
            let save = self.pos;
            if let Some(rhs) = self.variable_reference()?
                && self.vector_new_from_copy(&nv, &rhs)?
            {
                return Ok(true);
            }
            self.pos = save;
        }
        if let Some(rhs) = self.conditional_expression()? {
            self.scalar_new_from_expression(&nv, &rhs)?;
            return Ok(true);
        }
        if let Some((count, value)) = self.repeat()? {
            self.vector_new_from_array(&nv, &count, &value)?;
            return Ok(true);
        }
        self.expected("Expecting an expression, an initializer list or repeat().")
    }

    /// `repeat '(' additive_expression ',' conditional_expression ')'`.
    fn repeat(&mut self) -> R<Option<(Expr, Expr)>> {
        if !self.kw("repeat")? {
            return Ok(None);
        }
        self.expect_lit(b'(')?;
        let Some(count) = self.additive_expression()? else {
            return self.expected("Expecting an expression.");
        };
        self.expect_lit(b',')?;
        let value = self.expect_conditional()?;
        self.expect_lit(b')')?;
        Ok(Some((count, value)))
    }

    /// `initializer_list`: `'(' (')' | expr (',' expr)* ')')`; skipped values are dropped.
    fn initializer_list(&mut self) -> R<Option<Vec<Expr>>> {
        if !self.lit(b'(')? {
            return Ok(None);
        }
        let mut list = Vec::new();
        if self.lit(b')')? {
            return Ok(Some(list));
        }
        loop {
            let e = self.expect_conditional()?;
            if !e.is_empty() {
                list.push(e);
            }
            if !self.lit(b',')? {
                break;
            }
        }
        self.expect_lit(b')')?;
        Ok(Some(list))
    }

    fn new_old_variable(&mut self, global: bool, start: usize, end: usize) -> R<NewVar> {
        let name = self.src[start..end].to_string();
        let mut nv = NewVar {
            name,
            global,
            existing: None,
            start,
            end,
        };
        if self.skipping() {
            return Ok(nv);
        }
        if self.ctx.config_var(&nv.name).is_some() {
            return err(
                start,
                "Symbol is already defined in read-only system dictionary",
            );
        }
        if self.ctx.has_output(&nv.name) {
            return err(start, "Symbol is already defined as system output variable");
        }
        let in_globals = self.ctx.global(&nv.name).is_some();
        if global {
            if self.locals.contains_key(&nv.name) {
                return err(start, "Variable name already defined in local scope");
            }
            nv.existing = in_globals.then_some(Scope::Global);
        } else {
            if in_globals {
                return err(start, "Variable name already defined in global scope");
            }
            nv.existing = self.locals.contains_key(&nv.name).then_some(Scope::Local);
        }
        Ok(nv)
    }

    fn store_new_variable(&mut self, nv: &NewVar, var: Var) {
        if nv.global {
            self.ctx
                .scope_map_mut(Scope::Global)
                .expect("global scope")
                .insert(nv.name.clone(), var);
        } else {
            self.locals.insert(nv.name.clone(), var);
        }
    }

    /// `scalar_variable_assign_scalar_expression`.
    fn assign_scalar_expression(&mut self, lhs: &VarRef, rhs: &Expr) -> R<()> {
        if self.skipping() {
            return Ok(());
        }
        self.check_writable(lhs)?;
        if self.ref_var(lhs).is_vector() {
            self.assign_vector_element(lhs, rhs)
        } else {
            self.assign_scalar(lhs, rhs)
        }
    }

    /// `scalar_variable_assign_scalar`.
    fn assign_scalar(&mut self, lhs: &VarRef, rhs: &Expr) -> R<()> {
        self.check_writable(lhs)?;
        let current = match self.ref_var(lhs) {
            Var::Scalar(v) => v.value_type(),
            Var::Vector(..) => unreachable!("scalar expected"),
        };
        // A nil scalar has no type of its own here; it takes the type of the assigned value.
        let Some(ty) = current.or_else(|| ev_type(&rhs.v)) else {
            return Ok(());
        };
        let v = convert(ty, rhs)?;
        *self.ref_var_mut(lhs) = Var::Scalar(v);
        Ok(())
    }

    /// `vector_variable_element_assign_scalar`.
    fn assign_vector_element(&mut self, lhs: &VarRef, rhs: &Expr) -> R<()> {
        self.check_writable(lhs)?;
        let Some(index) = lhs.index else {
            return err(
                lhs.start,
                "Referencing an output vector variable when scalar is expected",
            );
        };
        let Var::Vector(ty, values) = self.ref_var(lhs) else {
            unreachable!("vector expected")
        };
        if values.is_empty() {
            return err(lhs.start, "Indexing an empty vector variable");
        }
        if index as usize >= values.len() {
            return err(lhs.start, "Index out of range");
        }
        let v = convert(*ty, rhs)?;
        if let Var::Vector(_, values) = self.ref_var_mut(lhs) {
            values[index as usize] = v;
        }
        Ok(())
    }

    fn evaluate_count(e: &Expr) -> R<usize> {
        match e.v {
            Ev::Int(n) if n < 0 => e.fail("Negative number of elements specified."),
            Ev::Int(n) => Ok(n as usize),
            _ => e.fail("Expected number of elements to fill a vector with."),
        }
    }

    /// `vector_variable_assign_expr_with_count`.
    fn assign_with_count(&mut self, lhs: &VarRef, count: &Expr, value: &Expr) -> R<()> {
        let count = Self::evaluate_count(count)?;
        let Var::Vector(ty, _) = self.ref_var(lhs) else {
            unreachable!("vector expected")
        };
        let ty = *ty;
        let v = convert(ty, value)?;
        *self.ref_var_mut(lhs) = Var::Vector(ty, vec![v; count]);
        Ok(())
    }

    fn fill_vector(ty: ValueType, il: &[Expr]) -> R<Vec<Value>> {
        match ty {
            ValueType::Float | ValueType::Int => {
                for e in il {
                    e.check_numeric("Right side is not a numeric expression")?;
                }
            }
            ValueType::Bool => {
                for e in il {
                    if !matches!(e.v, Ev::Bool(_)) {
                        return e.fail("Right side is not a boolean expression");
                    }
                }
            }
            ValueType::Str => {}
        }
        il.iter().map(|e| convert(ty, e)).collect()
    }

    /// `vector_variable_assign_initializer_list`.
    fn vector_assign_initializer_list(&mut self, lhs: &VarRef, il: &[Expr]) -> R<()> {
        if self.skipping() {
            return Ok(());
        }
        self.check_writable(lhs)?;
        match self.ref_var(lhs) {
            Var::Scalar(_) => {
                if il.len() == 1 {
                    // scalar_var = ( scalar )
                    self.assign_scalar_expression(lhs, &il[0])
                } else {
                    err(
                        lhs.start,
                        "Cannot assign a vector value to a scalar variable.",
                    )
                }
            }
            Var::Vector(ty, _) => {
                let ty = *ty;
                let values = Self::fill_vector(ty, il)?;
                *self.ref_var_mut(lhs) = Var::Vector(ty, values);
                Ok(())
            }
        }
    }

    /// `vector_variable_new_from_initializer_list`.
    fn vector_new_from_initializer_list(&mut self, nv: &NewVar, il: &[Expr]) -> R<()> {
        if self.skipping() {
            return Ok(());
        }
        if nv.existing.is_some() {
            return self.vector_assign_initializer_list(&nv.as_ref(), il);
        }
        if il.is_empty() {
            return err(
                nv.start,
                "Cannot create vector variable from an empty initializer list, because its type cannot be deduced.",
            );
        }
        let count = |f: fn(&Ev) -> bool| il.iter().filter(|e| f(&e.v)).count();
        let num_bool = count(|v| matches!(v, Ev::Bool(_)));
        let num_int = count(|v| matches!(v, Ev::Int(_)));
        let num_double = count(|v| matches!(v, Ev::Double(_)));
        let num_string = count(|v| matches!(v, Ev::Str(_)));
        let ty = if num_string > 0 {
            ValueType::Str
        } else if num_bool > 0 {
            if num_double + num_int > 0 {
                return err(
                    il[0].start,
                    "Right side is not valid: Mixing numeric and boolean types.",
                );
            }
            ValueType::Bool
        } else if num_double == 0 {
            ValueType::Int
        } else {
            ValueType::Float
        };
        let values = Self::fill_vector(ty, il)?;
        self.store_new_variable(nv, Var::Vector(ty, values));
        Ok(())
    }

    /// `copy_vector_variable_to_vector_variable`.
    fn copy_vector(&mut self, lhs: &VarRef, rhs: &VarRef) -> R<()> {
        if self.skipping() {
            return Ok(());
        }
        self.check_writable(lhs)?;
        let rhs_var = self.ref_var(rhs);
        let Var::Vector(rty, values) = rhs_var else {
            return err(lhs.start, "Cannot assign scalar to a vector");
        };
        if rhs.index.is_some() {
            return err(lhs.start, "Cannot assign scalar to a vector");
        }
        if all_nil(rhs_var) {
            return err(
                rhs.start,
                "Some elements of the right hand side vector variable of optional values are undefined (nil)",
            );
        }
        let (rty, values) = (*rty, values.clone());
        let Var::Vector(lty, _) = self.ref_var(lhs) else {
            unreachable!("vector expected")
        };
        if *lty != rty {
            let msg = match lty {
                ValueType::Float => {
                    "Left hand side is a float vector, while the right hand side is not."
                }
                ValueType::Int => {
                    "Left hand side is an int vector, while the right hand side is not."
                }
                ValueType::Str => {
                    "Left hand side is a string vector, while the right hand side is not."
                }
                ValueType::Bool => {
                    "Left hand side is a bool vector, while the right hand side is not."
                }
            };
            return err(lhs.start, msg);
        }
        *self.ref_var_mut(lhs) = Var::Vector(rty, values);
        Ok(())
    }

    /// `vector_variable_new_from_copy`; `Ok(false)` lets the caller try a scalar expression instead.
    fn vector_new_from_copy(&mut self, nv: &NewVar, rhs: &VarRef) -> R<bool> {
        if self.skipping() {
            return Ok(true);
        }
        if nv.existing.is_some() {
            self.copy_vector(&nv.as_ref(), rhs)?;
            return Ok(true);
        }
        let rhs_var = self.ref_var(rhs);
        if rhs.index.is_some() || !rhs_var.is_vector() {
            return Ok(false);
        }
        if all_nil(rhs_var) {
            return err(
                rhs.start,
                "Some elements of the right hand side vector variable of optional values are undefined (nil)",
            );
        }
        let copy = rhs_var.clone();
        self.store_new_variable(nv, copy);
        Ok(true)
    }

    /// `scalar_variable_new_from_scalar_expression`.
    fn scalar_new_from_expression(&mut self, nv: &NewVar, rhs: &Expr) -> R<()> {
        if self.skipping() {
            return Ok(());
        }
        if let Some(scope) = nv.existing {
            if self.var(scope, &nv.name).is_vector() {
                return rhs.fail("Cannot assign a scalar value to a vector variable.");
            }
            return self.assign_scalar(&nv.as_ref(), rhs);
        }
        let v = match &rhs.v {
            Ev::Bool(b) => Value::Bool(*b),
            Ev::Int(i) => Value::Int(*i),
            Ev::Double(d) => Value::Float(*d),
            Ev::Str(s) => Value::Str(s.clone()),
            Ev::Empty => return Ok(()),
        };
        self.store_new_variable(nv, Var::Scalar(v));
        Ok(())
    }

    /// `vector_variable_new_from_array` (`repeat()`).
    fn vector_new_from_array(&mut self, nv: &NewVar, count: &Expr, value: &Expr) -> R<()> {
        if self.skipping() {
            return Ok(());
        }
        if let Some(scope) = nv.existing {
            if !self.var(scope, &nv.name).is_vector() {
                return value.fail("Cannot assign a vector value to a scalar variable.");
            }
            return self.assign_with_count(&nv.as_ref(), count, value);
        }
        let n = Self::evaluate_count(count)?;
        let (ty, v) = match &value.v {
            Ev::Bool(b) => (ValueType::Bool, Value::Bool(*b)),
            Ev::Int(i) => (ValueType::Int, Value::Int(*i)),
            Ev::Double(d) => (ValueType::Float, Value::Float(*d)),
            Ev::Str(s) => (ValueType::Str, Value::Str(s.clone())),
            Ev::Empty => return Ok(()),
        };
        self.store_new_variable(nv, Var::Vector(ty, vec![v; n]));
        Ok(())
    }

    // ---- expressions ----

    fn expect_conditional(&mut self) -> R<Expr> {
        match self.conditional_expression()? {
            Some(e) => Ok(e),
            None => self.expected("Expecting an expression."),
        }
    }

    fn expect_unary(&mut self) -> R<Expr> {
        match self.unary_expression()? {
            Some(e) => Ok(e),
            None => self.expected("Expecting an expression."),
        }
    }

    /// `conditional_expression`: `logical_or ('?' cond ':' cond)?`.
    fn conditional_expression(&mut self) -> R<Option<Expr>> {
        let Some(mut val) = self.logical_or_expression()? else {
            return Ok(None);
        };
        if self.lit(b'?')? {
            let condition = match val.v {
                Ev::Empty => false,
                Ev::Bool(b) => b,
                _ => return val.fail("Not a boolean expression"),
            };
            self.block_enter(condition);
            let e = self.expect_conditional()?;
            if self.block_exit(condition) {
                val = e;
            }
            self.expect_lit(b':')?;
            self.block_enter(!condition);
            let e = self.expect_conditional()?;
            if self.block_exit(!condition) {
                val = e;
            }
        }
        Ok(Some(val))
    }

    fn logical_or_expression(&mut self) -> R<Option<Expr>> {
        let Some(mut val) = self.logical_and_expression()? else {
            return Ok(None);
        };
        while self.kw("or")? || self.lit_str("||")? {
            let Some(rhs) = self.logical_and_expression()? else {
                return self.expected("Expecting a boolean expression.");
            };
            logical(&mut val, &rhs, true)?;
        }
        Ok(Some(val))
    }

    fn logical_and_expression(&mut self) -> R<Option<Expr>> {
        let Some(mut val) = self.equality_expression()? else {
            return Ok(None);
        };
        while self.kw("and")? || self.lit_str("&&")? {
            let Some(rhs) = self.equality_expression()? else {
                return self.expected("Expecting a boolean expression.");
            };
            logical(&mut val, &rhs, false)?;
        }
        Ok(Some(val))
    }

    fn expect_relational(&mut self) -> R<Expr> {
        match self.relational_expression()? {
            Some(e) => Ok(e),
            None => self.expected("Expecting an expression."),
        }
    }

    /// `equality_expression`: `==`, `!=`, `<>`, `=~ /re/`, `!~ /re/`.
    ///
    /// `<>` is unreachable in practice: the relational rule below consumes its `<` first and then
    /// fails on `>`, exactly as in PrusaSlicer.
    fn equality_expression(&mut self) -> R<Option<Expr>> {
        let Some(mut val) = self.relational_expression()? else {
            return Ok(None);
        };
        loop {
            if self.lit_str("==")? {
                let rhs = self.expect_relational()?;
                compare(&mut val, &rhs, CmpOp::Eq, false)?;
            } else if self.lit_str("!=")? || self.lit_str("<>")? {
                let rhs = self.expect_relational()?;
                compare(&mut val, &rhs, CmpOp::Eq, true)?;
            } else if self.lit_str("=~")? {
                self.regex_op(&mut val, false)?;
            } else if self.lit_str("!~")? {
                self.regex_op(&mut val, true)?;
            } else {
                break;
            }
        }
        Ok(Some(val))
    }

    /// `regular_expression`: returns the pattern's byte range (without the slashes).
    fn expect_regular_expression(&mut self) -> R<(usize, usize)> {
        match self.delimited(b'/')? {
            Some((s, e)) => Ok((s, e)),
            None => self.expected("Expecting a regular expression."),
        }
    }

    /// Full-match `lhs` against a regex literal; the result replaces `lhs`.
    fn regex_op(&mut self, lhs: &mut Expr, invert: bool) -> R<()> {
        let (s, e) = self.expect_regular_expression()?;
        if lhs.is_empty() {
            return Ok(());
        }
        let Ev::Str(subject) = &lhs.v else {
            return lhs.fail("Left hand side of a regex match must be a string.");
        };
        let matched = self.regex_match(subject, s, e)?;
        lhs.v = Ev::Bool(matched != invert);
        lhs.end = e;
        Ok(())
    }

    /// Matches `subject` against the regex literal at `s..e` (including slashes).
    fn regex_match(&mut self, subject: &str, s: usize, e: usize) -> R<bool> {
        let pattern = &self.src[s + 1..e - 1];
        match self.ctx.full_match_regex(pattern) {
            Ok(re) => Ok(re.is_match(subject)),
            Err(ex) => err(s, format!("Regular expression compilation failed: {ex}")),
        }
    }

    fn relational_expression(&mut self) -> R<Option<Expr>> {
        let Some(mut val) = self.additive_expression()? else {
            return Ok(None);
        };
        loop {
            let (op, invert) = if self.lit_str("<=")? {
                (CmpOp::Gt, true)
            } else if self.lit_str(">=")? {
                (CmpOp::Lt, true)
            } else if self.lit(b'<')? {
                (CmpOp::Lt, false)
            } else if self.lit(b'>')? {
                (CmpOp::Gt, false)
            } else {
                break;
            };
            let Some(rhs) = self.additive_expression()? else {
                return self.expected("Expecting an expression.");
            };
            compare(&mut val, &rhs, op, invert)?;
        }
        Ok(Some(val))
    }

    fn additive_expression(&mut self) -> R<Option<Expr>> {
        let Some(mut val) = self.multiplicative_expression()? else {
            return Ok(None);
        };
        loop {
            let plus = if self.lit(b'+')? {
                true
            } else if self.lit(b'-')? {
                false
            } else {
                break;
            };
            let Some(rhs) = self.multiplicative_expression()? else {
                return self.expected("Expecting an expression.");
            };
            if plus {
                add(&mut val, &rhs)?;
            } else {
                arith(&mut val, &rhs, ArithOp::Sub)?;
            }
        }
        Ok(Some(val))
    }

    fn multiplicative_expression(&mut self) -> R<Option<Expr>> {
        let Some(mut val) = self.unary_expression()? else {
            return Ok(None);
        };
        loop {
            let op = if self.lit(b'*')? {
                ArithOp::Mul
            } else if self.lit(b'/')? {
                ArithOp::Div
            } else if self.lit(b'%')? {
                ArithOp::Mod
            } else {
                break;
            };
            let rhs = self.expect_unary()?;
            arith(&mut val, &rhs, op)?;
        }
        Ok(Some(val))
    }

    /// `unary_expression`: variables, parentheses, unary operators, function calls and literals.
    fn unary_expression(&mut self) -> R<Option<Expr>> {
        let save = self.pos;
        self.skip_ws()?;
        let start = self.pos;

        if let Some(r) = self.variable_reference()? {
            return Ok(Some(self.variable_value(&r)?));
        }
        if self.lit(b'(')? {
            let e = self.expect_conditional()?;
            self.expect_lit(b')')?;
            return Ok(Some(Expr::new(e.v, start, self.pos)));
        }
        if self.lit(b'-')? {
            let e = self.expect_unary()?;
            let v = match e.v {
                Ev::Empty => Ev::Empty,
                Ev::Int(i) => Ev::Int(i.wrapping_neg()),
                Ev::Double(d) => Ev::Double(-d),
                _ => return e.fail("Cannot apply unary minus operator."),
            };
            return Ok(Some(Expr::new(v, start, self.pos)));
        }
        if self.lit(b'+')? {
            let e = self.expect_unary()?;
            return Ok(Some(Expr::new(e.v, start, self.pos)));
        }
        if self.kw("not")? || self.lit(b'!')? {
            let e = self.expect_unary()?;
            let v = match e.v {
                Ev::Empty => Ev::Empty,
                Ev::Bool(b) => Ev::Bool(!b),
                _ => return e.fail("Cannot apply a not operator."),
            };
            return Ok(Some(Expr::new(v, start, self.pos)));
        }
        for (word, min) in [("min", true), ("max", false)] {
            if self.kw(word)? {
                self.expect_lit(b'(')?;
                let mut a = self.expect_conditional()?;
                self.expect_lit(b',')?;
                let b = self.expect_conditional()?;
                self.expect_lit(b')')?;
                min_max(&mut a, &b, min)?;
                return Ok(Some(Expr::new(a.v, start, self.pos)));
            }
        }
        if self.kw("random")? {
            self.expect_lit(b'(')?;
            let a = self.expect_conditional()?;
            self.expect_lit(b',')?;
            let b = self.expect_conditional()?;
            self.expect_lit(b')')?;
            let v = self.random(&a, &b)?;
            return Ok(Some(Expr::new(v, start, self.pos)));
        }
        for (word, zero_pad) in [("digits", false), ("zdigits", true)] {
            if self.kw(word)? {
                self.expect_lit(b'(')?;
                let mut a = self.expect_conditional()?;
                self.expect_lit(b',')?;
                let width = self.expect_conditional()?;
                let decimals = if self.lit(b')')? {
                    Expr::new(Ev::Empty, self.pos, self.pos)
                } else if self.lit(b',')? {
                    let d = self.expect_conditional()?;
                    self.expect_lit(b')')?;
                    d
                } else {
                    return self.expected("Expecting a closing brace or an optional parameter.");
                };
                digits(&mut a, &width, &decimals, zero_pad)?;
                return Ok(Some(Expr::new(a.v, start, self.pos)));
            }
        }
        if self.kw("int")? {
            self.expect_lit(b'(')?;
            let e = self.expect_conditional()?;
            self.expect_lit(b')')?;
            let v = match e.v {
                Ev::Empty => Ev::Empty,
                Ev::Int(i) => Ev::Int(i),
                Ev::Double(d) => Ev::Int(d as i64),
                _ => return e.fail("Cannot convert to integer."),
            };
            return Ok(Some(Expr::new(v, start, self.pos)));
        }
        if self.kw("round")? {
            self.expect_lit(b'(')?;
            let e = self.expect_conditional()?;
            self.expect_lit(b')')?;
            let v = match e.v {
                Ev::Empty => Ev::Empty,
                Ev::Int(i) => Ev::Int(i),
                Ev::Double(d) => Ev::Int(d.round() as i64),
                _ => return e.fail("Cannot round a non-numeric value."),
            };
            return Ok(Some(Expr::new(v, start, self.pos)));
        }
        if self.kw("is_nil")? {
            self.expect_lit(b'(')?;
            let r = self.expect_variable_reference()?;
            self.expect_lit(b')')?;
            let v = self.is_nil_test(&r)?;
            return Ok(Some(Expr::new(v, start, self.pos)));
        }
        if self.kw("one_of")? {
            self.expect_lit(b'(')?;
            let v = self.one_of()?;
            self.expect_lit(b')')?;
            return Ok(Some(Expr::new(v, start, self.pos)));
        }
        for (word, is_size) in [("empty", false), ("size", true)] {
            if self.kw(word)? {
                self.expect_lit(b'(')?;
                let r = self.expect_variable_reference()?;
                self.expect_lit(b')')?;
                let v = if self.skipping() {
                    Ev::Empty
                } else {
                    match self.ref_var(&r) {
                        Var::Vector(_, values) if r.index.is_none() => {
                            if is_size {
                                Ev::Int(values.len() as i64)
                            } else {
                                Ev::Bool(values.is_empty())
                            }
                        }
                        _ => {
                            return err(
                                r.start,
                                format!("parameter of {word}() is not a vector variable"),
                            );
                        }
                    }
                };
                return Ok(Some(Expr::new(v, start, self.pos)));
            }
        }
        if self.kw("interpolate_table")? {
            self.expect_lit(b'(')?;
            let v = self.interpolate_table()?;
            self.expect_lit(b')')?;
            return Ok(Some(Expr::new(v, start, self.pos)));
        }
        if let Some(v) = self.number()? {
            let v = if self.skipping() { Ev::Empty } else { v };
            return Ok(Some(Expr::new(v, start, self.pos)));
        }
        for (word, value) in [("true", true), ("false", false)] {
            if self.kw(word)? {
                let v = if self.skipping() {
                    Ev::Empty
                } else {
                    Ev::Bool(value)
                };
                return Ok(Some(Expr::new(v, start, self.pos)));
            }
        }
        if let Some((s, e)) = self.delimited(b'"')? {
            let v = if self.skipping() {
                Ev::Empty
            } else {
                Ev::Str(self.unescape_string(s, e)?)
            };
            return Ok(Some(Expr::new(v, s, e)));
        }
        self.pos = save;
        Ok(None)
    }

    /// Numeric literal: a double needs a '.' or an exponent (Spirit's strict real policy, no
    /// inf/nan), otherwise it is an int.
    fn number(&mut self) -> R<Option<Ev>> {
        let save = self.pos;
        self.skip_ws()?;
        let start = self.pos;
        let digits_from = |p: usize, b: &[u8]| {
            let mut q = p;
            while b.get(q).is_some_and(u8::is_ascii_digit) {
                q += 1;
            }
            q
        };
        let int_end = digits_from(start, self.b);
        let n_int = int_end - start;
        let mut end = int_end;
        let mut is_double = false;
        if self.b.get(end) == Some(&b'.') {
            let frac_end = digits_from(end + 1, self.b);
            if n_int > 0 || frac_end > end + 1 {
                is_double = true;
                end = frac_end;
            }
        }
        if (n_int > 0 || is_double) && matches!(self.b.get(end), Some(b'e' | b'E')) {
            let mut q = end + 1;
            if matches!(self.b.get(q), Some(b'+' | b'-')) {
                q += 1;
            }
            let exp_end = digits_from(q, self.b);
            if exp_end > q {
                is_double = true;
                end = exp_end;
            }
        }
        if is_double {
            if let Ok(d) = self.src[start..end].parse::<f64>() {
                self.pos = end;
                return Ok(Some(Ev::Double(d)));
            }
        } else if n_int > 0
            && let Ok(i) = self.src[start..int_end].parse::<i64>()
        {
            self.pos = int_end;
            return Ok(Some(Ev::Int(i)));
        }
        self.pos = save;
        Ok(None)
    }

    /// String literal contents with `\r`, `\n`, `\"` and `\\` escapes.
    fn unescape_string(&self, s: usize, e: usize) -> R<String> {
        let body = &self.src[s + 1..e - 1];
        let mut out = String::with_capacity(body.len());
        let mut chars = body.char_indices();
        while let Some((i, c)) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some((_, 'r')) => out.push('\r'),
                Some((_, 'n')) => out.push('\n'),
                Some((_, '"')) => out.push('"'),
                Some((_, '\\')) => out.push('\\'),
                _ => return err(s + 1 + i, "Invalid escape sequence"),
            }
        }
        Ok(out)
    }

    fn random(&mut self, a: &Expr, b: &Expr) -> R<Ev> {
        if self.skipping() || a.is_empty() {
            return Ok(Ev::Empty);
        }
        a.check_numeric("Not a numeric type.")?;
        b.check_numeric("Not a numeric type.")?;
        if a.is_double() || b.is_double() {
            Ok(Ev::Double(self.ctx.random_float(a.as_d(), b.as_d())))
        } else {
            let (lo, hi) = (a.as_i(), b.as_i());
            if lo > hi {
                return a.fail("random(): the lower bound is greater than the upper bound.");
            }
            Ok(Ev::Int(self.ctx.random_int(lo, hi)))
        }
    }

    /// `is_nil_test`.
    fn is_nil_test(&self, r: &VarRef) -> R<Ev> {
        if self.skipping() {
            return Ok(Ev::Empty);
        }
        match self.ref_var(r) {
            Var::Scalar(v) => Ok(Ev::Bool(v.is_nil())),
            Var::Vector(_, values) => {
                let Some(index) = r.index else {
                    return err(
                        r.start,
                        "Referencing a vector variable when scalar is expected",
                    );
                };
                if values.is_empty() {
                    return err(r.start, "Indexing an empty vector variable");
                }
                let idx = if index as usize >= values.len() {
                    0
                } else {
                    index as usize
                };
                Ok(Ev::Bool(values[idx].is_nil()))
            }
        }
    }

    /// `one_of`: `subject (',' (pattern | '~' regex_string | /regex/) ','?)*`.
    fn one_of(&mut self) -> R<Ev> {
        let subject = self.expect_unary()?;
        let mut out = Ev::Bool(false);
        if !self.lit(b',')? {
            return Ok(out);
        }
        loop {
            if let Some(pattern) = self.unary_expression()? {
                self.one_of_test(&subject, &pattern, false, &mut out)?;
            } else if self.lit(b'~')? {
                let pattern = self.expect_unary()?;
                self.one_of_test(&subject, &pattern, true, &mut out)?;
            } else if let Some((s, e)) = self.delimited(b'/')? {
                if subject.is_empty() {
                    out = Ev::Empty;
                } else if out == Ev::Bool(false) {
                    let Ev::Str(text) = &subject.v else {
                        return subject.fail(
                            "one_of(): First parameter (the string to match against) has to be a string value",
                        );
                    };
                    out = Ev::Bool(self.regex_match(text, s, e)?);
                }
            } else {
                break;
            }
            self.lit(b',')?;
        }
        Ok(out)
    }

    fn one_of_test(&mut self, subject: &Expr, pattern: &Expr, regex: bool, out: &mut Ev) -> R<()> {
        if subject.is_empty() {
            *out = Ev::Empty;
            return Ok(());
        }
        if *out != Ev::Bool(false) {
            return Ok(());
        }
        let Ev::Str(text) = &subject.v else {
            return subject.fail(
                "one_of(): First parameter (the string to match against) has to be a string value",
            );
        };
        let Ev::Str(pat) = &pattern.v else {
            return subject.fail("one_of(): Pattern has to be a string value");
        };
        *out = Ev::Bool(if regex {
            match self.ctx.full_match_regex(pat) {
                Ok(re) => re.is_match(text),
                Err(_) => return pattern.fail("Regular expression compilation failed"),
            }
        } else {
            text == pat
        });
        Ok(())
    }

    /// `interpolate_table(x, (x0, y0), (x1, y1), ...)`, after the opening parenthesis.
    fn interpolate_table(&mut self) -> R<Ev> {
        let x = self.expect_unary()?;
        self.expect_lit(b',')?;
        if !x.is_empty() && !x.numeric() {
            return x.fail("Interpolation value must be a number.");
        }
        let mut table: Vec<(f64, usize, f64)> = Vec::new();
        while self.lit(b'(')? {
            let tx = self.expect_unary()?;
            self.expect_lit(b',')?;
            let ty = self.expect_unary()?;
            self.expect_lit(b')')?;
            if !tx.is_empty() {
                tx.check_numeric("X value of a table point must be a number.")?;
                ty.check_numeric("Y value of a table point must be a number.")?;
                table.push((tx.as_d(), tx.start, ty.as_d()));
            }
            self.lit(b',')?;
        }
        if x.is_empty() {
            return Ok(Ev::Empty);
        }
        let xv = x.as_d();
        let mut result = None;
        for w in table.windows(2) {
            let ((x0, x0_pos, y0), (x1, _, y1)) = (w[0], w[1]);
            if x0 > x1 {
                return err(x0_pos, "X coordinates of the table must be increasing");
            }
            if result.is_none() && xv >= x0 && xv <= x1 {
                result = Some(if xv == x0 {
                    y0
                } else if xv == x1 {
                    y1
                } else if (x0 - x1).abs() < EPSILON {
                    0.5 * (y0 + y1)
                } else {
                    let t = (xv - x0) / (x1 - x0);
                    (1.0 - t) * y0 + t * y1
                });
            }
        }
        if result.is_none() {
            // Clamp x into the table range with EPSILON.
            if let (Some(first), Some(last)) = (table.first(), table.last()) {
                if xv > first.0 - EPSILON && xv < first.0 {
                    result = Some(first.2);
                } else if xv > last.0 && xv < last.0 + EPSILON {
                    result = Some(last.2);
                }
            }
        }
        match result {
            Some(y) => Ok(Ev::Double(y)),
            None => x.fail("Interpolation value is outside the table range"),
        }
    }
}

/// A vector element as a value of the vector's type.
fn element_value(ty: ValueType, v: &Value) -> Value {
    match (ty, v) {
        (ValueType::Float, Value::Int(i)) => Value::Float(*i as f64),
        (_, v) => v.clone(),
    }
}
