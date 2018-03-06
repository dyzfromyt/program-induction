//! (representation) Polymorphically-typed lambda calculus.
//!
//! # Examples
//!
//! ```
//! # #[macro_use] extern crate polytype;
//! # extern crate programinduction;
//! use programinduction::lambda::{Language, task_by_simple_evaluation};
//!
//! fn simple_evaluator(name: &str, inps: &[i32]) -> i32 {
//!     match name {
//!         "0" => 0,
//!         "1" => 1,
//!         "+" => inps[0] + inps[1],
//!         _ => unreachable!(),
//!     }
//! }
//!
//! # fn main() {
//! let dsl = Language::uniform(
//!     vec![
//!         ("0", tp!(int)),
//!         ("1", tp!(int)),
//!         ("+", arrow![tp!(int), tp!(int), tp!(int)]),
//!     ],
//! );
//!
//! // task: sum 1 with two numbers
//! let tp = arrow![tp!(int), tp!(int), tp!(int)];
//! let examples = vec![(vec![2, 5], 8), (vec![1, 2], 4)];
//! let task = task_by_simple_evaluation(&simple_evaluator, tp, &examples);
//!
//! // solution:
//! let expr = dsl.parse("(λ (+ (+ 1 $0)))").unwrap();
//! assert!((task.oracle)(&dsl, &expr).is_finite())
//! # }
//! ```

mod enumerator;
mod eval;
mod compression;
mod lisp;
pub use self::compression::Params;
pub use self::lisp::LispEvaluator;

use std::collections::{HashMap, VecDeque};
use std::f64;
use std::fmt::{self, Debug};
use std::rc::Rc;
use polytype::{Context, Type};
use super::{InferenceError, Representation, Task};
use super::ec::{ECFrontier, EC};

#[derive(Debug, Clone)]
pub struct ParseError {
    /// The location of the original string where parsing failed.
    pub location: usize,
    /// A message associated with the parse error.
    pub msg: &'static str,
}
impl ParseError {
    fn new(location: usize, msg: &'static str) -> Self {
        Self { location, msg }
    }
}
impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{} at index {}", self.msg, self.location)
    }
}
impl ::std::error::Error for ParseError {
    fn description(&self) -> &str {
        "could not parse expression"
    }
}

/// A Language is a registry for primitive and invented expressions in a polymorphically-typed
/// lambda calculus with corresponding production log-probabilities.
#[derive(Debug, Clone)]
pub struct Language {
    pub primitives: Vec<(String, Type, f64)>,
    pub invented: Vec<(Expression, Type, f64)>,
    pub variable_logprob: f64,
}
impl Language {
    /// A uniform distribution over primitives and invented expressions, as well as the abstraction
    /// operation.
    pub fn uniform(primitives: Vec<(&str, Type)>) -> Self {
        let primitives = primitives
            .into_iter()
            .map(|(s, t)| (String::from(s), t, 0f64))
            .collect();
        Language {
            primitives,
            invented: vec![],
            variable_logprob: 0f64,
        }
    }

    /// As with any [`Representation`], we must be able to infer the type of an [`Expression`]:
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate polytype;
    /// # extern crate programinduction;
    /// # fn main() {
    /// # use programinduction::lambda::{Language, Expression};
    /// let mut dsl = Language::uniform(
    ///     vec![
    ///         ("singleton", arrow![tp!(0), tp!(list(tp!(0)))]),
    ///         (">=", arrow![tp!(int), tp!(int), tp!(bool)]),
    ///         ("+", arrow![tp!(int), tp!(int), tp!(int)]),
    ///         ("0", tp!(int)),
    ///         ("1", tp!(int)),
    ///     ],
    /// );
    /// dsl.invent(
    ///     Expression::Application( // (+ 1)
    ///         Box::new(Expression::Primitive(2)),
    ///         Box::new(Expression::Primitive(4)),
    ///     ),
    ///     0f64,
    /// );
    /// let expr = dsl.parse("(singleton ((λ (>= $0 1)) (#(+ 1) 0)))").unwrap();
    /// assert_eq!(dsl.infer(&expr).unwrap(), tp!(list(tp!(bool))));
    /// # }
    /// ```
    ///
    /// [`Representation`]: ../trait.Representation.html
    /// [`Expression`]: ../enum.Expression.html
    pub fn infer(&self, expr: &Expression) -> Result<Type, InferenceError> {
        let mut ctx = Context::default();
        let env = VecDeque::new();
        let mut indices = HashMap::new();
        expr.infer(self, &mut ctx, &env, &mut indices)
    }

