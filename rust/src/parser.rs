//! Parser for the ac language.
//!
//! Converts a token stream into an AST. Modeled after Go's recursive
//! descent parser with Zig-style precedence climbing for expressions.

use crate::scanner::Token;

/// Top-level AST node.
#[derive(Debug)]
pub enum Decl {
    Function(FnDecl),
}

#[derive(Debug)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
    pub body: Block,
}

#[derive(Debug)]
pub struct Param {
    pub name: String,
    pub ty: TypeExpr,
}

#[derive(Debug)]
pub struct TypeExpr {
    pub name: String,
}

#[derive(Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}

#[derive(Debug)]
pub enum Stmt {
    Return(Option<Expr>),
    Let { name: String, ty: Option<TypeExpr>, value: Expr },
    Expr(Expr),
}

#[derive(Debug)]
pub enum Expr {
    IntLiteral(i64),
    FloatLiteral(f64),
    StringLiteral(String),
    BoolLiteral(bool),
    Ident(String),
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr> },
    Call { callee: String, args: Vec<Expr> },
}

#[derive(Debug, Clone, Copy)]
pub enum BinOp {
    Add, Sub, Mul, Div, Rem,
    Eq, Ne, Lt, Le, Gt, Ge,
}

/// Parse a token stream into a list of declarations.
pub fn parse(_tokens: &[Token]) -> Vec<Decl> {
    // TODO: implement parser
    vec![]
}
