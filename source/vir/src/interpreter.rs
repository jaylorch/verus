//! A Symbolic Interpreter for VIR
//!
//! Operates on VIR's SST representation
//!
//! Current target is supporting proof by computation
//! https://github.com/secure-foundations/verus/discussions/120

use crate::ast::{
    ArithOp, BinaryOp, BitwiseOp, Constant, Fun, InequalityOp, IntRange, SpannedTyped, Typ, TypX,
    Typs, UnaryOp, VirErr,
};
use crate::ast_util::{err_str, path_as_rust_name};
use crate::def::{SstMap, ARCH_SIZE_MIN_BITS};
use crate::sst::{Bnd, BndX, Exp, ExpX, Exps, UniqueIdent};
use air::ast::{Binder, BinderX, Binders};
use air::scope_map::ScopeMap;
use im::Vector;
use num_bigint::{BigInt, Sign};
use num_traits::identities::Zero;
use num_traits::{FromPrimitive, One, Signed, ToPrimitive};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

type Env = ScopeMap<UniqueIdent, Exp>;

struct State {
    depth: usize,
    env: Env,
    debug: bool,
    cache: HashMap<Fun, Vec<(Exps, Exp)>>,
}

impl State {
    fn insert_call(&mut self, f: &Fun, args: &Exps, result: &Exp) {
        match self.cache.get_mut(f) {
            None => {
                self.cache.insert(f.clone(), vec![(args.clone(), result.clone())]);
                ()
            }
            Some(prev_results) => prev_results.push((args.clone(), result.clone())),
        }
    }

    fn lookup_call(&self, f: &Fun, args: &Exps) -> Option<Exp> {
        for (prev_args, prev_result) in self.cache.get(f)? {
            match equal_exprs(prev_args, &args) {
                None => (),
                Some(b) => {
                    if b {
                        return Some(prev_result.clone());
                    }
                }
            }
        }
        None
    }
}

struct Ctx<'a> {
    fun_ssts: &'a SstMap,
    time_start: Instant,
    time_limit: Duration,
}

// TODO: Potential optimizations:
//  - Add support for function evaluation memoization
//  - Swap to CPS with enforced tail-call optimization to avoid exhausting the stack
//    - See crates musttail and with_locals

/*
fn print_exp(exp: Exp) {
    use ExpX::*;
    match &exp.x {
        Const(c) => match c {
            Constant::Bool(b) => print!("{}", b),
            Constant::Int(i) => print!("{}", i),
        }
        Var(id) => print!("{}", id),
        Call(fun, _, exps) => {
            print!("{}", fun.path.segments.last());
            for exp in &exps {
                print_exp(exp);
            }
            print!(")")
        }
        Unary(op, exp) => match op {
            UnaryOp::Not => { print!("!"); print_exp(exp) },
            UnaryOp::BitNot => { print!("!"); print_exp(exp) } ,
            UnaryOp::Trigger(..) => ()),
            UnaryOp::Clip(range) => print!("clip("); print_exp(exp); print!(")"),
        }
        UnaryOpr(op, exp) => {
            use crate::ast::UnaryOpr::*;
            match op {
                Box(_) => { print!("box("); print_exp(exp); print!(")") },
                Unbox(_) => { print!("unbox("); print_exp(exp); print!(")") },
                HasType(t) => { print_exp(exp); print!(".has_type({})", t),
                IsVariant { _datatype, variant } => { print_exp(exp); print!("is_type({})", variant) },
                TupleField {..} => panic!("TupleField should have been removed by ast_simplify!"),
                Field(field) => { print_exp(exp); print!(".{}", field.field) }
            }
        }
        Binary(op, e1, e2) => {
            use BinaryOp::*;
            use ArithOp::*;
            use InequalityOp::*;
            use BitwiseOp::*;
            print_exp(e1);
            match op {
                And => print!(" && "),
                Or  => print!(" || "),
                Xor => print!(" ^ "),
                Implies => print!(" ==> "),
                Eq(_) => print!(" == "),
                Ne => print!(" != "),
                Inequality(o) => match o {
                    Le => print!(" <= "),
                    Ge => print!(" >= "),
                    Lt => print!(" < "),
                    Gt => print!(" > "),
                }
                Arith(o, _) => match o {
                    Add => print!(" + "),
                    Sub => print!(" - "),
                    Mul => print!(" * "),
                    EuclideanDiv => print!(" / "),
                    EuclideanMod => print!(" % "),
                }
                Bitwise(o) => match o {
                    BitXor => print!(" ^ "),
                    BitAnd => print!(" & "),
                    BitOr  => print!(" | "),
                    Shr => print!(" >> "),
                    Shl => print!(" << "),
                }
            };
            print_exp(e2);
        },
        If(e1, e2, e3) => { print!("if "); print_exp(e1); print!(" then "); print_exp(e2); print!("
                                                                 {} else {}", e1, e2, e3),
        Bind(bnd, exp) => match bnd {
            Bnd::Let(bnds) => {
                print!("let ");
                for b in bnds.iter() {
                    print!("{} = {}, ", b.name, b.a);
                }
                print!("in {}", exp)
            },
            Bnd::Quant(..) | Bnd::Lambda(..) | Bnd::Choose(..) => print!("Unexpected: {:?}", exp.x),
        },
        Ctor(_path, id, bnds) => {
            print!("{}(", id);
            for b in bnds.iter() {
                print!("{}, ", b.a);
            }
            print!(")")
        }
        CallLambda(..) | VarLoc(..) | VarAt(..) | Loc(..) | Old(..) | WithTriggers(..) => print!("Unexpected: {:?}", self.x)

    }
}
*/