    /// Enumerate expressions for a request type (including log-probabilities and appropriately
    /// instantiated `Type`s):
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate polytype;
    /// # extern crate programinduction;
    /// # fn main() {
    /// use programinduction::lambda::{Expression, Language};
    ///
    /// let dsl = Language::uniform(
    ///     vec![
    ///         ("0", tp!(int)),
    ///         ("1", tp!(int)),
    ///         ("+", arrow![tp!(int), tp!(int), tp!(int)]),
    ///         (">", arrow![tp!(int), tp!(int), tp!(bool)]),
    ///     ],
    /// );
    /// let exprs: Vec<Expression> = dsl.enumerate(tp!(int))
    ///     .take(8)
    ///     .map(|(expr, _log_prior)| expr)
    ///     .collect();
    ///
    /// assert_eq!(
    ///     exprs,
    ///     vec![
    ///         Expression::Primitive(0),
    ///         Expression::Primitive(1),
    ///         dsl.parse("(+ 0 0)").unwrap(),
    ///         dsl.parse("(+ 0 1)").unwrap(),
    ///         dsl.parse("(+ 1 0)").unwrap(),
    ///         dsl.parse("(+ 1 1)").unwrap(),
    ///         dsl.parse("(+ 0 (+ 0 0))").unwrap(),
    ///         dsl.parse("(+ 0 (+ 0 1))").unwrap(),
    ///     ]
    /// );
    /// # }
    /// ```
    pub fn enumerate<'a>(&'a self, tp: Type) -> Box<Iterator<Item = (Expression, f64)> + 'a> {
        enumerator::new(self, tp)
    }

    /// Update production probabilities and induce new primitives, with the guarantee that any
    /// changes to the language yield net lower prior probability for expressions in the frontier.
    // TODO: add example
    pub fn compress<O: Sync>(
        &self,
        params: &Params,
        tasks: &[Task<Self, O>],
        frontiers: &[ECFrontier<Self>],
    ) -> Self {
        compression::induce(self, params, tasks, frontiers)
    }

    /// Evaluate an expressions based on an input/output pair.
    ///
    /// Inputs are given as a sequence representing sequentially applied arguments.
    ///
    /// This is not capable of dealing with first order functions. For now, if you need that kind
    /// of behavior, you must implement your own evaluator (e.g. call scheme and run appropriate
    /// code).
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use]
    /// # extern crate polytype;
    /// # extern crate programinduction;
    /// use programinduction::lambda::Language;
    ///
    /// fn evaluator(name: &str, inps: &[i32]) -> i32 {
    ///     match name {
    ///         "0" => 0,
    ///         "1" => 1,
    ///         "+" => inps[0] + inps[1],
    ///         _ => unreachable!(),
    ///     }
    /// }
    ///
    /// # fn main() {
    /// let dsl = Language::uniform(
    ///     vec![
    ///         ("0", tp!(int)),
    ///         ("1", tp!(int)),
    ///         ("+", arrow![tp!(int), tp!(int), tp!(int)]),
    ///     ],
    /// );
    /// let expr = dsl.parse("(λ (λ (+ (+ 1 $0) $1)))").unwrap();
    /// let inps = vec![2, 5];
    /// let evaluated = dsl.eval(&expr, &evaluator, &inps).unwrap();
    /// assert_eq!(evaluated, 8);
    /// # }
    /// ```
    pub fn eval<V, F>(&self, expr: &Expression, evaluator: &F, inps: &[V]) -> Option<V>
    where
        F: Fn(&str, &[V]) -> V,
        V: Clone + PartialEq + Debug,
    {
        eval::ReducedExpression::new(self, expr).eval_inps(evaluator, inps)
    }

    /// Get the log-likelihood of an expression normalized with other expressions with the given
    /// request type.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use]
    /// # extern crate polytype;
    /// # extern crate programinduction;
    /// # use programinduction::lambda::Language;
    /// # fn main() {
    /// let dsl = Language::uniform(
    ///     vec![
    ///         ("0", tp!(int)),
    ///         ("1", tp!(int)),
    ///         ("+", arrow![tp!(int), tp!(int), tp!(int)]),
    ///     ],
    /// );
    /// let req = arrow![tp!(int), tp!(int), tp!(int)];
    ///
    /// let expr = dsl.parse("(λ (λ (+ $0 $1)))").unwrap();
    /// assert_eq!(dsl.likelihood(&req, &expr), -5.545177444479561);
    ///
    /// let expr = dsl.parse("(λ (λ (+ (+ $0 1) $1)))").unwrap();
    /// assert_eq!(dsl.likelihood(&req, &expr), -8.317766166719343);
    /// # }
    /// ```
    pub fn likelihood(&self, request: &Type, expr: &Expression) -> f64 {
        enumerator::likelihood(self, request, expr)
    }

    /// Register a new invented expression. If it has a valid type, this will be `Ok(num)`.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[macro_use] extern crate polytype;
    /// # extern crate programinduction;
    /// # fn main() {
    /// # use programinduction::lambda::{Language, Expression};
    /// let mut dsl = Language::uniform(
    ///     vec![
    ///         ("0", tp!(int)),
    ///         ("1", tp!(int)),
    ///         ("+", arrow![tp!(int), tp!(int), tp!(int)]),
    ///     ],
    /// );
    /// let expr = dsl.parse("(+ 1)").unwrap();
    /// dsl.invent(expr.clone(), -0.5).unwrap();
    /// assert_eq!(dsl.invented(0), Some((&expr, &arrow![tp!(int), tp!(int)], -0.5)));
    /// # }
    /// ```
    pub fn invent(
        &mut self,
        expr: Expression,
        log_probability: f64,
    ) -> Result<usize, InferenceError> {
        let tp = self.infer(&expr)?;
        self.invented.push((expr, tp, log_probability));
        Ok(self.invented.len() - 1)
    }

    /// Remove all invented expressions by pulling out their underlying expressions.
    pub fn strip_invented(&self, expr: &Expression) -> Expression {
        expr.strip_invented(&self.invented)
    }
    /// The inverse of [`display`].
    ///
    /// Lambda expressions take the form `(lambda BODY)` or `(λ BODY)`, where BODY is an expression
    /// that may use a corresponding De Bruijn [`Index`].
    ///
    /// [`display`]: #method.display
    /// [`Index`]: enum.Expression.html#variant.Index
    pub fn parse(&self, inp: &str) -> Result<Expression, ParseError> {
        let s = inp.trim_left();
        let offset = inp.len() - s.len();
        Expression::parse(self, s, offset).and_then(move |(di, expr)| {
            if s[di..].chars().all(char::is_whitespace) {
                Ok(expr)
            } else {
                Err(ParseError::new(
                    offset + di,
                    "expected end of expression, found more tokens",
                ))
            }
        })
    }
    /// The inverse of [`parse`].
    ///
    /// [`parse`]: #method.parse
    pub fn display(&self, expr: &Expression) -> String {
        expr.show(self, false)
    }

    /// Like `display`, but in a format ready for lisp interpretation.
    pub fn lispify(&self, expr: &Expression, conversions: &HashMap<String, String>) -> String {
        expr.as_lisp(self, false, conversions, 0)
    }

    fn candidates(
        &self,
        request: &Type,
        ctx: &Context,
        env: &VecDeque<Type>,
    ) -> Vec<(f64, Expression, Type, Context)> {
        let prims = self.primitives
            .iter()
            .enumerate()
            .map(|(i, &(_, ref tp, p))| (p, tp, true, Expression::Primitive(i)));
        let invented = self.invented
            .iter()
            .enumerate()
            .map(|(i, &(_, ref tp, p))| (p, tp, true, Expression::Invented(i)));
        let indices = env.iter()
            .enumerate()
            .map(|(i, tp)| (self.variable_logprob, tp, false, Expression::Index(i)));
        let mut cands: Vec<_> = prims
            .chain(invented)
            .chain(indices)
            .filter_map(|(p, tp, instantiate, expr)| {
                let mut ctx = ctx.clone();
                let itp;
                let tp = if instantiate {
                    itp = tp.instantiate_indep(&mut ctx);
                    &itp
                } else {
                    tp
                };
                let ret = if let Type::Arrow(ref arrow) = *tp {
                    arrow.returns()
                } else {
                    &tp
                };
                ctx.unify(ret, request).ok().map(|_| {
                    let tp = tp.apply(&ctx);
                    (p, expr, tp, ctx)
                })
            })
            .collect();
        // update probabilities for variables (indices)
        let log_n_indexed = (cands
            .iter()
            .filter(|&&(_, ref expr, _, _)| match *expr {
                Expression::Index(_) => true,
                _ => false,
            })
            .count() as f64)
            .ln();
        for mut c in &mut cands {
            if let Expression::Index(_) = c.1 {
                c.0 -= log_n_indexed
            }
        }
        // normalize
        let p_largest = cands
            .iter()
            .map(|&(p, _, _, _)| p)
            .fold(f64::NEG_INFINITY, f64::max);
        let z = p_largest
            + cands
                .iter()
                .map(|&(p, _, _, _)| (p - p_largest).exp())
                .sum::<f64>()
                .ln();
        for mut c in &mut cands {
            c.0 -= z;
        }
        cands
    }
}
impl Representation for Language {
    type Expression = Expression;
    fn infer(&self, expr: &Self::Expression) -> Result<Type, InferenceError> {
        self.infer(expr)
    }
    fn display(&self, expr: &Self::Expression) -> String {
        self.display(expr)
    }
}
impl EC for Language {
    type Params = Params;
    fn enumerate<'a>(&'a self, tp: Type) -> Box<Iterator<Item = (Expression, f64)> + 'a> {
        self.enumerate(tp)
    }
    fn compress<O: Sync>(
        &self,
        params: &Self::Params,
        tasks: &[Task<Self, O>],
        frontiers: &[ECFrontier<Self>],
    ) -> Self {
        self.compress(params, tasks, frontiers)
    }
}

