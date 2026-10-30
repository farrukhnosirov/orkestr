//! The spoken calculator (`Calculate`): «сколько будет двадцать пять умножить
//! на четыре», "what is 15 percent of 200", "2+2*3", "корень из 144" →
//! an arithmetic expression (+ − × ÷ ^ %, parentheses, sqrt) evaluated by a
//! small recursive-descent parser. Local, instant, no AI.

use crate::error::{ComputerError, ComputerResult};
use crate::text::parse_number;

/// Spoken operator phrases (longest first) → expression text.
const OPS: &[(&str, &str)] = &[
    ("процентов от", " %of "),
    ("процента от", " %of "),
    ("процент от", " %of "),
    ("percent of", " %of "),
    ("умножить на", " * "),
    ("умножь на", " * "),
    ("умноженное на", " * "),
    ("multiplied by", " * "),
    ("разделить на", " / "),
    ("раздели на", " / "),
    ("делить на", " / "),
    ("поделить на", " / "),
    ("деленное на", " / "),
    ("divided by", " / "),
    ("в степени", " ^ "),
    ("to the power of", " ^ "),
    ("в квадрате", " ^ 2 "),
    ("в кубе", " ^ 3 "),
    ("squared", " ^ 2 "),
    ("cubed", " ^ 3 "),
    ("квадратный корень из", " sqrt "),
    ("корень из", " sqrt "),
    ("square root of", " sqrt "),
    ("открыть скобку", " ( "),
    ("закрыть скобку", " ) "),
    ("плюс", " + "),
    ("plus", " + "),
    ("минус", " - "),
    ("minus", " - "),
    ("times", " * "),
    ("x", " * "),
    ("х", " * "),
    ("на", " * "),
    ("over", " / "),
    ("целых", " . "),
    ("point", " . "),
];

/// Words dropped before parsing.
const NOISE: &[&str] = &[
    "сколько",
    "будет",
    "посчитай",
    "вычисли",
    "подсчитай",
    "калькулятор",
    "равно",
    "равняется",
    "what",
    "is",
    "calculate",
    "compute",
    "how",
    "much",
    "equals",
    "the",
    "of",
    "посчитать",
    "реши",
    "пример",
];

/// Converts spoken text to an expression string.
#[must_use]
pub fn spoken_to_expression(text: &str) -> String {
    let mut s = format!(" {} ", text.to_lowercase().replace('ё', "е").replace(',', "."));
    for (from, to) in [("×", "*"), ("÷", "/"), ("−", "-"), ("—", "-")] {
        s = s.replace(from, to);
    }
    for (phrase, op) in OPS {
        s = s.replace(&format!(" {phrase} "), op);
    }
    // keep operators apart from numbers
    let mut spaced = String::new();
    for c in s.chars() {
        if "+-*/^()%".contains(c) {
            spaced.push(' ');
            spaced.push(c);
            spaced.push(' ');
        } else {
            spaced.push(c);
        }
    }
    let spaced = spaced.replace(" % of ", " %of ");
    // runs of number words → digits
    let mut out: Vec<String> = Vec::new();
    let mut run: Vec<&str> = Vec::new();
    let flush = |run: &mut Vec<&str>, out: &mut Vec<String>| {
        if !run.is_empty() {
            let joined = run.join(" ");
            out.push(parse_number(&joined).map_or(joined, |n| n.to_string()));
            run.clear();
        }
    };
    for w in spaced.split_whitespace() {
        if NOISE.contains(&w) {
            continue;
        }
        let w = w.trim_matches(|c: char| c == '?' || c == '!');
        if w.is_empty() {
            continue;
        }
        let is_num_word = parse_number(w).is_some() && !w.chars().any(|c| c.is_ascii_digit());
        if is_num_word {
            run.push(w);
        } else {
            flush(&mut run, &mut out);
            out.push(w.to_owned());
        }
    }
    flush(&mut run, &mut out);
    // "3 . 5" → "3.5"
    out.join(" ").replace(" . ", ".")
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Op(char),
    PercentOf,
    Sqrt,
    LParen,
    RParen,
}

