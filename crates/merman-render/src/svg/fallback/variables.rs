use cssparser::{Parser, ParserInput, ToCss, Token, TokenSerializationType};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const MAX_DEPTH: usize = 64;
const MAX_VALUE_BYTES: usize = 64 * 1024;
const MAX_WORK: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
struct Atom(String, TokenSerializationType);

#[derive(Clone, Debug)]
enum Part {
    Atom(Atom),
    Variable(String, Option<Value>),
    Block(Atom, Value, Atom),
}

#[derive(Clone, Debug)]
pub(super) struct Value(Vec<Part>);

#[derive(Clone, Debug, PartialEq, Default)]
pub(super) struct Computed(Vec<Atom>, usize);

pub(super) type Variables = HashMap<String, Option<Arc<Computed>>>;

#[derive(Clone, Debug)]
pub(super) enum Error {
    Invalid,
    Limit(usize, usize),
}

impl Value {
    pub(super) fn parse(value: &str) -> Result<Self, Error> {
        if value.len() > MAX_VALUE_BYTES {
            return Err(Error::Limit(value.len(), MAX_VALUE_BYTES));
        }
        let mut input = ParserInput::new(value);
        parse(&mut Parser::new(&mut input), 0, true)
    }

    pub(super) fn has_variables(&self) -> bool {
        self.0.iter().any(|part| match part {
            Part::Variable(..) => true,
            Part::Block(_, children, _) => children.has_variables(),
            _ => false,
        })
    }

    pub(super) fn keyword(&self) -> Option<&str> {
        let mut tokens = self.0.iter().filter(|part| {
            !matches!(
                part,
                Part::Atom(Atom(_, TokenSerializationType::WhiteSpace))
            )
        });
        let Part::Atom(Atom(text, TokenSerializationType::Ident)) = tokens.next()? else {
            return None;
        };
        tokens.next().is_none().then_some(text)
    }

    fn substitute(
        &self,
        lookup: &mut impl FnMut(&str, &mut usize) -> Result<Option<Arc<Computed>>, Error>,
        out: &mut Computed,
        work: &mut usize,
    ) -> Result<(), Error> {
        for part in &self.0 {
            match part {
                Part::Atom(atom) => out.push(atom, work)?,
                Part::Block(open, children, close) => {
                    out.push(open, work)?;
                    children.substitute(lookup, out, work)?;
                    out.push(close, work)?;
                }
                Part::Variable(name, fallback) => {
                    if let Some(value) = lookup(name, work)? {
                        for atom in &value.0 {
                            out.push(atom, work)?;
                        }
                    } else if let Some(fallback) = fallback {
                        fallback.substitute(lookup, out, work)?;
                    } else {
                        return Err(Error::Invalid);
                    }
                }
            }
        }
        Ok(())
    }
}

fn atom(token: Token<'_>) -> Atom {
    Atom(token.to_css_string(), token.serialization_type())
}

fn parse(input: &mut Parser<'_, '_>, depth: usize, declaration: bool) -> Result<Value, Error> {
    if depth > MAX_DEPTH {
        return Err(Error::Limit(depth, MAX_DEPTH));
    }
    let mut parts = Vec::new();
    while !input.is_exhausted() {
        let token = input
            .next_including_whitespace_and_comments()
            .map_err(|_| Error::Invalid)?
            .clone();
        let close = match &token {
            Token::Function(name) if name.eq_ignore_ascii_case("var") => {
                let part = input
                    .parse_nested_block(|input| {
                        let name = input.expect_ident_cloned()?.to_string();
                        if !name.starts_with("--") || name == "--" {
                            return Err(input.new_custom_error(Error::Invalid));
                        }
                        let fallback = if input.is_exhausted() {
                            None
                        } else {
                            input.expect_comma()?;
                            Some(
                                parse(input, depth + 1, true)
                                    .map_err(|e| input.new_custom_error(e))?,
                            )
                        };
                        Ok(Part::Variable(name, fallback))
                    })
                    .map_err(|error| match error.kind {
                        cssparser::ParseErrorKind::Custom(error) => error,
                        _ => Error::Invalid,
                    })?;
                parts.push(part);
                continue;
            }
            Token::Function(_) | Token::ParenthesisBlock => Some(Token::CloseParenthesis),
            Token::SquareBracketBlock => Some(Token::CloseSquareBracket),
            Token::CurlyBracketBlock => Some(Token::CloseCurlyBracket),
            Token::BadUrl(_)
            | Token::BadString(_)
            | Token::CloseParenthesis
            | Token::CloseSquareBracket
            | Token::CloseCurlyBracket => return Err(Error::Invalid),
            Token::Semicolon | Token::Delim('!') if declaration => return Err(Error::Invalid),
            Token::Comment(_) => continue,
            _ => None,
        };
        if let Some(close) = close {
            let children = input
                .parse_nested_block(|input| {
                    parse(input, depth + 1, false).map_err(|e| input.new_custom_error(e))
                })
                .map_err(|error| match error.kind {
                    cssparser::ParseErrorKind::Custom(error) => error,
                    _ => Error::Invalid,
                })?;
            parts.push(Part::Block(atom(token), children, atom(close)));
        } else {
            parts.push(Part::Atom(atom(token)));
        }
    }
    Ok(Value(parts))
}