// Computes the syntactic equality of two types
// Some(b) means b is exp1 == exp2
// None means we can't tell
fn equal_typ(left: &Typ, right: &Typ) -> Option<bool> {
    use TypX::*;
    match (&**left, &**right) {
        (Bool, Bool) => Some(true),
        (Int(l), Int(r)) => Some(l == r),
        (Tuple(typs_l), Tuple(typs_r)) => equal_typs(typs_l, typs_r),
        (Lambda(formals_l, res_l), Lambda(formals_r, res_r)) => {
            Some(equal_typs(formals_l, formals_r)? && equal_typ(res_l, res_r)?)
        }
        (Datatype(path_l, typs_l), Datatype(path_r, typs_r)) => {
            Some(path_l == path_r && equal_typs(typs_l, typs_r)?)
        }
        (Boxed(l), Boxed(r)) => equal_typ(l, r),
        (TypParam(l), TypParam(r)) => {
            if l == r {
                Some(true)
            } else {
                None
            }
        }
        (TypeId, TypeId) => Some(true),
        (Air(l), Air(r)) => Some(l == r),
        _ => None,
    }
}

fn equal_typs(left: &Typs, right: &Typs) -> Option<bool> {
    let eq: Option<bool> = left
        .iter()
        .zip(right.iter())
        .fold(Some(true), |b, (t_l, t_r)| Some(b? && equal_typ(&*t_l, &*t_r)?));
    eq
}

// Computes the syntactic equality of two binders
// Some(b) means b is exp1 == exp2
// None means we can't tell
fn equal_bnd(left: &Bnd, right: &Bnd) -> Option<bool> {
    use BndX::*;
    // If we can't definitively establish equality, we conservatively return None
    let def_eq = |bnds_l, bnds_r| if equal_bnds_typ(bnds_l, bnds_r)? { Some(true) } else { None };
    match (&left.x, &right.x) {
        (Let(bnds_l), Let(bnds_r)) => {
            if equal_bnds_exp(bnds_l, bnds_r)? {
                Some(true)
            } else {
                None
            }
        }
        (Quant(q_l, bnds_l, _trigs_l), Quant(q_r, bnds_r, _trigs_r)) => {
            Some(q_l == q_r && def_eq(bnds_l, bnds_r)?)
        }
        (Lambda(bnds_l), Lambda(bnds_r)) => def_eq(bnds_l, bnds_r),
        (Choose(bnds_l, _trigs_l, e_l), Choose(bnds_r, _trigs_r, e_r)) => {
            Some(def_eq(bnds_l, bnds_r)? && equal_expr(e_l, e_r)?)
        }
        _ => None,
    }
}

fn equal_bnds_typ(left: &Binders<Typ>, right: &Binders<Typ>) -> Option<bool> {
    let eq: Option<bool> = left.iter().zip(right.iter()).fold(Some(true), |b, (bnd_l, bnd_r)| {
        Some(b? && bnd_l.name == bnd_r.name && equal_typ(&bnd_l.a, &bnd_r.a)?)
    });
    eq
}

fn equal_bnds_exp(left: &Binders<Exp>, right: &Binders<Exp>) -> Option<bool> {
    let eq: Option<bool> = left.iter().zip(right.iter()).fold(Some(true), |b, (bnd_l, bnd_r)| {
        Some(b? && bnd_l.name == bnd_r.name && equal_expr(&bnd_l.a, &bnd_r.a)?)
    });
    eq
}

