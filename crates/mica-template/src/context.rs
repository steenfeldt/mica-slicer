// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/PlaceholderParser.hpp :: PlaceholderParser::ContextData
// Source: prusaslicer@b028299c77 src/libslic3r/PlaceholderParser.cpp :: MyContext::resolve_output_symbol
// Changes: PrusaSlicer's typed ConfigOption zoo is reduced to four value types (bool, int, float, string),
// Changes: scalar or vector, with `Value::Nil` for undefined (nullable) values. The read-only config,
// Changes: the output config and the global dictionary live together in one `Context`; the std::mt19937
// Changes: generator is replaced by an injectable `RandomSource` (SplitMix64 by default).

//! Variables and the evaluation context.

use std::collections::HashMap;
use std::fmt;

use regex::Regex;

/// A single template value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// Undefined value of a nullable option (PrusaSlicer's "nil"). Reading it is an error;
    /// `is_nil()` tests for it.
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
}

impl Value {
    /// The type of this value, `None` for [`Value::Nil`].
    pub fn value_type(&self) -> Option<ValueType> {
        match self {
            Value::Nil => None,
            Value::Bool(_) => Some(ValueType::Bool),
            Value::Int(_) => Some(ValueType::Int),
            Value::Float(_) => Some(ValueType::Float),
            Value::Str(_) => Some(ValueType::Str),
        }
    }

    pub fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Integer value (floats are not converted).
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// Numeric value as a float (ints are converted).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// Element type of a variable. Assignments convert the right hand side to the variable's type,
/// exactly as PrusaSlicer converts to the type of the `ConfigOption` being written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueType {
    Bool,
    Int,
    Float,
    Str,
}

/// A variable: a scalar or a typed vector.
///
/// Vector elements must be [`Value::Nil`] or of the vector's type (an `Int` element in a `Float`
/// vector is accepted and read as a float). Templates address vectors as `name[index]`; an index past
/// the end reads element 0 (PrusaSlicer behaviour), writing past the end is an error.
#[derive(Clone, Debug, PartialEq)]
pub enum Var {
    /// A scalar. Its type is the type of the value; assigning to a `Nil` scalar adopts the type of
    /// the assigned value.
    Scalar(Value),
    Vector(ValueType, Vec<Value>),
}

impl Var {
    pub fn bool(v: bool) -> Var {
        Var::Scalar(Value::Bool(v))
    }
    pub fn int(v: i64) -> Var {
        Var::Scalar(Value::Int(v))
    }
    pub fn float(v: f64) -> Var {
        Var::Scalar(Value::Float(v))
    }
    pub fn string(v: impl Into<String>) -> Var {
        Var::Scalar(Value::Str(v.into()))
    }
    pub fn bools(v: impl IntoIterator<Item = bool>) -> Var {
        Var::Vector(ValueType::Bool, v.into_iter().map(Value::Bool).collect())
    }
    pub fn ints(v: impl IntoIterator<Item = i64>) -> Var {
        Var::Vector(ValueType::Int, v.into_iter().map(Value::Int).collect())
    }
    pub fn floats(v: impl IntoIterator<Item = f64>) -> Var {
        Var::Vector(ValueType::Float, v.into_iter().map(Value::Float).collect())
    }
    pub fn strings<S: Into<String>>(v: impl IntoIterator<Item = S>) -> Var {
        Var::Vector(
            ValueType::Str,
            v.into_iter().map(|s| Value::Str(s.into())).collect(),
        )
    }
    /// A vector of a nullable option; `None` elements are nil.
    pub fn nullable(ty: ValueType, v: impl IntoIterator<Item = Option<Value>>) -> Var {
        Var::Vector(ty, v.into_iter().map(|e| e.unwrap_or(Value::Nil)).collect())
    }

    pub fn is_vector(&self) -> bool {
        matches!(self, Var::Vector(..))
    }

    pub fn as_scalar(&self) -> Option<&Value> {
        match self {
            Var::Scalar(v) => Some(v),
            Var::Vector(..) => None,
        }
    }

    pub fn as_vector(&self) -> Option<&[Value]> {
        match self {
            Var::Vector(_, v) => Some(v),
            Var::Scalar(_) => None,
        }
    }