impl Computed {
    fn push(&mut self, atom: &Atom, work: &mut usize) -> Result<(), Error> {
        charge(work, atom.0.len().max(1))?;
        self.1 = self.1.saturating_add(atom.0.len().max(1));
        if self.1 > MAX_VALUE_BYTES {
            return Err(Error::Limit(self.1, MAX_VALUE_BYTES));
        }
        self.0.push(atom.clone());
        Ok(())
    }

    fn css(&self) -> Result<String, Error> {
        let mut out = String::new();
        let mut previous = TokenSerializationType::Nothing;
        for Atom(text, kind) in &self.0 {
            // Substitution never joins tokens, e.g. var(--number)px is not a dimension.
            let separator = if previous.needs_separator_when_before(*kind) {
                "/**/"
            } else {
                ""
            };
            let size = out
                .len()
                .saturating_add(separator.len())
                .saturating_add(text.len());
            if size > MAX_VALUE_BYTES {
                return Err(Error::Limit(size, MAX_VALUE_BYTES));
            }
            out.push_str(separator);
            out.push_str(text);
            previous = *kind;
        }
        Ok(out.trim().to_string())
    }
}

pub(super) fn substitute(value: &Value, variables: &Variables) -> Result<(String, usize), Error> {
    let mut computed = Computed::default();
    let mut work = 0;
    value.substitute(
        &mut |name, _| Ok(variables.get(name).cloned().flatten()),
        &mut computed,
        &mut work,
    )?;
    Ok((computed.css()?, work))
}

fn charge(work: &mut usize, amount: usize) -> Result<(), Error> {
    *work = work.saturating_add(amount);
    if *work > MAX_WORK {
        return Err(Error::Limit(*work, MAX_WORK));
    }
    Ok(())
}

pub(super) fn compute(
    values: HashMap<String, Vec<(usize, String)>>,
    inherited: &Variables,
) -> Result<(Variables, usize), Error> {
    let mut work = 0;
    for name in inherited.keys() {
        charge(&mut work, name.len().saturating_add(1))?;
    }
    let mut resolver = Resolver {
        pending: HashMap::new(),
        values: inherited.clone(),
        stack: Vec::new(),
        cyclic: HashSet::new(),
        inherited,
    };
    for (name, candidates) in values {
        let mut parsed = Vec::new();
        for (tier, text) in candidates {
            charge(
                &mut work,
                name.len().saturating_add(text.len()).saturating_add(1),
            )?;
            parsed.push((tier, Value::parse(&text)?));
        }
        resolver.pending.insert(name, parsed);
    }
    let names: Vec<_> = resolver.pending.keys().cloned().collect();
    for name in names {
        resolver.resolve(&name, &mut work)?;
    }
    Ok((resolver.values, work))
}

struct Resolver<'a> {
    pending: HashMap<String, Vec<(usize, Value)>>,
    values: Variables,
    stack: Vec<String>,
    cyclic: HashSet<String>,
    inherited: &'a Variables,
}

impl Resolver<'_> {
    fn resolve(&mut self, name: &str, work: &mut usize) -> Result<(), Error> {
        charge(work, name.len().saturating_add(1))?;
        let Some(value) = self.pending.get(name) else {
            return Ok(());
        };
        if let Some(start) = self.stack.iter().position(|item| item == name) {
            self.cyclic.extend(self.stack[start..].iter().cloned());
            for name in &self.stack[start..] {
                self.values.insert(name.clone(), None);
            }
            return Ok(());
        }
        if self.stack.len() >= MAX_DEPTH {
            return Err(Error::Limit(self.stack.len() + 1, MAX_DEPTH));
        }
        let candidates = value.clone();
        self.stack.push(name.to_string());
        let mut computed = self.inherited.get(name).cloned().flatten();
        let mut ceiling = None;
        for (tier, value) in candidates {
            if ceiling.is_some_and(|ceiling| tier >= ceiling) {
                continue;
            }
            let mut result = Computed::default();
            let substitution = value.substitute(
                &mut |dependency, work| {
                    self.resolve(dependency, work)?;
                    Ok(self.values.get(dependency).cloned().flatten())
                },
                &mut result,
                work,
            );
            match substitution {
                Ok(()) if !self.cyclic.contains(name) => {
                    match result.css()?.to_ascii_lowercase().as_str() {
                        "revert-layer" => {
                            ceiling = Some(tier);
                            continue;
                        }
                        "inherit" | "unset" | "revert" => {}
                        "initial" => computed = None,
                        _ => computed = Some(Arc::new(result)),
                    }
                }
                Ok(()) | Err(Error::Invalid) => computed = None,
                Err(error) => return Err(error),
            }
            break;
        }
        self.values.insert(name.to_string(), computed);
        self.pending.remove(name);
        self.stack.pop();
        Ok(())
    }
}