// Computes the syntactic equality of two expressions
// Some(b) means b is exp1 == exp2
// None means we can't tell
// We expect to only call this after eval_expr has been called on both expressions
fn equal_expr(left: &Exp, right: &Exp) -> Option<bool> {
    // If we can't definitively establish equality, we conservatively return None
    let def_eq = |b| if b { Some(true) } else { None };
    use ExpX::*;
    match (&left.x, &right.x) {
        (Const(l), Const(r)) => Some(l == r),
        (Var(l), Var(r)) => {
            if l == r {
                Some(true)
            } else {
                // These are free variables, so we can't know for sure
                // if they are equal (applies to cases below, too)
                None
            }
        }
        (VarLoc(l), VarLoc(r)) => {
            if l == r {
                Some(true)
            } else {
                None
            }
        }
        (VarAt(l, at_l), VarAt(r, at_r)) => {
            if l == r && at_l == at_r {
                Some(true)
            } else {
                None
            }
        }
        (Loc(l), Loc(r)) => equal_expr(l, r),
        (Old(id_l, unique_id_l), Old(id_r, unique_id_r)) => {
            if id_l == id_r && unique_id_l == unique_id_r { Some(true) } else { None }
        }
        (Call(f_l, _, exps_l), Call(f_r, _, exps_r)) => {
            if f_l == f_r && exps_l.len() == exps_r.len() {
                equal_exprs(exps_l, exps_r)
            } else {
                // We don't know if a function call on symbolic values
                // will return the same or different values
                None
            }
        }
        (CallLambda(typ_l, exp_l, exps_l), CallLambda(typ_r, exp_r, exps_r)) => Some(
            equal_typ(typ_l, typ_r)? && equal_expr(exp_l, exp_r)? && equal_exprs(exps_l, exps_r)?,
        ),

        (Ctor(path_l, id_l, bnds_l), Ctor(path_r, id_r, bnds_r)) => {
            if path_l != path_r || id_l != id_r {
                // These are definitely different datatypes or different
                // constructors of the same datatype
                Some(false)
            } else {
                equal_bnds_exp(bnds_l, bnds_r)
            }
        }
        (Unary(op_l, e_l), Unary(op_r, e_r)) => def_eq(op_l == op_r && equal_expr(e_l, e_r)?),
        (UnaryOpr(op_l, e_l), UnaryOpr(op_r, e_r)) => {
            use crate::ast::UnaryOpr::*;
            let op_eq = match (op_l, op_r) {
                (Box(l), Box(r)) => def_eq(equal_typ(l, r)?),
                (Unbox(l), Unbox(r)) => def_eq(equal_typ(l, r)?),
                (HasType(l), HasType(r)) => def_eq(equal_typ(l, r)?),
                (
                    IsVariant { datatype: dt_l, variant: var_l },
                    IsVariant { datatype: dt_r, variant: var_r },
                ) => def_eq(dt_l == dt_r && var_l == var_r),
                (TupleField { .. }, TupleField { .. }) => {
                    panic!("TupleField should have been removed by ast_simplify!")
                }
                (Field(l), Field(r)) => def_eq(l == r),
                _ => None,
            };
            def_eq(op_eq? && equal_expr(e_l, e_r)?)
        }
        (Binary(op_l, e1_l, e2_l), Binary(op_r, e1_r, e2_r)) => {
            def_eq(op_l == op_r && equal_expr(e1_l, e1_r)? && equal_expr(e2_l, e2_r)?)
        }
        (If(e1_l, e2_l, e3_l), If(e1_r, e2_r, e3_r)) => {
            Some(equal_expr(e1_l, e1_r)? && equal_expr(e2_l, e2_r)? && equal_expr(e3_l, e3_r)?)
        }
        (WithTriggers(_trigs_l, e_l), WithTriggers(_trigs_r, e_r)) => equal_expr(e_l, e_r),
        (Bind(bnd_l, e_l), Bind(bnd_r, e_r)) => {
            Some(equal_bnd(bnd_l, bnd_r)? && equal_expr(e_l, e_r)?)
        }
        _ => None,
    }
}

fn equal_exprs(left: &Exps, right: &Exps) -> Option<bool> {
    let eq: Option<bool> = left
        .iter()
        .zip(right.iter())
        .fold(Some(true), |b, (e_l, e_r)| Some(b? && equal_expr(e_l, e_r)?));
    eq
}

fn definitely_equal(left: &Exp, right: &Exp) -> bool {
    match equal_expr(left, right) {
        None => false,
        Some(b) => b,
    }
}

// Based on Dafny's C# implementation:
// https://github.com/dafny-lang/dafny/blob/08744a797296897f4efd486083579e484f57b9dc/Source/DafnyRuntime/DafnyRuntime.cs#L1383
fn euclidean_div(i1: &BigInt, i2: &BigInt) -> BigInt {
    use Sign::*;
    match (i1.sign(), i2.sign()) {
        (Plus | NoSign, Plus | NoSign) => i1 / i2,
        (Plus | NoSign, Minus) => -(i1 / (-i2)),
        (Minus, Plus | NoSign) => -(-i1 - BigInt::one() / i2) - BigInt::one(),
        (Minus, Minus) => ((-i1 - BigInt::one()) / (-i2)) + 1,
    }
}

// Based on Dafny's C# implementation:
// https://github.com/dafny-lang/dafny/blob/08744a797296897f4efd486083579e484f57b9dc/Source/DafnyRuntime/DafnyRuntime.cs#L1436
fn euclidean_mod(i1: &BigInt, i2: &BigInt) -> BigInt {
    use Sign::*;
    match i1.sign() {
        Plus | NoSign => i1 / i2.abs(),
        Minus => {
            let c = (-i1) % i2.abs();
            if c.is_zero() { BigInt::zero() } else { i2.abs() - c }
        }
    }
}

/// Truncate a u128 to a fixed width BigInt
fn u128_to_fixed_width(u: u128, width: u32) -> BigInt {
    match width {
        8 => BigInt::from_u8(u as u8),
        16 => BigInt::from_u16(u as u16),
        32 => BigInt::from_u32(u as u32),
        64 => BigInt::from_u64(u as u64),
        128 => BigInt::from_u128(u as u128),
        _ => panic!("Unexpected fixed-width integer type U({})", width),
    }
    .unwrap()
}