    /// Float vector elements given as ints are stored as floats.
    fn normalized(self) -> Var {
        match self {
            Var::Vector(ValueType::Float, values) => Var::Vector(
                ValueType::Float,
                values
                    .into_iter()
                    .map(|v| match v {
                        Value::Int(i) => Value::Float(i as f64),
                        v => v,
                    })
                    .collect(),
            ),
            v => v,
        }
    }
}

impl From<Value> for Var {
    fn from(v: Value) -> Var {
        Var::Scalar(v)
    }
}
impl From<bool> for Var {
    fn from(v: bool) -> Var {
        Var::bool(v)
    }
}
impl From<i64> for Var {
    fn from(v: i64) -> Var {
        Var::int(v)
    }
}
impl From<i32> for Var {
    fn from(v: i32) -> Var {
        Var::int(v.into())
    }
}
impl From<f64> for Var {
    fn from(v: f64) -> Var {
        Var::float(v)
    }
}
impl From<&str> for Var {
    fn from(v: &str) -> Var {
        Var::string(v)
    }
}
impl From<String> for Var {
    fn from(v: String) -> Var {
        Var::string(v)
    }
}

/// Source of random numbers for the `random(a, b)` template function.
pub trait RandomSource: Send {
    /// Next 64 uniformly distributed random bits.
    fn next_u64(&mut self) -> u64;
}

/// Default [`RandomSource`]: SplitMix64.
#[derive(Clone, Debug)]
pub struct SplitMix64(pub u64);

impl RandomSource for SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Where a variable lives. Lookup order is `Config`, `Output`, `Global`, then the per-call locals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    Config,
    Output,
    Global,
    Local,
}

/// Variables and state shared by successive [`process`](crate::process) calls.
///
/// It plays the role of PrusaSlicer's config + `config_outputs` + `ContextData` triple:
///
/// * **config variables** ([`set`](Context::set)) are read-only to templates — the print, printer and
///   filament settings plus per-call values such as `layer_z`, `max_layer_z`, `layer_num`,
///   `initial_tool`, `first_layer_print_min`, `zhop`, `extruded_volume`, … Assigning to them is an error.
/// * **output variables** ([`set_output`](Context::set_output)) are writable by templates and keep the
///   written value after the call. The G-code generator (PrusaSlicer's `PlaceholderParserIntegration`)
///   passes `position` (floats, x/y/z), `e_retracted`, `e_restart_extra` (floats per extruder) and,
///   with absolute E, `e_position` this way, then reads them back after running a custom G-code block
///   to learn where the script moved the head and how it changed retraction. Scripts must not resize them.
/// * **global variables** are created by templates with `global name = …` and persist across calls on
///   the same `Context`. `local name = …` variables live for one call only.
///
/// A name may exist in several scopes; config variables shadow output variables, which shadow globals.
pub struct Context {
    config: HashMap<String, Var>,
    outputs: HashMap<String, Var>,
    globals: HashMap<String, Var>,
    current_extruder_id: usize,
    rng: Box<dyn RandomSource>,
    regex_cache: HashMap<String, Regex>,
}

impl fmt::Debug for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Context")
            .field("config", &self.config)
            .field("outputs", &self.outputs)
            .field("globals", &self.globals)
            .field("current_extruder_id", &self.current_extruder_id)
            .finish_non_exhaustive()
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