fn lex(s: &str) -> ComputerResult<Vec<Tok>> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || c == '.' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            let t: String = chars[start..i].iter().collect();
            out.push(Tok::Num(t.parse().map_err(|_| ComputerError::Invalid(format!("bad number “{t}”")))?));
        } else if s[s.char_indices().nth(i).map_or(0, |(b, _)| b)..].starts_with("%of") {
            out.push(Tok::PercentOf);
            i += 3;
        } else if s[s.char_indices().nth(i).map_or(0, |(b, _)| b)..].starts_with("sqrt") {
            out.push(Tok::Sqrt);
            i += 4;
        } else if "+-*/^%".contains(c) {
            out.push(Tok::Op(c));
            i += 1;
        } else if c == '(' {
            out.push(Tok::LParen);
            i += 1;
        } else if c == ')' {
            out.push(Tok::RParen);
            i += 1;
        } else {
            let word: String = chars[i..].iter().take_while(|c| !c.is_whitespace()).collect();
            return Err(ComputerError::Invalid(format!("I can't calculate “{word}”")));
        }
    }
    Ok(out)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn expr(&mut self) -> ComputerResult<f64> {
        let mut v = self.term()?;
        while let Some(Tok::Op(c @ ('+' | '-'))) = self.peek().cloned() {
            self.pos += 1;
            let r = self.term()?;
            v = if c == '+' { v + r } else { v - r };
        }
        Ok(v)
    }
    fn term(&mut self) -> ComputerResult<f64> {
        let mut v = self.power()?;
        loop {
            match self.peek().cloned() {
                Some(Tok::Op(c @ ('*' | '/'))) => {
                    self.pos += 1;
                    let r = self.power()?;
                    if c == '/' && r == 0.0 {
                        return Err(ComputerError::Invalid("division by zero".into()));
                    }
                    v = if c == '*' { v * r } else { v / r };
                }
                Some(Tok::PercentOf) => {
                    self.pos += 1;
                    let r = self.power()?;
                    v = v / 100.0 * r;
                }
                _ => return Ok(v),
            }
        }
    }
    fn power(&mut self) -> ComputerResult<f64> {
        let b = self.unary()?;
        if self.peek() == Some(&Tok::Op('^')) {
            self.pos += 1;
            let e = self.power()?;
            return Ok(b.powf(e));
        }
        Ok(b)
    }
    fn unary(&mut self) -> ComputerResult<f64> {
        match self.peek() {
            Some(Tok::Op('-')) => {
                self.pos += 1;
                Ok(-self.unary()?)
            }
            Some(Tok::Op('+')) => {
                self.pos += 1;
                self.unary()
            }
            Some(Tok::Sqrt) => {
                self.pos += 1;
                let v = self.unary()?;
                if v < 0.0 {
                    return Err(ComputerError::Invalid("square root of a negative number".into()));
                }
                Ok(v.sqrt())
            }
            _ => self.atom(),
        }
    }
    fn atom(&mut self) -> ComputerResult<f64> {
        let mut v = match self.next() {
            Some(Tok::Num(n)) => n,
            Some(Tok::LParen) => {
                let v = self.expr()?;
                if self.next() != Some(Tok::RParen) {
                    return Err(ComputerError::Invalid("missing “)”".into()));
                }
                v
            }
            _ => return Err(ComputerError::Invalid("the expression is incomplete".into())),
        };
        // postfix percent: "15%" = 0.15
        if self.peek() == Some(&Tok::Op('%')) {
            self.pos += 1;
            v /= 100.0;
        }
        Ok(v)
    }
}

/// Evaluates an expression (digits / operators).
///
/// # Errors
/// `Invalid` for unknown words, bad syntax, division by zero.
pub fn eval(expr: &str) -> ComputerResult<f64> {
    let toks = lex(expr)?;
    if toks.is_empty() {
        return Err(ComputerError::Invalid("nothing to calculate".into()));
    }
    let mut p = Parser { toks, pos: 0 };
    let v = p.expr()?;
    if p.pos != p.toks.len() {
        return Err(ComputerError::Invalid("the expression has extra parts".into()));
    }
    if !v.is_finite() {
        return Err(ComputerError::Invalid("the result is not a finite number".into()));
    }
    Ok(v)
}

/// Evaluates a spoken or written expression.
///
/// # Errors
/// As [`eval`].
pub fn eval_spoken(text: &str) -> ComputerResult<f64> {
    eval(&spoken_to_expression(text))
}

/// `4`, `2.5`, `0.333333` (≤ 6 decimals, no trailing zeros).
#[must_use]
pub fn format_number(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 && v.abs() < 1e15 {
        #[allow(clippy::cast_possible_truncation)]
        return format!("{}", v.round() as i64);
    }
    let s = format!("{v:.6}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_expressions() {
        assert_eq!(eval("2+2*3").unwrap(), 8.0);
        assert_eq!(eval("(2+2)*3").unwrap(), 12.0);
        assert_eq!(eval("2^10").unwrap(), 1024.0);
        assert_eq!(eval("-3 + 5").unwrap(), 2.0);
        assert_eq!(eval("15 %of 200").unwrap(), 30.0);
        assert_eq!(eval("sqrt 16 + 1").unwrap(), 5.0);
        assert!(eval("1/0").is_err());
        assert!(eval("2 +").is_err());
        assert!(eval("").is_err());
    }

    #[test]
    fn spoken_expressions() {
        let cases: &[(&str, f64)] = &[
            ("сколько будет два плюс два", 4.0),
            ("двадцать пять умножить на четыре", 100.0),
            ("сто разделить на восемь", 12.5),
            ("посчитай 15 процентов от 200", 30.0),
            ("корень из ста сорока четырех", 12.0),
            ("семь в квадрате", 49.0),
            ("2 в степени 8", 256.0),
            ("три целых пять умножить на два", 7.0),
            ("what is 12 times 12", 144.0),
            ("10 divided by 4", 2.5),
            ("сорок два минус восемнадцать", 24.0),
            ("2 × 3", 6.0),
            ("1,5 + 1,5", 3.0),
        ];
        for (t, want) in cases {
            let got = eval_spoken(t);
            assert_eq!(got.as_ref().ok(), Some(want), "{t} → {} → {got:?}", spoken_to_expression(t));
        }
        assert!(eval_spoken("сколько будет банан плюс два").is_err());
    }

    #[test]
    fn formats() {
        assert_eq!(format_number(4.0), "4");
        assert_eq!(format_number(12.5), "12.5");
        assert_eq!(format_number(1.0 / 3.0), "0.333333");
    }
}