/// Truncate an i128 to a fixed width BigInt
fn i128_to_fixed_width(i: i128, width: u32) -> BigInt {
    match width {
        8 => BigInt::from_i8(i as i8),
        16 => BigInt::from_i16(i as i16),
        32 => BigInt::from_i32(i as i32),
        64 => BigInt::from_i64(i as i64),
        128 => BigInt::from_i128(i as i128),
        _ => panic!("Unexpected fixed-width integer type U({})", width),
    }
    .unwrap()
}

//fn is_sequence_producing(fun: &Fun) -> bool {
//    // TODO: Handle Seq::new; this would require handling lambdas in eval_expr_internal
//    match path_as_rust_name(&fun.path).as_str() {
//        "crate::seq::Seq::empty"
//        | "crate::seq::Seq::push"
//        | "crate::seq::Seq::update"
//        | "crate::seq::Seq::add" => true,
//        _ => false,
//    }
//}

fn is_sequence_consuming(fun: &Fun) -> bool {
    match path_as_rust_name(&fun.path).as_str() {
        "crate::pervasive::seq::Seq::len"
        | "crate::pervasive::seq::Seq::index"
        | "crate::pervasive::seq::Seq::ext_equal"
        | "crate::pervasive::seq::Seq::last" => true,
        _ => false,
    }
}

enum SeqResult {
    Concrete(Vector<Exp>),
    Symbolic,
}

fn eval_seq_producing(ctx: &Ctx, state: &mut State, exp: &Exp) -> Result<SeqResult, VirErr> {
    use ExpX::*;
    use SeqResult::*;
    match &exp.x {
        Call(fun, _typs, args) => {
            let new_args: Result<Vec<Exp>, VirErr> =
                args.iter().map(|e| eval_expr_internal(ctx, state, e)).collect();
            let new_args = new_args?;
            let ok = Ok(Symbolic);
            let get_int = |e: &Exp| match &e.x {
                UnaryOpr(crate::ast::UnaryOpr::Box(_), e) => match &e.x {
                    Const(Constant::Int(index)) => Some(BigInt::to_usize(index).unwrap()),
                    _ => None,
                },
                _ => None,
            };

            // TODO: Handle Seq::new; this would require handling lambdas in eval_expr_internal
            match path_as_rust_name(&fun.path).as_str() {
                "crate::pervasive::seq::Seq::empty" => {
                    //println!("Producing empty");
                    Ok(Concrete(Vector::new()))
                }
                "crate::pervasive::seq::Seq::push" => {
                    //println!("producing push");
                    match eval_seq_producing(ctx, state, &new_args[0])? {
                        Concrete(mut res) => {
                            res.push_back(new_args[1].clone());
                            Ok(Concrete(res))
                        }
                        Symbolic => ok,
                    }
                }
                "crate::pervasive::seq::Seq::update" => {
                    match eval_seq_producing(ctx, state, &new_args[0])? {
                        Concrete(mut res) => match get_int(&new_args[1]) {
                            Some(index) if index < res.len() => {
                                res[index] = new_args[2].clone();
                                Ok(Concrete(res))
                            }
                            _ => ok,
                        },
                        Symbolic => ok,
                    }
                }
                "crate::pervasive::seq::Seq::subrange" => {
                    match eval_seq_producing(ctx, state, &new_args[0])? {
                        Concrete(res) => {
                            let start = get_int(&new_args[1]);
                            let end = get_int(&new_args[2]);
                            match (start, end) {
                                (Some(start), Some(end)) if start <= end && end <= res.len() => {
                                    Ok(Concrete(res.clone().slice(start..end)))
                                }
                                _ => ok,
                            }
                        }
                        Symbolic => ok,
                    }
                }
                "crate::pervasive::seq::Seq::add" => {
                    let s1 = eval_seq_producing(ctx, state, &new_args[0])?;
                    let s2 = eval_seq_producing(ctx, state, &new_args[1])?;
                    match (s1, s2) {
                        (Concrete(mut s1), Concrete(s2)) => {
                            s1.append(s2);
                            Ok(Concrete(s1))
                        }
                        _ => ok,
                    }
                }
                _ => {
                    println!("***Failed to match {} ***", path_as_rust_name(&fun.path));
                    ok
                }
            }
        }
        UnaryOpr(crate::ast::UnaryOpr::Box(_), e) => eval_seq_producing(ctx, state, e),
        UnaryOpr(crate::ast::UnaryOpr::Unbox(_), e) => eval_seq_producing(ctx, state, e),
        _ => panic!("Expected sequence expression to be a Call.  Got {:} instead.", exp),
    }
}

