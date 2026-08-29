//! calc.rs — the command-line calculator and user variables.
//!
//! PURE module (no egui, no UI): a `CalcStore` of lazy named variables
//! (`name → expression string`, re-evaluated on every use), plus a small
//! recursive-descent evaluator for math expressions. Trig is in DEGREES.
//!
//! Everywhere the app accepts a number (prompt replies, dialog fields, the
//! 3D Factory command line) this evaluator is the fallback behind the plain
//! `f64::parse` fast path — so existing numeric behaviour is untouched and
//! expressions just work on top of it.

use std::collections::BTreeMap;
use std::fmt;

/// The user-visible calculator state. One per CadApp; persisted per drawing
/// via the SIMLUX sidecar (minus `ans`).
#[derive(Default, Clone)]
pub struct CalcStore {
    /// name → stored expression string (LAZY — evaluated on every use).
    vars: BTreeMap<String, String>,
}

impl CalcStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.vars.contains_key(name)
    }

    pub fn expr_of(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(String::as_str)
    }

    /// The last idle-prompt result, as a plain number string in the store.
    /// Excluded from persistence.
    pub fn set_ans(&mut self, v: f64) {
        self.vars.insert("ans".into(), format!("{v}"));
    }

    /// Everything that survives a save/reopen — every variable except `ans`.
    pub fn persist_map(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for (k, v) in &self.vars {
            if k != "ans" {
                out.insert(k.clone(), v.clone());
            }
        }
        out
    }

    /// Build the store from a loaded sidecar map. Invalid names are dropped
    /// (a hand-edited file cannot smuggle junk in), as is `ans` — it is a
    /// session value, never persisted.
    pub fn from_persist(map: BTreeMap<String, String>) -> Self {
        let mut vars = BTreeMap::new();
        for (k, v) in map {
            if k == "ans" || !valid_name(&k) {
                continue;
            }
            vars.insert(k, v);
        }
        CalcStore { vars }
    }

    /// Load variables from a `name = expression` text file (the app's
    /// `calc_vars.txt`). Missing/unreadable file → empty store, no error.
    pub fn load_from(path: Option<&std::path::Path>) -> Self {
        let Some(p) = path else { return Self::new() };
        let Ok(text) = std::fs::read_to_string(p) else { return Self::new() };
        let mut map = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                map.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        Self::from_persist(map)
    }

    /// Persist variables (minus `ans`) as a `name = expression` text file.
    pub fn save_to(&self, path: Option<&std::path::Path>) -> std::io::Result<()> {
        let Some(p) = path else { return Ok(()) };
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut s = String::from("# RUST_AutoRASM calculator variables (name = expression)\n");
        for (k, v) in self.persist_map() {
            s.push_str(&format!("{k} = {v}\n"));
        }
        std::fs::write(p, s)
    }
}

/// A name a variable may have: `[A-Za-z_][A-Za-z0-9_]*`, case-sensitive.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[derive(Debug, Clone, PartialEq)]
pub enum CalcError {
    /// Referenced a variable that does not exist.
    UnknownVar(String),
    /// Variable resolution looped — the Vec is the chain, e.g. `a → b → a`.
    Cycle(Vec<String>),
    /// Malformed input — the detail is a static reason string.
    Syntax(&'static str),
    /// Division (or modulo) by an exactly-zero divisor.
    DivZero,
    /// The result (or an intermediate) is NaN or infinite.
    NonFinite,
    /// Assignment into a SYSVAR name — system variables keep today's meaning.
    ReservedName(String),
}

impl fmt::Display for CalcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CalcError::UnknownVar(n) => write!(f, "unknown variable '{}'", n),
            CalcError::Cycle(c) => write!(f, "variable cycle: {}", c.join(" → ")),
            CalcError::Syntax(m) => write!(f, "syntax error: {m}"),
            CalcError::DivZero => write!(f, "division by zero"),
            CalcError::NonFinite => write!(f, "result is not a finite number"),
            CalcError::ReservedName(n) => write!(
                f,
                "'{n}' is a system variable name — variables cannot shadow a SYSVAR"
            ),
        }
    }
}

impl std::error::Error for CalcError {}

// ─────────────────────────────────────────────────────────────────────────────
// Lexer
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Op(char), // + - * / ^ %
    LParen,
    RParen,
    Comma,
}

struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Lexer { s: src.as_bytes(), i: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.i += 1;
        }
    }

    /// Tokenize the whole source. Returns Err(Syntax) on a bad character.
    fn run(mut self) -> Result<Vec<Tok>, CalcError> {
        let mut out = Vec::new();
        loop {
            self.skip_ws();
            let Some(c) = self.peek() else { break };
            match c {
                b'0'..=b'9' => out.push(Tok::Num(self.number()?)),
                b'.' => {
                    // `.5` — a leading-dot number, but a bare `.` is a syntax error.
                    if matches!(self.s.get(self.i + 1), Some(b'0'..=b'9')) {
                        out.push(Tok::Num(self.number()?));
                    } else {
                        return Err(CalcError::Syntax("unexpected '.'"));
                    }
                }
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => out.push(Tok::Ident(self.ident())),
                b'+' | b'-' | b'*' | b'/' | b'^' | b'%' => {
                    out.push(Tok::Op(c as char));
                    self.i += 1;
                }
                b'(' => { out.push(Tok::LParen); self.i += 1; }
                b')' => { out.push(Tok::RParen); self.i += 1; }
                b',' => { out.push(Tok::Comma); self.i += 1; }
                _ => return Err(CalcError::Syntax("unexpected character")),
            }
        }
        // Sentinel end-of-input — the parser matches it, never consumes it.
        out.push(Tok::Op('\0'));
        Ok(out)
    }
}

impl<'a> Lexer<'a> {
    fn number(&mut self) -> Result<f64, CalcError> {
        let start = self.i;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.i += 1;
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.i += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let save = self.i;
            self.i += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if matches!(self.peek(), Some(b'0'..=b'9')) {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.i += 1;
                }
            } else {
                // Not an exponent after all — `2e` is `2` then identifier `e`.
                self.i = save;
            }
        }
        let text = std::str::from_utf8(&self.s[start..self.i])
            .map_err(|_| CalcError::Syntax("bad number"))?;
        text.parse::<f64>()
            .map_err(|_| CalcError::Syntax("bad number"))
    }

    fn ident(&mut self) -> String {
        let start = self.i;
        while matches!(self.peek(), Some(b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')) {
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.i]).into_owned()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Parser (recursive descent)
//   precedence: unary > ^ > * / % > + -
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Num(f64),
    Var(String),
    Call(&'static str, Vec<Node>),
    Unary(char, Box<Node>),
    Bin(char, Box<Node>, Box<Node>),
}

const FUNCS: &[&str] = &[
    "sqrt", "abs", "round", "floor", "ceil", "min", "max",
    "sin", "cos", "tan", "asin", "acos", "atan", "atan2",
    "exp", "ln", "log10",
];

fn is_func(name: &str) -> Option<&'static str> {
    FUNCS.iter().find(|f| **f == name).copied()
}

/// Maximum expression nesting (parens, function args, unary chains). The
/// parser and evaluator recurse once per level, so hostile input — a pasted
/// megabyte of `((((…))))` or a sidecar variable carrying one — must error
/// cleanly instead of overflowing the thread stack.
const MAX_PARSE_DEPTH: usize = 256;