/// Expressions of lambda calculus, which only make sense with an accompanying [`Language`].
///
/// [`Language`]: struct.Language.html
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum Expression {
    /// The number associated with a primitive is used by the Language to identify the primitive.
    Primitive(usize),
    Application(Box<Expression>, Box<Expression>),
    Abstraction(Box<Expression>),
    /// De Bruijn index referring to the nth-nearest abstraction (0-indexed).
    /// For example, the identify function is `(λ $0)` or `Abstraction(Index(0))`.
    Index(usize),
    /// The number associated with an invented expression is used by the Language to identify the
    /// invention.
    Invented(usize),
}
impl Expression {
    fn infer(
        &self,
        dsl: &Language,
        mut ctx: &mut Context,
        env: &VecDeque<Type>,
        indices: &mut HashMap<usize, Type>,
    ) -> Result<Type, InferenceError> {
        match *self {
            Expression::Primitive(num) => if let Some(prim) = dsl.primitives.get(num as usize) {
                Ok(prim.1.instantiate_indep(ctx))
            } else {
                Err(InferenceError::BadExpression(format!(
                    "primitive does not exist: {}",
                    num
                )))
            },
            Expression::Application(ref f, ref x) => {
                let f_tp = f.infer(dsl, &mut ctx, env, indices)?;
                let x_tp = x.infer(dsl, &mut ctx, env, indices)?;
                let ret_tp = ctx.new_variable();
                ctx.unify(&f_tp, &arrow![x_tp, ret_tp.clone()])?;
                Ok(ret_tp.apply(ctx))
            }
            Expression::Abstraction(ref body) => {
                let arg_tp = ctx.new_variable();
                let mut env = env.clone();
                env.push_front(arg_tp.clone());
                let ret_tp = body.infer(dsl, &mut ctx, &env, indices)?;
                Ok(arrow![arg_tp, ret_tp].apply(ctx))
            }
            Expression::Index(i) => {
                if (i as usize) < env.len() {
                    Ok(env[i as usize].apply(ctx))
                } else {
                    Ok(indices
                        .entry(i - env.len())
                        .or_insert_with(|| ctx.new_variable())
                        .apply(ctx))
                }
            }
            Expression::Invented(num) => if let Some(inv) = dsl.invented.get(num as usize) {
                Ok(inv.1.instantiate_indep(ctx))
            } else {
                Err(InferenceError::BadExpression(format!(
                    "invention does not exist: {}",
                    num
                )))
            },
        }
    }
    fn strip_invented(&self, invented: &[(Expression, Type, f64)]) -> Expression {
        match *self {
            Expression::Application(ref f, ref x) => Expression::Application(
                Box::new(f.strip_invented(invented)),
                Box::new(x.strip_invented(invented)),
            ),
            Expression::Abstraction(ref body) => {
                Expression::Abstraction(Box::new(body.strip_invented(invented)))
            }
            Expression::Invented(num) => invented[num].0.strip_invented(invented),
            _ => self.clone(),
        }
    }
    fn shift(&mut self, offset: i64) -> bool {
        self.shift_internal(offset, 0)
    }
    fn shift_internal(&mut self, offset: i64, depth: usize) -> bool {
        match *self {
            Expression::Index(ref mut i) => {
                if *i < depth {
                    true
                } else if offset >= 0 {
                    *i += offset as usize;
                    true
                } else if let Some(ni) = i.checked_sub((-offset) as usize) {
                    *i = ni;
                    true
                } else {
                    false
                }
            }
            Expression::Application(ref mut f, ref mut x) => {
                f.shift_internal(offset, depth) && x.shift_internal(offset, depth)
            }
            Expression::Abstraction(ref mut body) => body.shift_internal(offset, depth + 1),
            _ => true,
        }
    }
    fn as_lisp(
        &self,
        dsl: &Language,
        is_function: bool,
        conversions: &HashMap<String, String>,
        depth: usize,
    ) -> String {
        match *self {
            Expression::Primitive(num) => {
                let name = &dsl.primitives[num as usize].0;
                conversions.get(name).unwrap_or(name).to_string()
            }
            Expression::Application(ref f, ref x) => {
                let f_lisp = f.as_lisp(dsl, true, conversions, depth);
                let x_lisp = x.as_lisp(dsl, false, conversions, depth);
                if is_function {
                    format!("{} {}", f_lisp, x_lisp)
                } else {
                    format!("({} {})", f_lisp, x_lisp)
                }
            }
            Expression::Abstraction(ref body) => {
                let var = (97 + depth as u8) as char;
                format!(
                    "(λ ({}) {})",
                    var,
                    body.as_lisp(dsl, false, conversions, depth + 1)
                )
            }
            Expression::Index(i) => {
                let var = (96 + (depth - i) as u8) as char;
                format!("{}", var)
            }
            Expression::Invented(num) => {
                dsl.invented[num as usize]
                    .0
                    .as_lisp(dsl, false, conversions, depth)
            }
        }
    }
    fn show(&self, dsl: &Language, is_function: bool) -> String {
        match *self {
            Expression::Primitive(num) => dsl.primitives[num as usize].0.clone(),
            Expression::Application(ref f, ref x) => if is_function {
                format!("{} {}", f.show(dsl, true), x.show(dsl, false))
            } else {
                format!("({} {})", f.show(dsl, true), x.show(dsl, false))
            },
            Expression::Abstraction(ref body) => format!("(λ {})", body.show(dsl, false)),
            Expression::Index(i) => format!("${}", i),
            Expression::Invented(num) => {
                format!("#{}", dsl.invented[num as usize].0.show(dsl, false))
            }
        }
    }
    /// inp must not have leading whitespace. Does not invent.
    fn parse(
        dsl: &Language,
        inp: &str,
        offset: usize, // for good error messages
    ) -> Result<(usize, Expression), ParseError> {
        let init: Option<Result<(usize, Expression), ParseError>> = None;

        let primitive = || {
            match inp.find(|c: char| c.is_whitespace() || c == ')') {
                None if !inp.is_empty() => Some(inp.len()),
                Some(next) if next > 0 => Some(next),
                _ => None,
            }.map(|di| {
                if let Some(num) = dsl.primitives
                    .iter()
                    .position(|&(ref name, _, _)| name == &inp[..di])
                {
                    Ok((di, Expression::Primitive(num)))
                } else {
                    Err(ParseError::new(offset + di, "unexpected end of expression"))
                }
            })
        };
        let application = || {
            inp.find('(')
                .and_then(|i| {
                    if inp[..i].chars().all(char::is_whitespace) {
                        Some(i + 1)
                    } else {
                        None
                    }
                })
                .map(|mut di| {
                    let mut items = VecDeque::new();
                    loop {
                        // parse expr
                        let (ndi, expr) = Expression::parse(dsl, &inp[di..], offset + di)?;
                        items.push_back(expr);
                        di += ndi;
                        // skip spaces
                        di += inp[di..].chars().take_while(|c| c.is_whitespace()).count();
                        // check if complete
                        match inp[di..].chars().nth(0) {
                            None => {
                                break Err(ParseError::new(offset + di, "incomplete application"))
                            }
                            Some(')') => {
                                di += 1;
                                break if let Some(init) = items.pop_front() {
                                    let app = items.into_iter().fold(init, |a, v| {
                                        Expression::Application(Box::new(a), Box::new(v))
                                    });
                                    Ok((di, app))
                                } else {
                                    Err(ParseError::new(offset + di, "empty application"))
                                };
                            }
                            _ => (),
                        }
                    }
                })
        };
        let abstraction = || {
            inp.find('(')
                .and_then(|i| {
                    if inp[..i].chars().all(char::is_whitespace) {
                        Some(i + 1)
                    } else {
                        None
                    }
                })
                .and_then(|di| match inp[di..].find(char::is_whitespace) {
                    Some(ndi) if &inp[di..di + ndi] == "lambda" || &inp[di..di + ndi] == "λ" => {
                        Some(di + ndi)
                    }
                    _ => None,
                })
                .map(|mut di| {
                    // skip spaces
                    di += inp[di..].chars().take_while(|c| c.is_whitespace()).count();
                    // parse body
                    let (ndi, body) = Expression::parse(dsl, &inp[di..], offset + di)?;
                    di += ndi;
                    // check if complete
                    inp[di..]
                        .chars()
                        .nth(0)
                        .and_then(|c| if c == ')' { Some(di + 1) } else { None })
                        .ok_or_else(|| ParseError::new(offset + di, "incomplete application"))
                        .map(|di| (di, Expression::Abstraction(Box::new(body))))
                })
        };
        let index = || {
            if inp.chars().nth(0) == Some('$') && inp.len() > 1 {
                inp[1..]
                    .find(|c: char| c.is_whitespace() || c == ')')
                    .and_then(|i| inp[1..1 + i].parse::<usize>().ok().map(|num| (1 + i, num)))
                    .map(|(di, num)| Ok((di, Expression::Index(num))))
            } else {
                None
            }
        };
        let invented = || {
            if inp.chars().take(2).collect::<String>() == "#(" {
                Some(1)
            } else {
                None
            }.map(|mut di| {
                let (ndi, expr) = Expression::parse(dsl, &inp[di..], offset + di)?;
                di += ndi;
                if let Some(num) = dsl.invented.iter().position(|&(ref x, _, _)| x == &expr) {
                    Ok((di, Expression::Invented(num)))
                } else {
                    Err(ParseError::new(
                        offset + di,
                        "invented expr is unfamiliar to context",
                    ))
                }
            })
        };
        // These parsers return None if the expr isn't applicable
        // or Some(Err(..)) if the expr applied but was invalid.
        // Ordering is intentional.
        init.or_else(abstraction)
            .or_else(application)
            .or_else(index)
            .or_else(invented)
            .or_else(primitive)
            .unwrap_or_else(|| {
                Err(ParseError::new(
                    offset,
                    "could not parse any expression variant",
                ))
            })
    }
}