fn eval_seq_consuming(ctx: &Ctx, state: &mut State, exp: &Exp) -> Result<Exp, VirErr> {
    use Constant::*;
    use ExpX::*;
    use SeqResult::*;
    let exp_new = |e: ExpX| Ok(SpannedTyped::new(&exp.span, &exp.typ, e));
    let bool_new = |b: bool| exp_new(Const(Bool(b)));
    let int_new = |i: BigInt| exp_new(Const(Int(i)));
    match &exp.x {
        Call(fun, typs, args) => {
            let new_args: Result<Vec<Exp>, VirErr> =
                args.iter().map(|e| eval_expr_internal(ctx, state, e)).collect();
            let new_args = new_args?;
            let ok = exp_new(Call(fun.clone(), typs.clone(), Arc::new(new_args.clone())));

            match path_as_rust_name(&fun.path).as_str() {
                "crate::pervasive::seq::Seq::len" => {
                    match eval_seq_producing(ctx, state, &new_args[0])? {
                        Concrete(s) => int_new(BigInt::from_usize(s.len()).unwrap()),
                        Symbolic => ok,
                    }
                }
                "crate::pervasive::seq::Seq::index" => {
                    match eval_seq_producing(ctx, state, &new_args[0])? {
                        Concrete(s) => match &new_args[1].x {
                            UnaryOpr(crate::ast::UnaryOpr::Box(_), e) => match &e.x {
                                Const(Constant::Int(index)) => {
                                    let index = BigInt::to_usize(index).unwrap();
                                    if index < s.len() { Ok(s[index].clone()) } else { ok }
                                }
                                _ => ok,
                            },
                            _ => ok,
                        },
                        Symbolic => ok,
                    }
                }
                "crate::pervasive::seq::Seq::ext_equal" => {
                    let left = eval_seq_producing(ctx, state, &new_args[0])?;
                    let right = eval_seq_producing(ctx, state, &new_args[1])?;
                    match (left, right) {
                        (Concrete(l), Concrete(r)) => {
                            let eq = l
                                .iter()
                                .zip(r.iter())
                                .fold(Some(true), |b, (l, r)| Some(b? && equal_expr(l, r)?));
                            match eq {
                                None => ok,
                                Some(b) => bool_new(b),
                            }
                        }
                        _ => ok,
                    }
                }
                "crate::pervasive::seq::Seq::last" => {
                    match eval_seq_producing(ctx, state, &new_args[0])? {
                        Concrete(s) => {
                            if s.len() > 0 {
                                Ok(s.last().unwrap().clone())
                            } else {
                                ok
                            }
                        }
                        Symbolic => ok,
                    }
                }
                _ => ok,
            }
        }
        _ => panic!("Expected sequence expression to be a Call.  Got {:?} instead.", exp),
    }
}

