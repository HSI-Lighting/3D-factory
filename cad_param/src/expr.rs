//! Expressions & global variables — the "Mathematical parameters" tier.
//!
//! SolidWorks lets a dimension be a number, a global variable, or an equation
//! (`OverallHeight / 2`). This module is the equivalent: a tiny, self-contained
//! recursive-descent evaluator (numbers, `+ - * / ^`, unary minus, parentheses,
//! the constant `pi`, and the functions `sqrt sin cos tan abs`), plus a
//! [`VarTable`] of named variables whose values are themselves expressions and
//! may reference one another.
//!
//! It is pure Rust with no dependencies (like the rest of `cad_param`). The
//! solver still only ever sees resolved `f64` targets — this layer turns the
//! user's `=W/2 + 3` into that number before a constraint is built.

use std::collections::HashMap;

/// A named global variable whose value is an expression string (so `H = W/2`
/// works). Empty/blank expressions evaluate to 0.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Var {
    pub name: String,
    pub expr: String,
}

/// An ordered table of global variables (the "Equations" dialog).
#[derive(Clone, Debug, Default)]
pub struct VarTable {
    pub vars: Vec<Var>,
}

impl VarTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, name: &str, expr: &str) {
        if let Some(v) = self.vars.iter_mut().find(|v| v.name == name) {
            v.expr = expr.to_string();
        } else {
            self.vars.push(Var { name: name.to_string(), expr: expr.to_string() });
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.vars.retain(|v| v.name != name);
    }

    /// Resolve every variable to a concrete value, allowing variables to
    /// reference one another (in any order). Iterates to a fixed point;
    /// unresolved/cyclic variables are reported as an error.
    pub fn resolve(&self) -> Result<HashMap<String, f64>, String> {
        let mut env: HashMap<String, f64> = HashMap::new();
        // Up to N passes (N = var count) is enough for any acyclic dependency chain.
        for _ in 0..=self.vars.len() {
            let mut progressed = false;
            let mut all_done = true;
            for v in &self.vars {
                if env.contains_key(&v.name) {
                    continue;
                }
                match eval(&v.expr, &env) {
                    Ok(val) => {
                        env.insert(v.name.clone(), val);
                        progressed = true;
                    }
                    Err(_) => all_done = false, // probably depends on an unresolved var
                }
            }
            if all_done {
                return Ok(env);
            }
            if !progressed {
                let missing: Vec<&str> = self
                    .vars
                    .iter()
                    .filter(|v| !env.contains_key(&v.name))
                    .map(|v| v.name.as_str())
                    .collect();
                return Err(format!("unresolved or cyclic variables: {}", missing.join(", ")));
            }
        }
        Ok(env)
    }
}

/// Evaluate an expression string against an environment of named values.
/// Blank input is 0.
pub fn eval(src: &str, env: &HashMap<String, f64>) -> Result<f64, String> {
    let s = src.trim();
    if s.is_empty() {
        return Ok(0.0);
    }
    let toks = tokenize(s)?;
    let mut p = Parser { toks: &toks, pos: 0, env };
    let v = p.expr()?;
    if p.pos != p.toks.len() {
        return Err(format!("unexpected trailing input at token {}", p.pos));
    }
    Ok(v)
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    LParen,
    RParen,
}

fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        match c {
            ' ' | '\t' => i += 1,
            '+' => { out.push(Tok::Plus); i += 1; }
            '-' => { out.push(Tok::Minus); i += 1; }
            '*' => { out.push(Tok::Star); i += 1; }
            '/' => { out.push(Tok::Slash); i += 1; }
            '^' => { out.push(Tok::Caret); i += 1; }
            '(' => { out.push(Tok::LParen); i += 1; }
            ')' => { out.push(Tok::RParen); i += 1; }
            _ if c.is_ascii_digit() || c == '.' => {
                let start = i;
                while i < b.len() && {
                    let d = b[i] as char;
                    d.is_ascii_digit() || d == '.' || d == 'e' || d == 'E'
                        || ((d == '+' || d == '-') && i > start && matches!(b[i - 1] as char, 'e' | 'E'))
                } {
                    i += 1;
                }
                let num = &s[start..i];
                out.push(Tok::Num(num.parse::<f64>().map_err(|e| format!("bad number `{num}`: {e}"))?));
            }
            _ if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < b.len() && {
                    let d = b[i] as char;
                    d.is_ascii_alphanumeric() || d == '_'
                } {
                    i += 1;
                }
                out.push(Tok::Ident(s[start..i].to_string()));
            }
            _ => return Err(format!("unexpected character `{c}`")),
        }
    }
    Ok(out)
}

