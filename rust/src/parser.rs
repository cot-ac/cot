//! Parser for the ac language.
//!
//! Converts a token stream into an AST. Port of the C++ Parser
//! (Parser.h + Parser.cpp). Recursive descent with Zig-style
//! precedence climbing for expressions.

use crate::scanner::{Token, TokenKind};

// ---------------------------------------------------------------------------
// AST types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TypeRef {
    pub name: String,
    pub array_len: i64,
    pub is_array: bool,
    pub is_optional: bool,
    pub is_error_union: bool,
    pub is_pointer: bool,
    pub is_slice: bool,
}

impl TypeRef {
    pub fn simple(name: &str) -> Self {
        Self {
            name: name.to_string(),
            array_len: 0,
            is_array: false,
            is_optional: false,
            is_error_union: false,
            is_pointer: false,
            is_slice: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: TypeRef,
}

#[derive(Debug, Clone)]
pub struct FieldInit {
    pub name: String,
    pub value: Expr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExprKind {
    IntLit,
    FloatLit,
    BoolLit,
    StringLit,
    NullLit,
    Ident,
    BinOp,
    UnaryOp,
    Call,
    StructLit,
    FieldAccess,
    ArrayLit,
    Index,
    ForceUnwrap,
    TryUnwrap,
    AddrOf,
    Deref,
    CastAs,
    SliceFrom,
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub pos: usize,

    pub int_val: i64,
    pub float_val: f64,
    pub bool_val: bool,
    pub name: String,
    pub str_val: String,
    pub op: TokenKind,
    pub lhs: Option<Box<Expr>>,
    pub rhs: Option<Box<Expr>>,
    pub args: Vec<Expr>,
    pub fields: Vec<FieldInit>,
    pub type_args: Vec<TypeRef>,
    pub cast_type: TypeRef,
}

impl Expr {
    fn new(kind: ExprKind, pos: usize) -> Self {
        Self {
            kind,
            pos,
            int_val: 0,
            float_val: 0.0,
            bool_val: false,
            name: String::new(),
            str_val: String::new(),
            op: TokenKind::Invalid,
            lhs: None,
            rhs: None,
            args: Vec::new(),
            fields: Vec::new(),
            type_args: Vec::new(),
            cast_type: TypeRef::simple(""),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StmtKind {
    Return,
    ExprStmt,
    If,
    While,
    For,
    Break,
    Continue,
    Let,
    Var,
    Assign,
    CompoundAssign,
    Assert,
    Match,
}

#[derive(Debug, Clone)]
pub struct Stmt {
    pub kind: StmtKind,
    pub pos: usize,
    pub expr: Option<Expr>,
    pub range_end: Option<Expr>,
    pub then_body: Vec<Stmt>,
    pub else_body: Vec<Stmt>,
    pub var_name: String,
    pub var_type: TypeRef,
    pub op: TokenKind,
    // Match arms
    pub match_variants: Vec<String>,
    pub match_bodies: Vec<Vec<Stmt>>,
    // For complex lhs assignment (field/index)
    pub assign_lhs: Option<Expr>,
}

impl Stmt {
    fn new(kind: StmtKind, pos: usize) -> Self {
        Self {
            kind,
            pos,
            expr: None,
            range_end: None,
            then_body: Vec::new(),
            else_body: Vec::new(),
            var_name: String::new(),
            var_type: TypeRef::simple(""),
            op: TokenKind::Invalid,
            match_variants: Vec::new(),
            match_bodies: Vec::new(),
            assign_lhs: None,
        }
    }
}

#[derive(Debug)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<String>,
    pub pos: usize,
}

#[derive(Debug)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<Param>,
    pub pos: usize,
}

#[derive(Debug)]
pub struct FnDecl {
    pub name: String,
    pub type_params: Vec<String>,
    pub params: Vec<Param>,
    pub return_type: TypeRef,
    pub body: Vec<Stmt>,
    pub pos: usize,
}

#[derive(Debug)]
pub struct TestDecl {
    pub name: String,
    pub body: Vec<Stmt>,
    pub pos: usize,
}

#[derive(Debug)]
pub struct Module {
    pub enums: Vec<EnumDef>,
    pub structs: Vec<StructDef>,
    pub functions: Vec<FnDecl>,
    pub tests: Vec<TestDecl>,
}

// ---------------------------------------------------------------------------
// Precedence table (Zig pattern)
// ---------------------------------------------------------------------------

fn precedence(kind: TokenKind) -> i32 {
    match kind {
        TokenKind::Catch | TokenKind::Orelse => 5,
        TokenKind::PipePipe => 10,
        TokenKind::AmpAmp => 20,
        TokenKind::EqEq | TokenKind::BangEq => 30,
        TokenKind::Less | TokenKind::LessEq | TokenKind::Greater | TokenKind::GreaterEq => 40,
        TokenKind::Pipe => 50,
        TokenKind::Caret => 60,
        TokenKind::Amp => 70,
        TokenKind::Shl | TokenKind::Shr => 80,
        TokenKind::Plus | TokenKind::Minus => 90,
        TokenKind::Star | TokenKind::Slash | TokenKind::Percent => 100,
        _ => 0,
    }
}

/// Can this token start a type expression?
/// Used to disambiguate generic call name[Type](...) from array index name[expr].
fn is_type_start(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::I8
            | TokenKind::I16
            | TokenKind::I32
            | TokenKind::I64
            | TokenKind::U8
            | TokenKind::U16
            | TokenKind::U32
            | TokenKind::U64
            | TokenKind::F32
            | TokenKind::F64
            | TokenKind::Bool
            | TokenKind::Void
            | TokenKind::Identifier
            | TokenKind::Question
            | TokenKind::Star
    )
}

// ---------------------------------------------------------------------------
// Parser implementation
// ---------------------------------------------------------------------------

struct Parser<'a> {
    source: &'a str,
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str, tokens: &'a [Token]) -> Self {
        Self {
            source,
            tokens,
            pos: 0,
        }
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn peek_at(&self, offset: usize) -> &Token {
        &self.tokens[self.pos + offset]
    }

    fn advance(&mut self) -> (usize, usize) {
        let tok = &self.tokens[self.pos];
        self.pos += 1;
        (tok.start, tok.end)
    }

    fn check(&self, k: TokenKind) -> bool {
        self.peek().kind == k
    }

    fn match_tok(&mut self, k: TokenKind) -> bool {
        if self.check(k) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn text(&self, start: usize, end: usize) -> &str {
        &self.source[start..end]
    }

    fn expect(&mut self, k: TokenKind) -> (usize, usize) {
        if !self.check(k) {
            let s = self.peek().start;
            let e = self.peek().end;
            eprintln!(
                "error: expected token {:?} got '{}'",
                k,
                &self.source[s..e]
            );
        }
        self.advance()
    }

    fn skip_semis(&mut self) {
        while self.check(TokenKind::Semicolon) {
            self.advance();
        }
    }

    // ---- Type ----

    fn parse_type(&mut self) -> TypeRef {
        // Pointer type: *T
        if self.check(TokenKind::Star) {
            self.advance();
            let mut inner = self.parse_type();
            inner.is_pointer = true;
            return inner;
        }
        // Optional type: ?T
        if self.check(TokenKind::Question) {
            self.advance();
            let mut inner = self.parse_type();
            inner.is_optional = true;
            return inner;
        }
        // Array type [N]T or slice type []T
        if self.check(TokenKind::LBracket) {
            self.advance(); // [
            if self.check(TokenKind::RBracket) {
                self.advance(); // ]
                let elem_type = self.parse_type();
                return TypeRef {
                    name: elem_type.name,
                    array_len: 0,
                    is_array: false,
                    is_optional: false,
                    is_error_union: false,
                    is_pointer: false,
                    is_slice: true,
                };
            }
            let (ls, le) = self.expect(TokenKind::IntLiteral);
            let len: i64 = self.text(ls, le).parse().unwrap_or(0);
            self.expect(TokenKind::RBracket);
            let elem_type = self.parse_type();
            return TypeRef {
                name: elem_type.name,
                array_len: len,
                is_array: true,
                is_optional: false,
                is_error_union: false,
                is_pointer: false,
                is_slice: false,
            };
        }

        let (ts, te) = self.advance();
        let name = self.text(ts, te).to_string();
        let mut t = TypeRef::simple(&name);

        // Error union type suffix: T!error
        if self.check(TokenKind::Bang)
            && self.pos + 1 < self.tokens.len()
            && self.peek_at(1).kind == TokenKind::Identifier
        {
            let (es, ee) = (self.peek_at(1).start, self.peek_at(1).end);
            if self.text(es, ee) == "error" {
                self.advance(); // !
                self.advance(); // error
                t.is_error_union = true;
            }
        }

        t
    }

    // ---- Expressions ----

    fn parse_primary(&mut self) -> Expr {
        let tok_kind = self.peek().kind;
        let tok_start = self.peek().start;
        let tok_end = self.peek().end;

        if tok_kind == TokenKind::IntLiteral {
            self.advance();
            let txt = self.text(tok_start, tok_end);
            let clean: String = txt.chars().filter(|&c| c != '_').collect();
            let mut e = Expr::new(ExprKind::IntLit, tok_start);
            e.int_val = clean.parse().unwrap_or(0);
            return e;
        }

        if tok_kind == TokenKind::FloatLiteral {
            self.advance();
            let txt = self.text(tok_start, tok_end);
            let clean: String = txt.chars().filter(|&c| c != '_').collect();
            let mut e = Expr::new(ExprKind::FloatLit, tok_start);
            e.float_val = clean.parse().unwrap_or(0.0);
            return e;
        }

        if tok_kind == TokenKind::BoolTrue || tok_kind == TokenKind::BoolFalse {
            self.advance();
            let mut e = Expr::new(ExprKind::BoolLit, tok_start);
            e.bool_val = tok_kind == TokenKind::BoolTrue;
            return e;
        }

        if tok_kind == TokenKind::StringLiteral {
            self.advance();
            let raw = self.text(tok_start, tok_end);
            let mut e = Expr::new(ExprKind::StringLit, tok_start);
            e.str_val = raw[1..raw.len() - 1].to_string();
            return e;
        }

        if tok_kind == TokenKind::Null {
            self.advance();
            return Expr::new(ExprKind::NullLit, tok_start);
        }

        // Array literal: [expr, expr, ...]
        if tok_kind == TokenKind::LBracket {
            self.advance();
            let mut e = Expr::new(ExprKind::ArrayLit, tok_start);
            if !self.check(TokenKind::RBracket) {
                e.args.push(self.parse_expr());
                while self.match_tok(TokenKind::Comma) {
                    e.args.push(self.parse_expr());
                }
            }
            self.expect(TokenKind::RBracket);
            return e;
        }

        if tok_kind == TokenKind::Identifier {
            self.advance();
            let name = self.text(tok_start, tok_end).to_string();

            // Struct literal: Name { field: expr, ... }
            if self.check(TokenKind::LBrace)
                && self.pos + 1 < self.tokens.len()
                && self.peek_at(1).kind == TokenKind::Identifier
                && self.pos + 2 < self.tokens.len()
                && self.peek_at(2).kind == TokenKind::Colon
            {
                self.advance(); // {
                let mut e = Expr::new(ExprKind::StructLit, tok_start);
                e.name = name;
                if !self.check(TokenKind::RBrace) {
                    let (fs, fe) = self.expect(TokenKind::Identifier);
                    let field_name = self.text(fs, fe).to_string();
                    self.expect(TokenKind::Colon);
                    let value = self.parse_expr();
                    e.fields.push(FieldInit { name: field_name, value });
                    while self.match_tok(TokenKind::Comma) {
                        self.skip_semis();
                        if self.check(TokenKind::RBrace) { break; }
                        let (fs, fe) = self.expect(TokenKind::Identifier);
                        let field_name = self.text(fs, fe).to_string();
                        self.expect(TokenKind::Colon);
                        let value = self.parse_expr();
                        e.fields.push(FieldInit { name: field_name, value });
                    }
                }
                self.skip_semis();
                self.expect(TokenKind::RBrace);
                return e;
            }

            // Generic call: identity[i32](args...)
            if self.check(TokenKind::LBracket)
                && self.pos + 1 < self.tokens.len()
                && is_type_start(self.peek_at(1).kind)
            {
                self.advance(); // [
                let mut e = Expr::new(ExprKind::Call, tok_start);
                e.name = name;
                if !self.check(TokenKind::RBracket) {
                    e.type_args.push(self.parse_type());
                    while self.match_tok(TokenKind::Comma) {
                        e.type_args.push(self.parse_type());
                    }
                }
                self.expect(TokenKind::RBracket);
                self.expect(TokenKind::LParen);
                if !self.check(TokenKind::RParen) {
                    e.args.push(self.parse_expr());
                    while self.match_tok(TokenKind::Comma) {
                        e.args.push(self.parse_expr());
                    }
                }
                self.expect(TokenKind::RParen);
                return e;
            }

            // Function call: ident(args...)
            if self.check(TokenKind::LParen) {
                self.advance(); // (
                let mut e = Expr::new(ExprKind::Call, tok_start);
                e.name = name;
                if !self.check(TokenKind::RParen) {
                    e.args.push(self.parse_expr());
                    while self.match_tok(TokenKind::Comma) {
                        e.args.push(self.parse_expr());
                    }
                }
                self.expect(TokenKind::RParen);
                return e;
            }

            let mut e = Expr::new(ExprKind::Ident, tok_start);
            e.name = name;
            return e;
        }

        if tok_kind == TokenKind::LParen {
            self.advance();
            let e = self.parse_expr();
            self.expect(TokenKind::RParen);
            return e;
        }

        // try expr
        if tok_kind == TokenKind::Try {
            self.advance();
            let mut e = Expr::new(ExprKind::TryUnwrap, tok_start);
            let inner = self.parse_primary();
            let inner = self.parse_postfix(inner);
            e.lhs = Some(Box::new(inner));
            return e;
        }

        // Address-of: &expr
        if tok_kind == TokenKind::Amp {
            self.advance();
            let mut e = Expr::new(ExprKind::AddrOf, tok_start);
            let inner = self.parse_primary();
            e.lhs = Some(Box::new(inner));
            return e;
        }

        // Dereference: *expr
        if tok_kind == TokenKind::Star {
            self.advance();
            let mut e = Expr::new(ExprKind::Deref, tok_start);
            let inner = self.parse_primary();
            let inner = self.parse_postfix(inner);
            e.lhs = Some(Box::new(inner));
            return e;
        }

        // Unary operators: -, !, ~
        if tok_kind == TokenKind::Minus
            || tok_kind == TokenKind::Bang
            || tok_kind == TokenKind::Tilde
        {
            self.advance();
            let mut e = Expr::new(ExprKind::UnaryOp, tok_start);
            e.op = tok_kind;
            let inner = self.parse_primary();
            e.rhs = Some(Box::new(inner));
            return e;
        }

        eprintln!("error: unexpected token '{}'", self.text(tok_start, tok_end));
        self.advance();
        Expr::new(ExprKind::IntLit, tok_start) // error recovery
    }

    fn parse_postfix(&mut self, mut base: Expr) -> Expr {
        loop {
            if self.check(TokenKind::Bang) {
                if self.pos + 1 < self.tokens.len() && self.peek_at(1).kind == TokenKind::Eq {
                    break;
                }
                let bang_start = self.peek().start;
                self.advance();
                let mut e = Expr::new(ExprKind::ForceUnwrap, bang_start);
                e.lhs = Some(Box::new(base));
                base = e;
                continue;
            }
            if self.check(TokenKind::Dot) {
                self.advance(); // .
                let fs = self.peek().start;
                let fe = self.peek().end;
                self.expect(TokenKind::Identifier);
                let field_name = self.text(fs, fe).to_string();
                let mut e = Expr::new(ExprKind::FieldAccess, fs);
                e.name = field_name;
                e.lhs = Some(Box::new(base));
                base = e;
                continue;
            }
            if self.check(TokenKind::LBracket) {
                self.advance(); // [
                let idx = self.parse_expr();
                if self.check(TokenKind::DotDot) {
                    self.advance();
                    let hi = self.parse_expr();
                    self.expect(TokenKind::RBracket);
                    let mut e = Expr::new(ExprKind::SliceFrom, base.pos);
                    e.lhs = Some(Box::new(base));
                    e.rhs = Some(Box::new(hi));
                    e.args.push(idx);
                    base = e;
                    continue;
                }
                self.expect(TokenKind::RBracket);
                let mut e = Expr::new(ExprKind::Index, base.pos);
                e.lhs = Some(Box::new(base));
                e.rhs = Some(Box::new(idx));
                base = e;
                continue;
            }
            if self.check(TokenKind::As) {
                let as_start = self.peek().start;
                self.advance();
                let mut e = Expr::new(ExprKind::CastAs, as_start);
                e.lhs = Some(Box::new(base));
                e.cast_type = self.parse_type();
                base = e;
                continue;
            }
            break;
        }
        base
    }

    fn parse_binary_expr(&mut self, mut lhs: Expr, min_prec: i32) -> Expr {
        loop {
            let prec = precedence(self.peek().kind);
            if prec < min_prec {
                break;
            }
            let op_kind = self.peek().kind;
            let op_pos = self.peek().start;
            self.advance();
            let mut rhs = self.parse_primary();
            rhs = self.parse_postfix(rhs);
            while precedence(self.peek().kind) > prec {
                rhs = self.parse_binary_expr(rhs, prec + 1);
            }
            let mut binop = Expr::new(ExprKind::BinOp, op_pos);
            binop.op = op_kind;
            binop.lhs = Some(Box::new(lhs));
            binop.rhs = Some(Box::new(rhs));
            lhs = binop;
        }
        lhs
    }

    fn parse_expr(&mut self) -> Expr {
        let lhs = self.parse_primary();
        let lhs = self.parse_postfix(lhs);
        self.parse_binary_expr(lhs, 1)
    }

    // ---- Statements ----

    fn parse_block(&mut self) -> Vec<Stmt> {
        self.expect(TokenKind::LBrace);
        self.skip_semis();
        let mut stmts = Vec::new();
        while !self.check(TokenKind::RBrace) && !self.check(TokenKind::Eof) {
            stmts.push(self.parse_stmt());
            self.skip_semis();
        }
        self.expect(TokenKind::RBrace);
        stmts
    }

    fn parse_stmt(&mut self) -> Stmt {
        let tok_kind = self.peek().kind;
        let tok_start = self.peek().start;

        if tok_kind == TokenKind::Return {
            self.advance();
            let mut s = Stmt::new(StmtKind::Return, tok_start);
            if !self.check(TokenKind::Semicolon)
                && !self.check(TokenKind::RBrace)
                && !self.check(TokenKind::Eof)
            {
                s.expr = Some(self.parse_expr());
            }
            return s;
        }

        if tok_kind == TokenKind::Let || tok_kind == TokenKind::Var {
            let is_mut = tok_kind == TokenKind::Var;
            self.advance();
            let mut s = Stmt::new(
                if is_mut { StmtKind::Var } else { StmtKind::Let },
                tok_start,
            );
            let (ns, ne) = self.expect(TokenKind::Identifier);
            s.var_name = self.text(ns, ne).to_string();
            if self.match_tok(TokenKind::Colon) {
                s.var_type = self.parse_type();
            }
            self.expect(TokenKind::Eq);
            s.expr = Some(self.parse_expr());
            return s;
        }

        if tok_kind == TokenKind::If {
            self.advance();
            let mut s = Stmt::new(StmtKind::If, tok_start);
            s.expr = Some(self.parse_expr());
            s.then_body = self.parse_block();
            self.skip_semis();
            if self.match_tok(TokenKind::Else) {
                if self.check(TokenKind::If) {
                    s.else_body.push(self.parse_stmt());
                } else {
                    s.else_body = self.parse_block();
                }
            }
            return s;
        }

        if tok_kind == TokenKind::Assert {
            self.advance();
            let mut s = Stmt::new(StmtKind::Assert, tok_start);
            self.expect(TokenKind::LParen);
            s.expr = Some(self.parse_expr());
            self.expect(TokenKind::RParen);
            return s;
        }

        if tok_kind == TokenKind::For {
            self.advance();
            let mut s = Stmt::new(StmtKind::For, tok_start);
            let (ns, ne) = self.expect(TokenKind::Identifier);
            s.var_name = self.text(ns, ne).to_string();
            self.expect(TokenKind::In);
            s.expr = Some(self.parse_expr());
            self.expect(TokenKind::DotDot);
            s.range_end = Some(self.parse_expr());
            s.then_body = self.parse_block();
            return s;
        }

        if tok_kind == TokenKind::Break {
            self.advance();
            return Stmt::new(StmtKind::Break, tok_start);
        }

        if tok_kind == TokenKind::Continue {
            self.advance();
            return Stmt::new(StmtKind::Continue, tok_start);
        }

        if tok_kind == TokenKind::While {
            self.advance();
            let mut s = Stmt::new(StmtKind::While, tok_start);
            s.expr = Some(self.parse_expr());
            s.then_body = self.parse_block();
            return s;
        }

        if tok_kind == TokenKind::Match {
            self.advance();
            let mut s = Stmt::new(StmtKind::Match, tok_start);
            s.expr = Some(self.parse_expr());
            self.expect(TokenKind::LBrace);
            self.skip_semis();
            while !self.check(TokenKind::RBrace) && !self.check(TokenKind::Eof) {
                let (vs, ve) = self.expect(TokenKind::Identifier);
                s.match_variants.push(self.text(vs, ve).to_string());
                self.expect(TokenKind::FatArrow);
                s.match_bodies.push(self.parse_block());
                self.skip_semis();
            }
            self.expect(TokenKind::RBrace);
            return s;
        }

        // Expression statement, assignment, or compound assignment
        let expr = self.parse_expr();

        if self.check(TokenKind::Eq) {
            self.advance();
            let mut s = Stmt::new(StmtKind::Assign, expr.pos);
            if expr.kind == ExprKind::Ident {
                s.var_name = expr.name.clone();
            }
            s.expr = Some(self.parse_expr());
            if expr.kind != ExprKind::Ident {
                s.assign_lhs = Some(expr);
            }
            return s;
        }

        if self.check(TokenKind::PlusEq)
            || self.check(TokenKind::MinusEq)
            || self.check(TokenKind::StarEq)
            || self.check(TokenKind::SlashEq)
        {
            let op_kind = self.peek().kind;
            self.advance();
            let mut s = Stmt::new(StmtKind::CompoundAssign, expr.pos);
            s.var_name = expr.name.clone();
            s.op = op_kind;
            s.expr = Some(self.parse_expr());
            return s;
        }

        let mut s = Stmt::new(StmtKind::ExprStmt, expr.pos);
        s.expr = Some(expr);
        s
    }

    // ---- Top-level ----

    fn parse_enum_def(&mut self) -> EnumDef {
        let (es, _) = self.expect(TokenKind::Enum);
        let (ns, ne) = self.expect(TokenKind::Identifier);
        let name = self.text(ns, ne).to_string();
        self.expect(TokenKind::LBrace);
        self.skip_semis();
        let mut variants = Vec::new();
        while !self.check(TokenKind::RBrace) && !self.check(TokenKind::Eof) {
            let (vs, ve) = self.expect(TokenKind::Identifier);
            variants.push(self.text(vs, ve).to_string());
            self.skip_semis();
            if self.check(TokenKind::Comma) {
                self.advance();
                self.skip_semis();
            }
        }
        self.expect(TokenKind::RBrace);
        EnumDef { name, variants, pos: es }
    }

    fn parse_struct_def(&mut self) -> StructDef {
        let (ss, _) = self.expect(TokenKind::Struct);
        let (ns, ne) = self.expect(TokenKind::Identifier);
        let name = self.text(ns, ne).to_string();
        self.expect(TokenKind::LBrace);
        self.skip_semis();
        let mut fields = Vec::new();
        while !self.check(TokenKind::RBrace) && !self.check(TokenKind::Eof) {
            let (fs, fe) = self.expect(TokenKind::Identifier);
            let field_name = self.text(fs, fe).to_string();
            self.expect(TokenKind::Colon);
            let ty = self.parse_type();
            fields.push(Param { name: field_name, ty });
            // Accept commas or semicolons (newlines) between fields.
            if !self.match_tok(TokenKind::Comma) {
                self.skip_semis();
            }
        }
        self.expect(TokenKind::RBrace);
        StructDef { name, fields, pos: ss }
    }

    fn parse_fn_decl(&mut self) -> FnDecl {
        let (fp, _) = self.expect(TokenKind::Fn);
        let (ns, ne) = self.expect(TokenKind::Identifier);
        let name = self.text(ns, ne).to_string();

        // Generic type parameters
        let mut type_params = Vec::new();
        if self.match_tok(TokenKind::LBracket) {
            if !self.check(TokenKind::RBracket) {
                let (ts, te) = self.expect(TokenKind::Identifier);
                type_params.push(self.text(ts, te).to_string());
                while self.match_tok(TokenKind::Comma) {
                    let (ts, te) = self.expect(TokenKind::Identifier);
                    type_params.push(self.text(ts, te).to_string());
                }
            }
            self.expect(TokenKind::RBracket);
        }

        // Parameters
        self.expect(TokenKind::LParen);
        let mut params = Vec::new();
        if !self.check(TokenKind::RParen) {
            let (ps, pe) = self.expect(TokenKind::Identifier);
            let p_name = self.text(ps, pe).to_string();
            self.expect(TokenKind::Colon);
            let ty = self.parse_type();
            params.push(Param { name: p_name, ty });
            while self.match_tok(TokenKind::Comma) {
                let (ps, pe) = self.expect(TokenKind::Identifier);
                let p_name = self.text(ps, pe).to_string();
                self.expect(TokenKind::Colon);
                let ty = self.parse_type();
                params.push(Param { name: p_name, ty });
            }
        }
        self.expect(TokenKind::RParen);

        let return_type = if self.match_tok(TokenKind::Arrow) {
            self.parse_type()
        } else {
            TypeRef::simple("void")
        };

        let body = self.parse_block();

        FnDecl { name, type_params, params, return_type, body, pos: fp }
    }

    fn parse_test_decl(&mut self) -> TestDecl {
        let (tp, _) = self.expect(TokenKind::Test);
        let (ns, ne) = self.expect(TokenKind::StringLiteral);
        let raw = self.text(ns, ne);
        let name = raw[1..raw.len() - 1].to_string();
        let body = self.parse_block();
        TestDecl { name, body, pos: tp }
    }

    fn parse_module(&mut self) -> Module {
        let mut module = Module {
            enums: Vec::new(),
            structs: Vec::new(),
            functions: Vec::new(),
            tests: Vec::new(),
        };

        self.skip_semis();
        while !self.check(TokenKind::Eof) {
            match self.peek().kind {
                TokenKind::Fn => module.functions.push(self.parse_fn_decl()),
                TokenKind::Struct => module.structs.push(self.parse_struct_def()),
                TokenKind::Enum => module.enums.push(self.parse_enum_def()),
                TokenKind::Test => module.tests.push(self.parse_test_decl()),
                _ => {
                    let s = self.peek().start;
                    let e = self.peek().end;
                    eprintln!(
                        "error: expected 'fn', 'struct', 'enum', or 'test', got '{}'",
                        self.text(s, e)
                    );
                    self.advance();
                }
            }
            self.skip_semis();
        }

        module
    }
}

/// Parse a token stream into a Module.
pub fn parse(source: &str, tokens: &[Token]) -> Module {
    let mut parser = Parser::new(source, tokens);
    parser.parse_module()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner;

    fn parse_source(src: &str) -> Module {
        let tokens = scanner::scan(src);
        parse(src, &tokens)
    }

    #[test]
    fn test_parse_hello() {
        let m = parse_source("fn main() -> i32 {\n    return 42\n}\n");
        assert_eq!(m.functions.len(), 1);
        assert_eq!(m.functions[0].name, "main");
        assert_eq!(m.functions[0].return_type.name, "i32");
        assert_eq!(m.functions[0].body.len(), 1);
        assert_eq!(m.functions[0].body[0].kind, StmtKind::Return);
    }

    #[test]
    fn test_parse_arithmetic() {
        let src = "fn add(a: i32, b: i32) -> i32 {\n    return a + b\n}\n\nfn main() -> i32 {\n    return add(19, 23)\n}\n";
        let m = parse_source(src);
        assert_eq!(m.functions.len(), 2);
        assert_eq!(m.functions[0].name, "add");
        assert_eq!(m.functions[0].params.len(), 2);
        assert_eq!(m.functions[0].params[0].name, "a");
        assert_eq!(m.functions[0].params[0].ty.name, "i32");
        assert_eq!(m.functions[1].name, "main");
    }

    #[test]
    fn test_parse_return_expr() {
        let m = parse_source("fn main() -> i32 {\n    return 42\n}\n");
        let ret = &m.functions[0].body[0];
        assert_eq!(ret.kind, StmtKind::Return);
        let expr = ret.expr.as_ref().unwrap();
        assert_eq!(expr.kind, ExprKind::IntLit);
        assert_eq!(expr.int_val, 42);
    }

    #[test]
    fn test_parse_binop() {
        let m = parse_source("fn main() -> i32 {\n    return 1 + 2\n}\n");
        let ret = &m.functions[0].body[0];
        let expr = ret.expr.as_ref().unwrap();
        assert_eq!(expr.kind, ExprKind::BinOp);
        assert_eq!(expr.op, TokenKind::Plus);
    }

    #[test]
    fn test_parse_call() {
        let m = parse_source(
            "fn add(a: i32, b: i32) -> i32 {\n    return a + b\n}\nfn main() -> i32 {\n    return add(1, 2)\n}\n",
        );
        let ret = &m.functions[1].body[0];
        let expr = ret.expr.as_ref().unwrap();
        assert_eq!(expr.kind, ExprKind::Call);
        assert_eq!(expr.name, "add");
        assert_eq!(expr.args.len(), 2);
    }

    #[test]
    fn test_parse_let() {
        let m = parse_source("fn main() -> i32 {\n    let x: i32 = 10\n    return x\n}\n");
        assert_eq!(m.functions[0].body.len(), 2);
        let let_stmt = &m.functions[0].body[0];
        assert_eq!(let_stmt.kind, StmtKind::Let);
        assert_eq!(let_stmt.var_name, "x");
        assert_eq!(let_stmt.var_type.name, "i32");
    }

    #[test]
    fn test_parse_var_assign() {
        let m = parse_source(
            "fn main() -> i32 {\n    var y: i32 = 20\n    y = y + 12\n    return y\n}\n",
        );
        assert_eq!(m.functions[0].body.len(), 3);
        assert_eq!(m.functions[0].body[0].kind, StmtKind::Var);
        assert_eq!(m.functions[0].body[1].kind, StmtKind::Assign);
    }

    #[test]
    fn test_parse_precedence() {
        // 1 + 2 * 3 should parse as 1 + (2 * 3)
        let m = parse_source("fn main() -> i32 {\n    return 1 + 2 * 3\n}\n");
        let ret = &m.functions[0].body[0];
        let expr = ret.expr.as_ref().unwrap();
        assert_eq!(expr.kind, ExprKind::BinOp);
        assert_eq!(expr.op, TokenKind::Plus);
        let rhs = expr.rhs.as_ref().unwrap();
        assert_eq!(rhs.kind, ExprKind::BinOp);
        assert_eq!(rhs.op, TokenKind::Star);
    }

    #[test]
    fn test_parse_void_return() {
        let m = parse_source("fn noop() {\n    return\n}\n");
        assert_eq!(m.functions[0].return_type.name, "void");
        assert_eq!(m.functions[0].body[0].kind, StmtKind::Return);
        assert!(m.functions[0].body[0].expr.is_none());
    }
}