/// Symbolically execute the expression as far as we can,
/// stopping when we hit a symbolic control-flow decision
fn eval_expr_internal(ctx: &Ctx, state: &mut State, exp: &Exp) -> Result<Exp, VirErr> {
    if ctx.time_start.elapsed() > ctx.time_limit {
        return err_str(&exp.span, "assert_by_compute timed out");
    }
    if state.debug {
        println!("{}Evaluating {:}", "\t".repeat(state.depth), exp);
    }
    state.depth += 1;
    let exp_new = |e: ExpX| Ok(SpannedTyped::new(&exp.span, &exp.typ, e));
    let bool_new = |b: bool| exp_new(Const(Constant::Bool(b)));
    let int_new = |i: BigInt| exp_new(Const(Constant::Int(i)));
    let zero = int_new(BigInt::zero());
    let ok = Ok(exp.clone());
    use ExpX::*;
    let r = match &exp.x {
        Const(_) => ok,
        Var(id) => match state.env.get(id) {
            None => {
                if state.debug {
                    println!("Failed to find a match for {:?}", id);
                };
                ok
            }
            Some(e) => {
                if state.debug {
                    //println!("Found match for {:?}", id);
                };
                Ok(e.clone())
            }
        },
        Unary(op, e) => {
            use Constant::*;
            use UnaryOp::*;
            let e = eval_expr_internal(ctx, state, e)?;
            let ok = exp_new(Unary(*op, e.clone()));
            match &e.x {
                Const(Bool(b)) => {
                    // Explicitly enumerate UnaryOps, in case more are added
                    match op {
                        Not => bool_new(!b),
                        BitNot | Clip(_) | Trigger(_) => ok,
                    }
                }
                Const(Int(i)) => {
                    // Explicitly enumerate UnaryOps, in case more are added
                    match op {
                        BitNot => {
                            use IntRange::*;
                            let r = match *e.typ {
                                TypX::Int(U(n)) => {
                                    let i = i.to_u128().unwrap();
                                    u128_to_fixed_width(!i, n)
                                }
                                TypX::Int(I(n)) => {
                                    let i = i.to_i128().unwrap();
                                    i128_to_fixed_width(!i, n)
                                }
                                TypX::Int(USize) => {
                                    let i = i.to_u128().unwrap();
                                    u128_to_fixed_width(!i, ARCH_SIZE_MIN_BITS)
                                }
                                TypX::Int(ISize) => {
                                    let i = i.to_i128().unwrap();
                                    i128_to_fixed_width(!i, ARCH_SIZE_MIN_BITS)
                                }

                                _ => panic!(
                                    "Type checker should not allow bitwise ops on non-fixed-width types"
                                ),
                            };
                            int_new(r)
                        }
                        Clip(range) => {
                            let apply_range = |lower: BigInt, upper: BigInt| {
                                if i < &lower || i > &upper { ok.clone() } else { Ok(e.clone()) }
                            };
                            match range {
                                IntRange::Int => ok,
                                IntRange::Nat => apply_range(BigInt::zero(), i.clone()),
                                IntRange::U(n) => {
                                    let u = apply_range(
                                        BigInt::zero(),
                                        (BigInt::one() << n) - BigInt::one(),
                                    );
                                    u
                                }
                                IntRange::I(n) => apply_range(
                                    BigInt::one() << (n - 1),
                                    (BigInt::one() << (n - 1)) - BigInt::one(),
                                ),
                                IntRange::USize => {
                                    let u = apply_range(
                                        BigInt::zero(),
                                        (BigInt::one() << ARCH_SIZE_MIN_BITS) - BigInt::one(),
                                    );
                                    u
                                }
                                IntRange::ISize => apply_range(
                                    BigInt::one() << (ARCH_SIZE_MIN_BITS - 1),
                                    (BigInt::one() << (ARCH_SIZE_MIN_BITS - 1)) - BigInt::one(),
                                ),
                            }
                        }
                        Not | Trigger(_) => ok,
                    }
                }
                // !(!(e_inner)) == e_inner
                Unary(Not, e_inner) if matches!(op, Not) => Ok(e_inner.clone()),
                _ => ok,
            }
        }
        UnaryOpr(op, e) => {
            let e = eval_expr_internal(ctx, state, e)?;
            let ok = exp_new(UnaryOpr(op.clone(), e.clone()));
            use crate::ast::UnaryOpr::*;
            match op {
                Box(_) => ok,
                Unbox(_) => match &e.x {
                    UnaryOpr(Box(_), inner_e) => {
                        if state.debug {
                            //println!("Unbox found matching box");
                        };
                        Ok(inner_e.clone())
                    }
                    _ => ok,
                },
                HasType(_) => ok,
                IsVariant { datatype, variant } => match &e.x {
                    Ctor(dt, var, _) => {
                        if state.debug {
                            //println!("IsVariant found matching Ctor!");
                        };
                        bool_new(dt == datatype && var == variant)
                    }
                    _ => ok,
                },
                TupleField { .. } => panic!("TupleField should have been removed by ast_simplify!"),
                Field(f) => match &e.x {
                    Ctor(_dt, _var, binders) => {
                        match binders.iter().position(|b| b.name == f.field) {
                            None => ok,
                            Some(i) => Ok(binders.get(i).unwrap().a.clone()),
                        }
                    }
                    _ => ok,
                },
            }
        }
        Binary(op, e1, e2) => {
            use BinaryOp::*;
            use Constant::*;
            // We initially evaluate only e1, since op may short circuit
            // e.g., x != 0 && y == 5 / x
            let e1 = eval_expr_internal(ctx, state, e1)?;
            // Create the default value with a possibly updated value for e2
            let ok_e2 = |e2: Exp| exp_new(Binary(*op, e1.clone(), e2.clone()));
            match op {
                And => match &e1.x {
                    Const(Bool(true)) => eval_expr_internal(ctx, state, e2),
                    Const(Bool(false)) => bool_new(false),
                    _ => {
                        let e2 = eval_expr_internal(ctx, state, e2)?;
                        match &e2.x {
                            Const(Bool(true)) => Ok(e1.clone()),
                            Const(Bool(false)) => bool_new(false),
                            _ => ok_e2(e2),
                        }
                    }
                },
                Or => match &e1.x {
                    Const(Bool(true)) => bool_new(true),
                    Const(Bool(false)) => eval_expr_internal(ctx, state, e2),
                    _ => {
                        let e2 = eval_expr_internal(ctx, state, e2)?;
                        match &e2.x {
                            Const(Bool(true)) => bool_new(true),
                            Const(Bool(false)) => Ok(e1.clone()),
                            _ => ok_e2(e2),
                        }
                    }
                },
                Xor => {
                    let e2 = eval_expr_internal(ctx, state, e2)?;
                    match (&e1.x, &e2.x) {
                        (Const(Bool(b1)), Const(Bool(b2))) => {
                            let r = (*b1 && !b2) || (!b1 && *b2);
                            bool_new(r)
                        }
                        (Const(Bool(true)), _) => exp_new(Unary(UnaryOp::Not, e2.clone())),
                        (Const(Bool(false)), _) => Ok(e2.clone()),
                        (_, Const(Bool(true))) => exp_new(Unary(UnaryOp::Not, e1.clone())),
                        (_, Const(Bool(false))) => Ok(e1.clone()),
                        _ => ok_e2(e2),
                    }
                }
                Implies => {
                    match &e1.x {
                        Const(Bool(true)) => eval_expr_internal(ctx, state, e2),
                        Const(Bool(false)) => bool_new(true),
                        _ => {
                            let e2 = eval_expr_internal(ctx, state, e2)?;
                            match &e2.x {
                                Const(Bool(true)) => bool_new(false),
                                Const(Bool(false)) =>
                                // Recurse in case we can simplify the new negation
                                {
                                    eval_expr_internal(
                                        ctx,
                                        state,
                                        &exp_new(Unary(UnaryOp::Not, e1.clone()))?,
                                    )
                                }
                                _ => ok_e2(e2),
                            }
                        }
                    }
                }
                Eq(_mode) => {
                    let e2 = eval_expr_internal(ctx, state, e2)?;
                    match equal_expr(&e1, &e2) {
                        None => ok_e2(e2),
                        Some(b) => bool_new(b),
                    }
                }
                Ne => {
                    let e2 = eval_expr_internal(ctx, state, e2)?;
                    match equal_expr(&e1, &e2) {
                        None => ok_e2(e2),
                        Some(b) => bool_new(!b),
                    }
                }
                Inequality(op) => {
                    let e2 = eval_expr_internal(ctx, state, e2)?;
                    match (&e1.x, &e2.x) {
                        (Const(Int(i1)), Const(Int(i2))) => {
                            use InequalityOp::*;
                            let b = match op {
                                Le => i1 <= i2,
                                Ge => i1 >= i2,
                                Lt => i1 < i2,
                                Gt => i1 > i2,
                            };
                            bool_new(b)
                        }
                        _ => ok_e2(e2),
                    }
                }
                Arith(op, _mode) => {
                    let e2 = eval_expr_internal(ctx, state, e2)?;
                    use ArithOp::*;
                    match (&e1.x, &e2.x) {
                        // Ideal case where both sides are concrete
                        (Const(Int(i1)), Const(Int(i2))) => {
                            use ArithOp::*;
                            match op {
                                Add => int_new(i1 + i2),
                                Sub => int_new(i1 - i2),
                                Mul => int_new(i1 * i2),
                                EuclideanDiv => {
                                    if i2.is_zero() {
                                        ok_e2(e2) // Treat as symbolic instead of erroring
                                    } else {
                                        int_new(euclidean_div(i1, i2))
                                    }
                                }
                                EuclideanMod => {
                                    if i2.is_zero() {
                                        ok_e2(e2) // Treat as symbolic instead of erroring
                                    } else {
                                        int_new(euclidean_mod(i1, i2))
                                    }
                                }
                            }
                        }
                        // Special cases for certain concrete values
                        (Const(Int(i1)), _) if i1.is_zero() && matches!(op, Add) => Ok(e2.clone()),
                        (Const(Int(i1)), _) if i1.is_zero() && matches!(op, Mul) => zero,
                        (Const(Int(i1)), _) if i1.is_one() && matches!(op, Mul) => Ok(e2.clone()),
                        (_, Const(Int(i2))) if i2.is_zero() => {
                            use ArithOp::*;
                            match op {
                                Add | Sub => Ok(e1.clone()),
                                Mul => zero,
                                EuclideanDiv => {
                                    ok_e2(e2) // Treat as symbolic instead of erroring
                                }
                                EuclideanMod => {
                                    ok_e2(e2) // Treat as symbolic instead of erroring
                                }
                            }
                        }
                        (_, Const(Int(i2))) if i2.is_one() && matches!(op, EuclideanMod) => {
                            int_new(BigInt::one())
                        }
                        (_, Const(Int(i2))) if i2.is_one() && matches!(op, Mul | EuclideanDiv) => {
                            Ok(e1.clone())
                        }
                        _ => {
                            match op {
                                // X - X => 0
                                ArithOp::Sub if definitely_equal(&e1, &e2) => zero,
                                _ => ok_e2(e2),
                            }
                        }
                    }
                }
                Bitwise(op) => {
                    use BitwiseOp::*;
                    let e2 = eval_expr_internal(ctx, state, e2)?;
                    match (&e1.x, &e2.x) {
                        // Ideal case where both sides are concrete
                        (Const(Int(i1)), Const(Int(i2))) => match op {
                            BitXor => int_new(i1 ^ i2),
                            BitAnd => int_new(i1 & i2),
                            BitOr => int_new(i1 | i2),
                            Shr | Shl => match i2.to_u128() {
                                None => ok_e2(e2),
                                Some(shift) => {
                                    use IntRange::*;
                                    let r = match *exp.typ {
                                        TypX::Int(U(n)) => {
                                            let i1 = i1.to_u128().unwrap();
                                            let res = if matches!(op, Shr) {
                                                i1 >> shift
                                            } else {
                                                i1 << shift
                                            };
                                            u128_to_fixed_width(res, n)
                                        }
                                        TypX::Int(I(n)) => {
                                            let i1 = i1.to_i128().unwrap();
                                            let res = if matches!(op, Shr) {
                                                i1 >> shift
                                            } else {
                                                i1 << shift
                                            };
                                            i128_to_fixed_width(res, n)
                                        }
                                        TypX::Int(USize) => {
                                            let i1 = i1.to_u128().unwrap();
                                            let res = if matches!(op, Shr) {
                                                i1 >> shift
                                            } else {
                                                i1 << shift
                                            };
                                            u128_to_fixed_width(res, ARCH_SIZE_MIN_BITS)
                                        }
                                        TypX::Int(ISize) => {
                                            let i1 = i1.to_i128().unwrap();
                                            let res = if matches!(op, Shr) {
                                                i1 >> shift
                                            } else {
                                                i1 << shift
                                            };
                                            i128_to_fixed_width(res, ARCH_SIZE_MIN_BITS)
                                        }
                                        _ => panic!(
                                            "Type checker should not allow bitwise ops on non-fixed-width types"
                                        ),
                                    };
                                    int_new(r)
                                }
                            },
                        },
                        // Special cases for certain concrete values
                        (Const(Int(i)), _) | (_, Const(Int(i)))
                            if i.is_zero() && matches!(op, BitAnd) =>
                        {
                            zero
                        }
                        (Const(Int(i1)), _) if i1.is_zero() && matches!(op, BitOr) => {
                            Ok(e2.clone())
                        }
                        (_, Const(Int(i2))) if i2.is_zero() && matches!(op, BitOr) => {
                            Ok(e1.clone())
                        }
                        _ => {
                            match op {
                                // X ^ X => 0
                                BitXor if definitely_equal(&e1, &e2) => zero,
                                // X & X = X, X | X = X
                                BitAnd | BitOr if definitely_equal(&e1, &e2) => Ok(e1.clone()),
                                _ => ok_e2(e2),
                            }
                        }
                    }
                }
            }
        }
        If(e1, e2, e3) => {
            let e1 = eval_expr_internal(ctx, state, e1)?;
            match &e1.x {
                Const(Constant::Bool(b)) => {
                    if *b {
                        eval_expr_internal(ctx, state, e2)
                    } else {
                        eval_expr_internal(ctx, state, e3)
                    }
                }
                _ => exp_new(If(e1, e2.clone(), e3.clone())),
            }
        }
        Call(fun, typs, exps) => {
            let new_exps: Result<Vec<Exp>, VirErr> =
                exps.iter().map(|e| eval_expr_internal(ctx, state, e)).collect();
            let new_exps = Arc::new(new_exps?);
            match state.lookup_call(&fun, &new_exps) {
                Some(prev_result) => Ok(prev_result),
                None => {
                    let result = if is_sequence_consuming(&fun) {
                        eval_seq_consuming(ctx, state, exp)
                    } else {
                        match ctx.fun_ssts.get(fun) {
                            None => exp_new(Call(fun.clone(), typs.clone(), new_exps.clone())),
                            Some((params, body)) => {
                                state.env.push_scope(true);
                                for (formal, actual) in params.iter().zip(new_exps.iter()) {
                                    if state.debug {
                                        //println!("Binding {:?} to {:?}", formal, actual.x);
                                    }
                                    state
                                        .env
                                        .insert((formal.x.name.clone(), Some(0)), actual.clone())
                                        .unwrap();
                                }
                                let e = eval_expr_internal(ctx, state, body);
                                state.env.pop_scope();
                                e
                            }
                        }
                    };
                    state.insert_call(fun, &new_exps, &result.clone()?);
                    result
                }
            }
        }
        Bind(bnd, e) => match &bnd.x {
            BndX::Let(bnds) => {
                state.env.push_scope(true);
                for b in bnds.iter() {
                    let val = eval_expr_internal(ctx, state, &b.a)?;
                    state.env.insert((b.name.clone(), None), val).unwrap();
                }
                let e = eval_expr_internal(ctx, state, e);
                state.env.pop_scope();
                e
            }
            _ => ok,
        },
        Ctor(path, id, bnds) => {
            let new_bnds: Result<Vec<Binder<Exp>>, VirErr> = bnds
                .iter()
                .map(|b| {
                    let name = b.name.clone();
                    let a = eval_expr_internal(ctx, state, &b.a)?;
                    Ok(Arc::new(BinderX { name, a }))
                })
                .collect();
            let new_bnds = new_bnds?;
            exp_new(Ctor(path.clone(), id.clone(), Arc::new(new_bnds)))
        }
        // Ignored by the interpreter at present (i.e., treated as symbolic)
        VarAt(..) | VarLoc(..) | Loc(..) | Old(..) | CallLambda(..) | WithTriggers(..) => ok,
    };
    let res = r?;
    state.depth -= 1;
    if state.debug {
        println!("{}=> {:}", "\t".repeat(state.depth), &res);
    }
    Ok(res)
}

pub fn eval_expr(exp: &Exp, fun_ssts: &SstMap, rlimit: u32) -> Result<Exp, VirErr> {
    let env = ScopeMap::new();
    let cache = HashMap::new();
    let mut state = State { depth: 0, env, debug: false, cache };
    // Don't run for more than rlimit seconds
    let time_limit = Duration::new(rlimit as u64, 0);
    let time_start = Instant::now();
    let ctx = Ctx { fun_ssts, time_start, time_limit };
    //println!("Starting from {:?}", exp);
    eval_expr_internal(&ctx, &mut state, exp)
}