struct Parser<'a> {
    toks: &'a [Tok],
    pos: usize,
    env: &'a HashMap<String, f64>,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn expr(&mut self) -> Result<f64, String> {
        let mut v = self.term()?;
        while let Some(t) = self.peek() {
            match t {
                Tok::Plus => { self.pos += 1; v += self.term()?; }
                Tok::Minus => { self.pos += 1; v -= self.term()?; }
                _ => break,
            }
        }
        Ok(v)
    }

    fn term(&mut self) -> Result<f64, String> {
        let mut v = self.unary()?;
        while let Some(t) = self.peek() {
            match t {
                Tok::Star => { self.pos += 1; v *= self.unary()?; }
                Tok::Slash => {
                    self.pos += 1;
                    let d = self.unary()?;
                    if d == 0.0 {
                        return Err("division by zero".into());
                    }
                    v /= d;
                }
                _ => break,
            }
        }
        Ok(v)
    }

    // Unary minus binds LOOSER than exponentiation, so `-2^2` = −(2²) = −4.
    fn unary(&mut self) -> Result<f64, String> {
        match self.peek() {
            Some(Tok::Minus) => { self.pos += 1; Ok(-self.unary()?) }
            Some(Tok::Plus) => { self.pos += 1; self.unary() }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Result<f64, String> {
        let base = self.primary()?;
        if let Some(Tok::Caret) = self.peek() {
            self.pos += 1;
            let exp = self.unary()?; // right-associative; allows `2^-3`
            Ok(base.powf(exp))
        } else {
            Ok(base)
        }
    }

    fn primary(&mut self) -> Result<f64, String> {
        match self.peek().cloned() {
            Some(Tok::Num(n)) => { self.pos += 1; Ok(n) }
            Some(Tok::LParen) => {
                self.pos += 1;
                let v = self.expr()?;
                match self.peek() {
                    Some(Tok::RParen) => { self.pos += 1; Ok(v) }
                    _ => Err("expected `)`".into()),
                }
            }
            Some(Tok::Ident(name)) => {
                self.pos += 1;
                // function call?
                if let Some(Tok::LParen) = self.peek() {
                    self.pos += 1;
                    let arg = self.expr()?;
                    match self.peek() {
                        Some(Tok::RParen) => self.pos += 1,
                        _ => return Err(format!("expected `)` after {name}(…")),
                    }
                    return apply_fn(&name, arg);
                }
                // constant or variable
                match name.as_str() {
                    "pi" | "PI" | "Pi" => Ok(std::f64::consts::PI),
                    "e" | "E" => Ok(std::f64::consts::E),
                    _ => self
                        .env
                        .get(&name)
                        .copied()
                        .ok_or_else(|| format!("unknown variable `{name}`")),
                }
            }
            other => Err(format!("unexpected token {other:?}")),
        }
    }
}

fn apply_fn(name: &str, x: f64) -> Result<f64, String> {
    Ok(match name {
        "sqrt" => x.sqrt(),
        "sin" => x.sin(),
        "cos" => x.cos(),
        "tan" => x.tan(),
        "abs" => x.abs(),
        "rad" => x.to_radians(),
        "deg" => x.to_degrees(),
        _ => return Err(format!("unknown function `{name}`")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(s: &str) -> f64 {
        eval(s, &HashMap::new()).unwrap()
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert!((e("1 + 2 * 3") - 7.0).abs() < 1e-12);
        assert!((e("(1 + 2) * 3") - 9.0).abs() < 1e-12);
        assert!((e("-2 ^ 2") + 4.0).abs() < 1e-12); // unary binds looser than ^: -(2^2)
        assert!((e("2 ^ 3 ^ 2") - 512.0).abs() < 1e-9); // right-assoc: 2^(3^2)
        assert!((e("10 / 4") - 2.5).abs() < 1e-12);
    }

    #[test]
    fn functions_and_constants() {
        assert!((e("sqrt(16)") - 4.0).abs() < 1e-12);
        assert!((e("abs(-3.5)") - 3.5).abs() < 1e-12);
        assert!((e("sin(pi/2)") - 1.0).abs() < 1e-9);
        assert!((e("cos(rad(60))") - 0.5).abs() < 1e-9);
    }

    #[test]
    fn variables_reference_each_other() {
        let mut t = VarTable::new();
        t.set("W", "100");
        t.set("H", "W / 2");
        t.set("Area", "W * H");
        let env = t.resolve().unwrap();
        assert!((env["W"] - 100.0).abs() < 1e-9);
        assert!((env["H"] - 50.0).abs() < 1e-9);
        assert!((env["Area"] - 5000.0).abs() < 1e-9);
    }

    #[test]
    fn out_of_order_dependencies_resolve() {
        let mut t = VarTable::new();
        t.set("Area", "W * H"); // defined before its dependencies
        t.set("H", "W / 2");
        t.set("W", "100");
        let env = t.resolve().unwrap();
        assert!((env["Area"] - 5000.0).abs() < 1e-9);
    }

    #[test]
    fn cycles_are_reported() {
        let mut t = VarTable::new();
        t.set("A", "B + 1");
        t.set("B", "A + 1");
        assert!(t.resolve().is_err());
    }

    #[test]
    fn eval_with_env() {
        let mut env = HashMap::new();
        env.insert("x".to_string(), 3.0);
        assert!((eval("x^2 + 1", &env).unwrap() - 10.0).abs() < 1e-12);
    }
}