struct Parser {
    toks: Vec<Tok>,
    i: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.i]
    }

    /// Enter one nesting level; `Err` when the cap is crossed. The caller
    /// MUST balance every successful `enter` with a `leave`.
    fn enter(&mut self) -> Result<(), CalcError> {
        self.depth += 1;
        if self.depth > MAX_PARSE_DEPTH {
            return Err(CalcError::Syntax("too deeply nested"));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    fn next(&mut self) -> Tok {
        let t = self.toks[self.i].clone();
        if self.i < self.toks.len() - 1 {
            self.i += 1;
        }
        t
    }

    fn expr(&mut self) -> Result<Node, CalcError> {
        let mut lhs = self.term()?;
        loop {
            match self.peek() {
                Tok::Op('+') => {
                    self.next();
                    let rhs = self.term()?;
                    lhs = Node::Bin('+', Box::new(lhs), Box::new(rhs));
                }
                Tok::Op('-') => {
                    self.next();
                    let rhs = self.term()?;
                    lhs = Node::Bin('-', Box::new(lhs), Box::new(rhs));
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    fn term(&mut self) -> Result<Node, CalcError> {
        let mut lhs = self.power()?;
        loop {
            match self.peek() {
                Tok::Op('*') => {
                    self.next();
                    let rhs = self.power()?;
                    lhs = Node::Bin('*', Box::new(lhs), Box::new(rhs));
                }
                Tok::Op('/') => {
                    self.next();
                    let rhs = self.power()?;
                    lhs = Node::Bin('/', Box::new(lhs), Box::new(rhs));
                }
                Tok::Op('%') => {
                    self.next();
                    let rhs = self.power()?;
                    lhs = Node::Bin('%', Box::new(lhs), Box::new(rhs));
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Node, CalcError> {
        match self.peek() {
            Tok::Op('-') => {
                self.next();
                self.enter()?;
                let inner = self.unary();
                self.leave();
                Ok(Node::Unary('-', Box::new(inner?)))
            }
            Tok::Op('+') => {
                self.next();
                self.enter()?;
                let inner = self.unary();
                self.leave();
                inner
            }
            _ => self.atom(),
        }
    }

    /// `^` is right-associative (`2^3^2` = 512) and binds LOOSER than unary
    /// minus (`-2^2` = 4, per the settled precedence: unary > ^ > * / % > + -).
    fn power(&mut self) -> Result<Node, CalcError> {
        let base = self.unary()?;
        if matches!(self.peek(), Tok::Op('^')) {
            self.next();
            let exp = self.power()?;
            return Ok(Node::Bin('^', Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }

    fn atom(&mut self) -> Result<Node, CalcError> {
        match self.next() {
            Tok::Num(v) => Ok(Node::Num(v)),
            Tok::Ident(name) => {
                if matches!(self.peek(), Tok::LParen) {
                    self.next();
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Tok::RParen) {
                        loop {
                            self.enter()?;
                            let arg = self.expr();
                            self.leave();
                            args.push(arg?);
                            match self.next() {
                                Tok::Comma => continue,
                                Tok::RParen => break,
                                _ => return Err(CalcError::Syntax("expected ')' or ','")),
                            }
                        }
                    } else {
                        self.next(); // consume ')'
                    }
                    match is_func(&name) {
                        Some(f) => Ok(Node::Call(f, args)),
                        None => Err(CalcError::Syntax("unknown function")),
                    }
                } else {
                    Ok(Node::Var(name))
                }
            }
            Tok::LParen => {
                self.enter()?;
                let inner = self.expr();
                self.leave();
                let inner = inner?;
                match self.next() {
                    Tok::RParen => Ok(inner),
                    _ => Err(CalcError::Syntax("expected ')'")),
                }
            }
            Tok::Op(c) if c == '\0' => Err(CalcError::Syntax("unexpected end of input")),
            _ => Err(CalcError::Syntax("unexpected token")),
        }
    }
}

/// Parse `src` into an AST. The sentinel `\0` token is never an operator the
/// parser consumes, so end-of-input only shows up where an operand belongs.
fn parse(src: &str) -> Result<Node, CalcError> {
    let toks = Lexer::new(src).run()?;
    let mut p = Parser { toks, i: 0, depth: 0 };
    let node = p.expr()?;
    // Trailing input must not be silently ignored.
    match p.peek() {
        Tok::Op('\0') => Ok(node),
        _ => Err(CalcError::Syntax("unexpected input after expression")),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Evaluation
// ─────────────────────────────────────────────────────────────────────────────

fn check_finite(v: f64) -> Result<f64, CalcError> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(CalcError::NonFinite)
    }
}

/// One top-level `eval` resolves each variable at most once. Lazy semantics
/// are preserved (the cache is per-call — the NEXT eval sees changed
/// definitions), while a diamond-shaped dependency graph (`a=b+c`, `b=d+e`,
/// `c=d+e`, …) evaluates each variable once instead of exponentially.
type EvalCache = std::collections::HashMap<String, f64>;

/// How deep one eval may chase variable definitions. A linear chain of a
/// million definitions must error, not overflow the stack.
const MAX_RESOLUTION_DEPTH: usize = 256;

fn eval_node(
    store: &CalcStore,
    node: &Node,
    chain: &mut Vec<String>,
    cache: &mut EvalCache,
) -> Result<f64, CalcError> {
    match node {
        Node::Num(v) => check_finite(*v),
        Node::Var(name) => {
            if let Some(expr) = store.vars.get(name) {
                // Cycle detection: the name is already on the resolution
                // chain, so the chain from its first occurrence back to
                // itself IS the loop — `a → b → a`.
                if let Some(pos) = chain.iter().position(|n| n == name) {
                    let mut cyc: Vec<String> = chain[pos..].to_vec();
                    cyc.push(name.clone());
                    return Err(CalcError::Cycle(cyc));
                }
                if chain.len() >= MAX_RESOLUTION_DEPTH {
                    return Err(CalcError::Syntax("too many nested variable definitions"));
                }
                if let Some(v) = cache.get(name) {
                    return Ok(*v);
                }
                chain.push(name.clone());
                let r = match parse(expr) {
                    Ok(ast) => eval_node(store, &ast, chain, cache),
                    Err(e) => Err(e),
                };
                chain.pop();
                // Only successes are cached — a cycle or an error must not
                // mask a later, corrected definition.
                if let Ok(v) = r {
                    cache.insert(name.clone(), v);
                }
                r
            } else {
                match name.as_str() {
                    "pi" => Ok(std::f64::consts::PI),
                    "e" => Ok(std::f64::consts::E),
                    _ => Err(CalcError::UnknownVar(name.clone())),
                }
            }
        }
        Node::Call(f, args) => {
            let n = args.len();
            let want = match *f {
                "atan2" | "min" | "max" => 2,
                _ => 1,
            };
            if n != want {
                return Err(CalcError::Syntax(
                    match n {
                        0 => "missing argument",
                        _ => "wrong number of arguments",
                    },
                ));
            }
            let mut vals = Vec::with_capacity(n);
            for a in args {
                vals.push(eval_node(store, a, chain, cache)?);
            }
            let r = match *f {
                "sqrt"   => vals[0].sqrt(),
                "abs"    => vals[0].abs(),
                "round"  => vals[0].round(),
                "floor"  => vals[0].floor(),
                "ceil"   => vals[0].ceil(),
                "min"    => vals[0].min(vals[1]),
                "max"    => vals[0].max(vals[1]),
                // Trig in DEGREES.
                "sin"    => (vals[0].to_radians()).sin(),
                "cos"    => (vals[0].to_radians()).cos(),
                "tan"    => (vals[0].to_radians()).tan(),
                "asin"   => vals[0].asin().to_degrees(),
                "acos"   => vals[0].acos().to_degrees(),
                "atan"   => vals[0].atan().to_degrees(),
                "atan2"  => vals[0].atan2(vals[1]).to_degrees(),
                "exp"    => vals[0].exp(),
                "ln"     => vals[0].ln(),
                "log10"  => vals[0].log10(),
                _ => unreachable!("call node only built for known functions"),
            };
            check_finite(r)
        }
        Node::Unary(op, inner) => {
            let v = eval_node(store, inner, chain, cache)?;
            match op {
                '-' => check_finite(-v),
                '+' => check_finite(v),
                _ => unreachable!("unary node only built for + and -"),
            }
        }
        Node::Bin(op, a, b) => {
            let x = eval_node(store, a, chain, cache)?;
            let y = eval_node(store, b, chain, cache)?;
            let r = match op {
                '+' => x + y,
                '-' => x - y,
                '*' => x * y,
                '/' => {
                    if y == 0.0 {
                        return Err(CalcError::DivZero);
                    }
                    x / y
                }
                '%' => {
                    if y == 0.0 {
                        return Err(CalcError::DivZero);
                    }
                    x % y
                }
                '^' => x.powf(y),
                _ => unreachable!("binop node only built for + - * / % ^"),
            };
            check_finite(r)
        }
    }
}

/// Evaluate `src` against `store`. Plain numbers are a fast path the caller
/// handles before this; here we lex → parse → evaluate.
pub fn eval(store: &CalcStore, src: &str) -> Result<f64, CalcError> {
    let ast = parse(src)?;
    let mut chain = Vec::new();
    let mut cache = EvalCache::new();
    eval_node(store, &ast, &mut chain, &mut cache)
}

/// A cheap gate: does this input plausibly want the calculator?
///
/// True when it contains an operator / paren / comma, names a known function,
/// a constant, `ans`, or an already-defined variable. Plain text and bare
/// words (unknown commands) are never touched.
pub fn looks_like_expr(store: &CalcStore, s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }
    if t.bytes().any(|b| matches!(b, b'+' | b'-' | b'*' | b'/' | b'^' | b'%' | b'(' | b')' | b','))
    {
        return true;
    }
    if t == "pi" || t == "e" || t == "ans" || store.vars.contains_key(t) {
        return true;
    }
    FUNCS.iter().any(|f| t.starts_with(f))
}

/// A STRICTER gate for contexts where a bare word must keep its other
/// meaning: expression-shaped ONLY (operator / paren / comma / function
/// prefix). Unlike [`looks_like_expr`] it never claims a bare defined name
/// or a bare constant — so at a 3D modify prompt, `e` (erase), `r`
/// (reference), `c` (copy) and `pi` still mean what they mean, while
/// `r*2` and `2+3` still evaluate.
pub fn looks_like_expr_token(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }
    if t.bytes().any(|b| matches!(b, b'+' | b'-' | b'*' | b'/' | b'^' | b'%' | b'(' | b')' | b','))
    {
        return true;
    }
    FUNCS.iter().any(|f| t.starts_with(f))
}

/// Assign `name = expr` if `src` is an assignment. Returns:
/// - `Ok(None)` — not an assignment at all.
/// - `Ok(Some((name, Some(v))))` — stored AND evaluable now; `v` is for echo.
/// - `Ok(Some((name, None)))` — stored, but not evaluable yet (unknown vars
///   are fine in a LAZY store — the error surfaces at use, not definition).
///
/// `Err` — malformed assignment or a SYSVAR-name collision; NOTHING stored.
pub fn try_assign(
    store: &mut CalcStore,
    src: &str,
) -> Result<Option<(String, Option<f64>)>, CalcError> {
    let Some(eq) = src.find('=') else {
        return Ok(None);
    };
    let name = src[..eq].trim();
    if name.is_empty() {
        return Err(CalcError::Syntax("missing variable name"));
    }
    if !valid_name(name) {
        return Err(CalcError::Syntax("invalid variable name"));
    }
    if crate::varreg::find(name).is_some() {
        return Err(CalcError::ReservedName(name.to_string()));
    }
    let rhs = src[eq + 1..].trim();
    if rhs.is_empty() {
        return Err(CalcError::Syntax("missing expression after '='"));
    }
    // Syntax-check NOW (so `x=2+` fails at definition), but evaluate lazily:
    // `h=w*2` with `w` undefined must still store, per the lazy contract.
    let ast = parse(rhs)?;
    let value = {
        let mut chain = Vec::new();
        let mut cache = EvalCache::new();
        eval_node(store, &ast, &mut chain, &mut cache).ok()
    };
    store.vars.insert(name.to_string(), rhs.to_string());
    Ok(Some((name.to_string(), value)))
}

/// Format a number the way the command line echoes it: `14` not `14.0`, and
/// `0.6` not `0.5999999999999999` (the shortest fixed-6 rendering).
pub fn fmt_value(v: f64) -> String {
    if v == 0.0 {
        return "0".to_string();
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{v:.0}");
    }
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s == "-0" { "0".to_string() } else { s }
}

/// Parse a DragValue's typed text: plain number first, then the calculator
/// (so `2+3` and `x*2` commit in a spinner field). `None` = leave unchanged.
pub fn parse_drag(store: &CalcStore, s: &str) -> Option<f64> {
    let t = s.trim();
    if let Ok(v) = t.parse::<f64>() {
        return Some(v);
    }
    eval(store, t).ok()
}

/// Like [`parse_drag`] but for integer spinner fields: the value must come
/// out whole — no silent rounding of `2.5*2` into something else.
pub fn parse_drag_int(store: &CalcStore, s: &str, min: i64, max: i64) -> Option<i64> {
    let v = parse_drag(store, s)?;
    if !v.is_finite() || v.fract() != 0.0 || v < min as f64 || v > max as f64 {
        return None;
    }
    Some(v as i64)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn e(store: &CalcStore, s: &str) -> f64 {
        eval(store, s).unwrap_or_else(|e| panic!("eval({s:?}) failed: {e}"))
    }

    #[test]
    fn arithmetic_and_precedence() {
        let s = CalcStore::new();
        assert_eq!(e(&s, "2+3*4"), 14.0);
        assert_eq!(e(&s, "(2+3)*4"), 20.0);
        assert_eq!(e(&s, "10-2-3"), 5.0);
        assert_eq!(e(&s, "2*3%4"), 2.0);
        assert_eq!(e(&s, "10/4"), 2.5);
        assert_eq!(e(&s, "2^3^2"), 512.0); // right-assoc
        assert_eq!(e(&s, "-2^2"), 4.0);    // unary binds tighter than ^
        assert_eq!(e(&s, "2^-2"), 0.25);
        assert_eq!(e(&s, "1+-2"), -1.0);
        assert_eq!(e(&s, "-(-3)"), 3.0);
        assert_eq!(e(&s, "+5"), 5.0);
        assert_eq!(e(&s, "2e3"), 2000.0);
        assert_eq!(e(&s, ".5+.5"), 1.0);
        assert_eq!(e(&s, " 2 + 3 "), 5.0); // internal spaces tolerated (paste)
    }

    #[test]
    fn constants_and_functions() {
        let s = CalcStore::new();
        assert!((e(&s, "pi") - std::f64::consts::PI).abs() < 1e-12);
        assert!((e(&s, "e") - std::f64::consts::E).abs() < 1e-12);
        assert_eq!(e(&s, "sqrt(16)+2^3"), 12.0);
        assert_eq!(e(&s, "abs(-3)"), 3.0);
        assert_eq!(e(&s, "round(2.5)"), 3.0);
        assert_eq!(e(&s, "floor(2.9)"), 2.0);
        assert_eq!(e(&s, "ceil(2.1)"), 3.0);
        assert_eq!(e(&s, "min(3,7)"), 3.0);
        assert_eq!(e(&s, "max(3,7)"), 7.0);
        assert_eq!(e(&s, "atan2(1,1)"), 45.0);
        assert_eq!(e(&s, "exp(ln(2))"), 2.0);
        assert_eq!(e(&s, "log10(1000)"), 3.0);
        // Trig in degrees.
        assert!((e(&s, "sin(30)") - 0.5).abs() < 1e-12);
        assert!((e(&s, "cos(60)") - 0.5).abs() < 1e-12);
        assert!((e(&s, "tan(45)") - 1.0).abs() < 1e-12);
        assert!((e(&s, "asin(0.5)") - 30.0).abs() < 1e-12);
        assert!((e(&s, "acos(0.5)") - 60.0).abs() < 1e-12);
        assert!((e(&s, "atan(1)") - 45.0).abs() < 1e-12);
        assert!((e(&s, "sqrt(sin(30))") - 0.5f64.sqrt()).abs() < 1e-12);
    }

    #[test]
    fn ans_and_lazy_variables() {
        let mut s = CalcStore::new();
        s.set_ans(14.0);
        assert_eq!(e(&s, "ans"), 14.0);
        s.set_ans(0.6);
        assert_eq!(e(&s, "ans/0.5"), 1.2);
        // Lazy: `h = w*2` stores the EXPRESSION; changing w changes h.
        let (name, val) = try_assign(&mut s, "w=5").unwrap().unwrap();
        assert_eq!(name, "w");
        assert_eq!(val, Some(5.0));
        let (name, val) = try_assign(&mut s, "h = w*2").unwrap().unwrap();
        assert_eq!(name, "h");
        assert_eq!(val, Some(10.0));
        assert_eq!(e(&s, "h"), 10.0);
        try_assign(&mut s, "w=7").unwrap();
        assert_eq!(e(&s, "h"), 14.0, "re-evaluated on every use");
    }

    #[test]
    fn lazy_definition_survives_unknown_vars() {
        let mut s = CalcStore::new();
        let (name, val) = try_assign(&mut s, "h=w*2").unwrap().unwrap();
        assert_eq!(name, "h");
        assert_eq!(val, None, "not evaluable yet — but still defined");
        assert_eq!(
            eval(&s, "h").unwrap_err(),
            CalcError::UnknownVar("w".into()),
            "the unknown-variable error surfaces at USE, not definition"
        );
        try_assign(&mut s, "w=3").unwrap();
        assert_eq!(e(&s, "h"), 6.0);
    }

    #[test]
    fn cycles_are_detected_and_named() {
        let mut s = CalcStore::new();
        try_assign(&mut s, "a=b").unwrap();
        try_assign(&mut s, "b=a").unwrap();
        assert_eq!(
            eval(&s, "a").unwrap_err(),
            CalcError::Cycle(vec!["a".into(), "b".into(), "a".into()])
        );
        assert_eq!(
            eval(&s, "a").unwrap_err().to_string(),
            "variable cycle: a → b → a"
        );
        // Self-reference.
        try_assign(&mut s, "x=x+1").unwrap();
        assert_eq!(
            eval(&s, "x").unwrap_err(),
            CalcError::Cycle(vec!["x".into(), "x".into()])
        );
    }

    #[test]
    fn malformed_input() {
        let s = CalcStore::new();
        for bad in ["2+", "*3", "(1", "1)", "2..3", "sqrt", "sqrt(1,2)", "min(1)",
                     "1 2", "2+*3", "sin()", "1e999", "foo(1)"] {
            assert!(eval(&s, bad).is_err(), "{bad:?} should fail");
        }
        // Error KIND checks.
        assert_eq!(eval(&s, "1/0").unwrap_err(), CalcError::DivZero);
        assert_eq!(eval(&s, "1%0").unwrap_err(), CalcError::DivZero);
        assert_eq!(eval(&s, "sqrt(-1)").unwrap_err(), CalcError::NonFinite);
        assert_eq!(eval(&s, "ln(-1)").unwrap_err(), CalcError::NonFinite);
        assert_eq!(eval(&s, "ln(0)").unwrap_err(), CalcError::NonFinite);
        assert_eq!(eval(&s, "nope").unwrap_err(), CalcError::UnknownVar("nope".into()));
        assert!(eval(&s, "1e999").is_err(), "literal overflow is non-finite");
    }

    #[test]
    fn assignments() {
        let mut s = CalcStore::new();
        // Not an assignment.
        assert_eq!(try_assign(&mut s, "hello").unwrap(), None);
        assert_eq!(try_assign(&mut s, "2+3").unwrap(), None);
        // Malformed.
        assert!(try_assign(&mut s, "=5").is_err());
        assert!(try_assign(&mut s, "x=").is_err());
        assert!(try_assign(&mut s, "2x=5").is_err());
        assert!(try_assign(&mut s, "a b=5").is_err());
        assert!(try_assign(&mut s, "x=2+").is_err());
        // SYSVAR names are rejected at definition.
        assert_eq!(
            try_assign(&mut s, "CrsHrS=5").unwrap_err(),
            CalcError::ReservedName("CrsHrS".into())
        );
        assert!(!s.contains("CrsHrS"), "nothing stored on rejection");
        // Valid, case-sensitive.
        assert!(try_assign(&mut s, "w=5").unwrap().is_some());
        assert!(try_assign(&mut s, "W=5").unwrap().is_some());
        assert!(s.contains("w") && s.contains("W"));
        assert_eq!(e(&s, "w"), 5.0);
        assert_eq!(e(&s, "W"), 5.0);
        // Underscore names.
        assert!(try_assign(&mut s, "_x2=1").unwrap().is_some());
    }

    #[test]
    fn looks_like_expr_gate() {
        let mut s = CalcStore::new();
        try_assign(&mut s, "w=5").unwrap();
        assert!(looks_like_expr(&s, "2+3"));
        assert!(looks_like_expr(&s, "sqrt(4)"));
        assert!(looks_like_expr(&s, "w"));
        assert!(looks_like_expr(&s, "pi"));
        assert!(looks_like_expr(&s, "ans"));
        assert!(looks_like_expr(&s, "-5"));
        assert!(!looks_like_expr(&s, "hello"));
        assert!(!looks_like_expr(&s, ""));
        assert!(!looks_like_expr(&s, "line"));
    }

    #[test]
    fn persistence_round_trip_keeps_expressions_verbatim() {
        let mut s = CalcStore::new();
        try_assign(&mut s, "w=2").unwrap();
        try_assign(&mut s, "h = w*2.5").unwrap();
        s.set_ans(9.0);
        let map = s.persist_map();
        assert_eq!(map.len(), 2, "ans is excluded");
        assert_eq!(map.get("h").map(String::as_str), Some("w*2.5"));
        // Round trip: expressions survive EXACTLY — no float drift.
        let s2 = CalcStore::from_persist(map);
        assert_eq!(e(&s2, "h"), 5.0);
        assert!(!s2.contains("ans"));
        // Junk names are dropped on load.
        let junk: BTreeMap<String, String> = [
            ("ok".into(), "1".into()),
            ("2bad".into(), "1".into()),
            ("ans".into(), "42".into()),
        ]
        .into_iter()
        .collect();
        let s3 = CalcStore::from_persist(junk);
        assert!(s3.contains("ok") && !s3.contains("2bad") && !s3.contains("ans"));
    }

    /// The app's `calc_vars.txt` file round-trip: `name = expression` lines,
    /// `ans` excluded, missing file → empty store, junk lines skipped.
    #[test]
    fn vars_file_round_trip() {
        let dir = std::env::temp_dir().join(format!("calc_vars_file_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("calc_vars.txt");
        let _ = std::fs::remove_file(&path);
        // Missing file → empty store.
        let empty = CalcStore::load_from(Some(&path));
        assert!(!empty.contains("w"));
        let mut s = CalcStore::new();
        try_assign(&mut s, "w=2").unwrap();
        try_assign(&mut s, "hh=w*2.5").unwrap();
        s.set_ans(9.0);
        s.save_to(Some(&path)).expect("write");
        let text = std::fs::read_to_string(&path).expect("read back");
        assert!(text.contains("hh = w*2.5"), "expression verbatim: {text}");
        assert!(!text.contains("ans"), "ans is never persisted");
        // Reload: expressions come back exact, and evaluate.
        let s2 = CalcStore::load_from(Some(&path));
        assert_eq!(e(&s2, "hh"), 5.0);
        assert!(!s2.contains("ans"));
        // Junk lines are skipped.
        std::fs::write(&path, "# comment\nw = 3\njunk line\n= 5\n").expect("junk file");
        let s3 = CalcStore::load_from(Some(&path));
        assert_eq!(e(&s3, "w"), 3.0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn fmt_value_is_clean() {
        assert_eq!(fmt_value(14.0), "14");
        assert_eq!(fmt_value(0.5), "0.5");
        assert_eq!(fmt_value(0.5999999999999999), "0.6");
        assert_eq!(fmt_value(-0.0), "0");
        assert_eq!(fmt_value(1e15), "1000000000000000");
        assert_eq!(fmt_value(12.0), "12");
        assert_eq!(fmt_value(2.25), "2.25");
    }

    #[test]
    fn drag_parsers() {
        let mut s = CalcStore::new();
        try_assign(&mut s, "x=3").unwrap();
        assert_eq!(parse_drag(&s, "5"), Some(5.0));
        assert_eq!(parse_drag(&s, "2+3"), Some(5.0));
        assert_eq!(parse_drag(&s, "x*2"), Some(6.0));
        assert_eq!(parse_drag(&s, "nope"), None);
        assert_eq!(parse_drag_int(&s, "4", 0, 100), Some(4));
        assert_eq!(parse_drag_int(&s, "2*2", 0, 100), Some(4));
        assert_eq!(parse_drag_int(&s, "2.5*1", 0, 100), None, "no silent rounding");
        assert_eq!(parse_drag_int(&s, "2.5", 0, 100), None);
        assert_eq!(parse_drag_int(&s, "500", 0, 100), None, "out of range");
    }

    /// Hostile input must error, not overflow the stack: megabyte-deep parens,
    /// unary chains, and definition chains all hit a clean depth error.
    #[test]
    fn deep_nesting_errors_instead_of_overflowing_the_stack() {
        let s = CalcStore::new();
        let deep = format!("{}1{}", "(".repeat(100_000), ")".repeat(100_000));
        assert_eq!(
            eval(&s, &deep).unwrap_err(),
            CalcError::Syntax("too deeply nested")
        );
        let unary = format!("{}1", "-".repeat(100_000));
        assert_eq!(
            eval(&s, &unary).unwrap_err(),
            CalcError::Syntax("too deeply nested")
        );
        // Sibling arguments do not nest, so a huge arg list must still fail
        // cleanly (arity) without recursing.
        let fargs = format!("max({},1)", "1,".repeat(100_000));
        assert!(eval(&s, &fargs).is_err(), "huge arg list errors");
        // Deep but legal input still evaluates.
        let ok = format!("{}1{}", "(".repeat(100), ")".repeat(100));
        assert_eq!(eval(&s, &ok).unwrap(), 1.0);
        // A long VARIABLE chain errors too — 100k definitions must not recurse.
        let mut st = CalcStore::new();
        for i in 0..100_000 {
            let name = format!("v{i}");
            let next = format!("v{}", i + 1);
            try_assign(&mut st, &format!("{name}={next}")).unwrap();
        }
        try_assign(&mut st, "v100000=1").unwrap();
        assert_eq!(
            eval(&st, "v0").unwrap_err(),
            CalcError::Syntax("too many nested variable definitions")
        );
    }

    /// A diamond-shaped dependency graph evaluates each variable once, not
    /// 2^depth times — lazy semantics are per-eval, so this must be fast.
    #[test]
    fn diamond_graphs_are_not_exponential() {
        let mut s = CalcStore::new();
        try_assign(&mut s, "x1=1+1").unwrap();
        for i in 2..=40 {
            let prev = i - 1;
            try_assign(&mut s, &format!("x{i}=x{prev}+x{prev}")).unwrap();
        }
        let t0 = std::time::Instant::now();
        assert_eq!(e(&s, "x40"), 2.0f64.powi(40));
        assert!(
            t0.elapsed().as_millis() < 2_000,
            "40-deep diamond took {:?} — re-evaluation is exponential",
            t0.elapsed()
        );
        // The next eval still sees updated definitions (lazy contract).
        try_assign(&mut s, "x1=3").unwrap();
        assert_eq!(e(&s, "x40"), 3.0 * 2.0f64.powi(39));
    }

}