/// Create a task based on evaluating lambda calculus expressions on test input/output pairs.
///
/// Here we let all tasks be represented by input/output pairs that are values in the space of
/// type `V`. For example, circuits may have `V` be just `bool`, whereas string editing may
/// have `V` be an enum featuring strings, chars, and natural numbers. All inputs, outputs, and
/// evaluated expressions must be representable by `V`.
///
/// An `evaluator` takes the name of a primitive and a vector of sequential inputs to the
/// expression (so an expression with unary type will have one input in a vec of size 1).
///
/// The resulting task is "all-or-nothing": the oracle returns either `0` if all examples are
/// correctly hit or `f64::NEG_INFINITY` otherwise.
///
/// # Examples
///
/// ```
/// # #[macro_use]
/// # extern crate polytype;
/// # extern crate programinduction;
/// use programinduction::lambda::{Language, task_by_simple_evaluation};
///
/// fn evaluator(name: &str, inps: &[i32]) -> i32 {
///     match name {
///         "0" => 0,
///         "1" => 1,
///         "+" => inps[0] + inps[1],
///         _ => unreachable!(),
///     }
/// }
///
/// # fn main() {
/// let examples = vec![(vec![2, 5], 8), (vec![1, 2], 4)];
/// let tp = arrow![tp!(int), tp!(int), tp!(int)];
/// let task = task_by_simple_evaluation(&evaluator, tp, &examples);
///
/// let dsl = Language::uniform(
///     vec![
///         ("0", tp!(int)),
///         ("1", tp!(int)),
///         ("+", arrow![tp!(int), tp!(int), tp!(int)]),
///     ],
/// );
/// let expr = dsl.parse("(λ (+ (+ 1 $0)))").unwrap();
/// assert!((task.oracle)(&dsl, &expr).is_finite())
/// # }
/// ```
pub fn task_by_simple_evaluation<'a, V, F>(
    simple_evaluator: &'a F,
    tp: Type,
    examples: &'a [(Vec<V>, V)],
) -> Task<'a, Language, &'a [(Vec<V>, V)]>
where
    V: PartialEq + Clone + Sync + Debug + 'a,
    F: Fn(&str, &[V]) -> V + Sync + 'a,
{
    let oracle = Box::new(move |dsl: &Language, expr: &Expression| {
        let success = examples.iter().all(|&(ref inps, ref out)| {
            if let Some(ref o) = dsl.eval(expr, simple_evaluator, inps) {
                o == out
            } else {
                false
            }
        });
        if success {
            0f64
        } else {
            f64::NEG_INFINITY
        }
    });
    Task {
        oracle,
        observation: examples,
        tp,
    }
}