impl Context {
    /// An empty context. The random generator is seeded from the clock, as PrusaSlicer does.
    pub fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x5EED);
        Self::with_seed(seed)
    }

    /// An empty context with a deterministic random generator.
    pub fn with_seed(seed: u64) -> Self {
        Self {
            config: HashMap::new(),
            outputs: HashMap::new(),
            globals: HashMap::new(),
            current_extruder_id: 0,
            rng: Box::new(SplitMix64(seed)),
            regex_cache: HashMap::new(),
        }
    }

    /// Sets a read-only (config) variable.
    pub fn set(&mut self, name: impl Into<String>, var: impl Into<Var>) {
        self.config.insert(name.into(), var.into().normalized());
    }

    /// Sets a writable output variable; templates may assign to it and the value stays here.
    pub fn set_output(&mut self, name: impl Into<String>, var: impl Into<Var>) {
        self.outputs.insert(name.into(), var.into().normalized());
    }

    /// Looks a variable up the way a template does (config, then outputs, then globals).
    pub fn get(&self, name: &str) -> Option<&Var> {
        self.config
            .get(name)
            .or_else(|| self.outputs.get(name))
            .or_else(|| self.globals.get(name))
    }

    /// An output variable (possibly modified by a template).
    pub fn output(&self, name: &str) -> Option<&Var> {
        self.outputs.get(name)
    }

    /// A global variable created by a template with `global name = …`.
    pub fn global(&self, name: &str) -> Option<&Var> {
        self.globals.get(name)
    }

    /// Removes a config or output variable (config first).
    pub fn remove(&mut self, name: &str) -> Option<Var> {
        self.config
            .remove(name)
            .or_else(|| self.outputs.remove(name))
    }

    /// Drops all template-created global variables.
    pub fn clear_globals(&mut self) {
        self.globals.clear();
    }

    /// Extruder used when a vector is expanded with the legacy `[name]` syntax (default 0).
    pub fn set_current_extruder(&mut self, id: usize) {
        self.current_extruder_id = id;
    }

    pub fn current_extruder(&self) -> usize {
        self.current_extruder_id
    }

    /// Replaces the random generator used by `random(a, b)`.
    pub fn set_random_source(&mut self, rng: Box<dyn RandomSource>) {
        self.rng = rng;
    }

    /// Re-seeds the default random generator.
    pub fn seed_random(&mut self, seed: u64) {
        self.rng = Box::new(SplitMix64(seed));
    }

    // ---- crate-internal access used by the interpreter ----

    pub(crate) fn resolve(&self, name: &str) -> Option<Scope> {
        if self.config.contains_key(name) {
            Some(Scope::Config)
        } else if self.outputs.contains_key(name) {
            Some(Scope::Output)
        } else if self.globals.contains_key(name) {
            Some(Scope::Global)
        } else {
            None
        }
    }

    pub(crate) fn config_var(&self, name: &str) -> Option<&Var> {
        self.config.get(name)
    }

    pub(crate) fn has_output(&self, name: &str) -> bool {
        self.outputs.contains_key(name)
    }

    pub(crate) fn scope_map(&self, scope: Scope) -> Option<&HashMap<String, Var>> {
        match scope {
            Scope::Config => Some(&self.config),
            Scope::Output => Some(&self.outputs),
            Scope::Global => Some(&self.globals),
            Scope::Local => None,
        }
    }

    pub(crate) fn scope_map_mut(&mut self, scope: Scope) -> Option<&mut HashMap<String, Var>> {
        match scope {
            Scope::Config => Some(&mut self.config),
            Scope::Output => Some(&mut self.outputs),
            Scope::Global => Some(&mut self.globals),
            Scope::Local => None,
        }
    }

    /// Uniform integer in `[lo, hi]`.
    pub(crate) fn random_int(&mut self, lo: i64, hi: i64) -> i64 {
        let span = (i128::from(hi) - i128::from(lo) + 1) as u128;
        let r = (u128::from(self.rng.next_u64()) * span) >> 64;
        (i128::from(lo) + r as i128) as i64
    }

    /// Uniform float in `[lo, hi)`.
    pub(crate) fn random_float(&mut self, lo: f64, hi: f64) -> f64 {
        let u = (self.rng.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64);
        lo + (hi - lo) * u
    }

    /// Compiles (and caches) a regular expression with full-match semantics.
    pub(crate) fn full_match_regex(&mut self, pattern: &str) -> Result<&Regex, regex::Error> {
        if !self.regex_cache.contains_key(pattern) {
            // Boost.Regex (Perl syntax) defaults: '.' matches newlines, '^'/'$' match at line breaks.
            let re = Regex::new(&format!("(?ms)\\A(?:{pattern})\\z"))?;
            self.regex_cache.insert(pattern.to_string(), re);
        }
        Ok(&self.regex_cache[pattern])
    }
}