#[derive(Debug, Clone)]
struct LinkedList<T: Clone>(Option<(T, Rc<LinkedList<T>>)>);
impl<T: Clone> LinkedList<T> {
    fn prepend(lst: &Rc<LinkedList<T>>, v: T) -> Rc<LinkedList<T>> {
        Rc::new(LinkedList(Some((v, lst.clone()))))
    }
    fn as_vecdeque(&self) -> VecDeque<T> {
        let mut lst: &Rc<LinkedList<T>>;
        let mut out = VecDeque::new();
        if let Some((ref v, ref nlst)) = self.0 {
            out.push_back(v.clone());
            lst = nlst;
            while let Some((ref v, ref nlst)) = lst.0 {
                out.push_back(v.clone());
                lst = nlst;
            }
        }
        out
    }
    fn len(&self) -> usize {
        let mut lst: &Rc<LinkedList<T>>;
        let mut n = 0;
        if let Some((_, ref nlst)) = self.0 {
            n += 1;
            lst = nlst;
            while let Some((_, ref nlst)) = lst.0 {
                n += 1;
                lst = nlst;
            }
        }
        n
    }
}
impl<T: Clone> Default for LinkedList<T> {
    fn default() -> Self {
        LinkedList(None)
    }
}
impl<T: Clone> ::std::ops::Index<usize> for LinkedList<T> {
    type Output = T;
    fn index(&self, i: usize) -> &Self::Output {
        let mut lst: &Rc<LinkedList<T>>;
        let mut n = 0;
        if let Some((ref v, ref nlst)) = self.0 {
            if i == n {
                return v;
            }
            n += 1;
            lst = nlst;
            while let Some((ref v, ref nlst)) = lst.0 {
                if i == n {
                    return v;
                }
                n += 1;
                lst = nlst;
            }
        }
        panic!("index out of bounds");
    }
}
