//===- Parser.cpp - ac language parser -------------------------*- C++ -*-===//
//
// Recursive descent with Zig-style precedence climbing.
// Reference: Go src/go/parser/parser.go, Zig lib/std/zig/Parse.zig
//
//===----------------------------------------------------------------------===//
#include "Parser.h"

#include "llvm/Support/raw_ostream.h"

namespace ac {

// Zig pattern: operator precedence table.
static int precedence(TokenKind kind) {
  switch (kind) {
  case TokenKind::Catch:                                 return 5;
  case TokenKind::Orelse:                                return 5;
  case TokenKind::PipePipe:                              return 10;
  case TokenKind::AmpAmp:                                return 20;
  case TokenKind::EqEq: case TokenKind::BangEq:         return 30;
  case TokenKind::Less: case TokenKind::LessEq:
  case TokenKind::Greater: case TokenKind::GreaterEq:    return 40;
  case TokenKind::Pipe:                                  return 50;
  case TokenKind::Caret:                                 return 60;
  case TokenKind::Amp:                                   return 70;
  case TokenKind::Shl: case TokenKind::Shr:              return 80;
  case TokenKind::Plus: case TokenKind::Minus:           return 90;
  case TokenKind::Star: case TokenKind::Slash:
  case TokenKind::Percent:                               return 100;
  default:                                               return 0;
  }
}

// Can this token start a type expression?
// Used to disambiguate generic call name[Type](...) from array index name[expr].
static bool isTypeStart(TokenKind kind) {
  switch (kind) {
  case TokenKind::I8:  case TokenKind::I16:
  case TokenKind::I32: case TokenKind::I64:
  case TokenKind::U8:  case TokenKind::U16:
  case TokenKind::U32: case TokenKind::U64:
  case TokenKind::F32: case TokenKind::F64:
  case TokenKind::Bool: case TokenKind::Void:
  case TokenKind::Identifier:  // User-defined types: Point, List
  case TokenKind::Question:    // ?T optional
  case TokenKind::Star:        // *T pointer
    return true;
  default:
    return false;
  }
}

// Is this token a type keyword? (used to disambiguate Ident vs type)
static bool isTypeKeyword(TokenKind kind) {
  switch (kind) {
  case TokenKind::I8:  case TokenKind::I16:
  case TokenKind::I32: case TokenKind::I64:
  case TokenKind::U8:  case TokenKind::U16:
  case TokenKind::U32: case TokenKind::U64:
  case TokenKind::F32: case TokenKind::F64:
  case TokenKind::Bool: case TokenKind::Void:
    return true;
  default:
    return false;
  }
}

class ParserImpl {
  llvm::StringRef source_;
  const llvm::SmallVector<Token> &tokens_;
  size_t pos_ = 0;

  const Token &peek() const { return tokens_[pos_]; }
  const Token &peekAt(size_t offset) const { return tokens_[pos_ + offset]; }
  const Token &advance() { return tokens_[pos_++]; }
  bool check(TokenKind k) const { return peek().kind == k; }

  bool match(TokenKind k) {
    if (check(k)) { advance(); return true; }
    return false;
  }

  llvm::StringRef tokenText(const Token &tok) const {
    return source_.substr(tok.start, tok.end - tok.start);
  }

  const Token &expect(TokenKind k) {
    if (!check(k)) {
      llvm::errs() << "error: expected token " << static_cast<int>(k)
                    << " got '" << tokenText(peek()) << "'\n";
    }
    return advance();
  }

  void skipSemis() { while (check(TokenKind::Semicolon)) advance(); }

  // ---- Type ----
  // Handles: i32, Point (identifier), [N]T (array), ?T (optional)
  TypeRef parseType() {
    // Pointer type: *T
    if (check(TokenKind::Star)) {
      advance(); // *
      auto inner = parseType();
      inner.isPointer = true;
      return inner;
    }
    // Optional type: ?T
    if (check(TokenKind::Question)) {
      advance(); // ?
      auto inner = parseType();
      inner.isOptional = true;
      return inner;
    }
    // Array type [N]T or slice type []T
    if (check(TokenKind::LBracket)) {
      advance(); // [
      // Slice type: []T (no length)
      if (check(TokenKind::RBracket)) {
        advance(); // ]
        auto elemType = parseType();
        TypeRef t;
        t.name = elemType.name;
        t.isSlice = true;
        return t;
      }
      // Array type: [N]T
      auto &lenTok = expect(TokenKind::IntLiteral);
      int64_t len = std::stoll(std::string(tokenText(lenTok)));
      expect(TokenKind::RBracket);
      auto elemType = parseType();
      TypeRef t;
      t.name = elemType.name;
      t.isArray = true;
      t.arrayLen = len;
      return t;
    }
    auto &tok = advance();
    TypeRef t{tokenText(tok), 0, false, false, false};
    // Error union type suffix: T!error
    if (check(TokenKind::Bang) && pos_ + 1 < tokens_.size() &&
        peekAt(1).kind == TokenKind::Identifier &&
        tokenText(peekAt(1)) == "error") {
      advance(); // !
      advance(); // error
      t.isErrorUnion = true;
    }
    return t;
  }

  // ---- Expressions ----
  ExprPtr parsePrimary() {
    auto &tok = peek();

    if (tok.kind == TokenKind::IntLiteral) {
      advance();
      auto e = std::make_unique<Expr>();
      e->kind = ExprKind::IntLit;
      e->pos = tok.start;
      llvm::StringRef txt = tokenText(tok);
      std::string clean;
      for (char c : txt)
        if (c != '_') clean += c;
      e->intVal = std::stoll(clean);
      return e;
    }

    if (tok.kind == TokenKind::FloatLiteral) {
      advance();
      auto e = std::make_unique<Expr>();
      e->kind = ExprKind::FloatLit;
      e->pos = tok.start;
      std::string clean;
      for (char c : tokenText(tok))
        if (c != '_') clean += c;
      e->floatVal = std::stod(clean);
      return e;
    }

    if (tok.kind == TokenKind::BoolTrue || tok.kind == TokenKind::BoolFalse) {
      advance();
      auto e = std::make_unique<Expr>();
      e->kind = ExprKind::BoolLit;
      e->pos = tok.start;
      e->boolVal = (tok.kind == TokenKind::BoolTrue);
      return e;
    }

    if (tok.kind == TokenKind::StringLiteral) {
      advance();
      auto e = std::make_unique<Expr>();
      e->kind = ExprKind::StringLit;
      e->pos = tok.start;
      // Strip quotes
      auto raw = tokenText(tok);
      e->strVal = raw.substr(1, raw.size() - 2);
      return e;
    }

    // Null literal
    if (tok.kind == TokenKind::Null) {
      advance();
      auto e = std::make_unique<Expr>();
      e->kind = ExprKind::NullLit;
      e->pos = tok.start;
      return e;
    }

    // Array literal: [expr, expr, ...]
    if (tok.kind == TokenKind::LBracket) {
      advance(); // [
      auto e = std::make_unique<Expr>();
      e->kind = ExprKind::ArrayLit;
      e->pos = tok.start;
      if (!check(TokenKind::RBracket)) {
        e->args.push_back(parseExpr());
        while (match(TokenKind::Comma))
          e->args.push_back(parseExpr());
      }
      expect(TokenKind::RBracket);
      return e;
    }

    if (tok.kind == TokenKind::Identifier) {
      advance();
      auto name = tokenText(tok);

      // Struct literal: Name { field: expr, ... }
      // Disambiguate from block: struct lit requires ident before {
      if (check(TokenKind::LBrace)) {
        // Lookahead: if next is `ident :` then it's a struct literal
        // Otherwise it could be a block (but blocks only appear in statements)
        if (pos_ + 1 < tokens_.size() &&
            peekAt(1).kind == TokenKind::Identifier &&
            pos_ + 2 < tokens_.size() &&
            peekAt(2).kind == TokenKind::Colon) {
          advance(); // {
          auto e = std::make_unique<Expr>();
          e->kind = ExprKind::StructLit;
          e->pos = tok.start;
          e->name = name;
          if (!check(TokenKind::RBrace)) {
            FieldInit fi;
            fi.name = tokenText(expect(TokenKind::Identifier));
            expect(TokenKind::Colon);
            fi.value = parseExpr();
            e->fields.push_back(std::move(fi));
            while (match(TokenKind::Comma)) {
              skipSemis();
              if (check(TokenKind::RBrace)) break;
              FieldInit fi2;
              fi2.name = tokenText(expect(TokenKind::Identifier));
              expect(TokenKind::Colon);
              fi2.value = parseExpr();
              e->fields.push_back(std::move(fi2));
            }
          }
          skipSemis();
          expect(TokenKind::RBrace);
          return e;
        }
      }

      // Generic call: identity[i32](args...) — brackets contain type names
      // Disambiguate from array index: arr[0] — brackets contain expressions
      // Check: name[TypeKeyword...](  vs  name[expr]
      if (check(TokenKind::LBracket) && isTypeStart(peekAt(1).kind)) {
        advance(); // consume [
        auto e = std::make_unique<Expr>();
        e->kind = ExprKind::Call;
        e->pos = tok.start;
        e->name = name;
        if (!check(TokenKind::RBracket)) {
          e->typeArgs.push_back(parseType());
          while (match(TokenKind::Comma))
            e->typeArgs.push_back(parseType());
        }
        expect(TokenKind::RBracket);
        expect(TokenKind::LParen);
        if (!check(TokenKind::RParen)) {
          e->args.push_back(parseExpr());
          while (match(TokenKind::Comma))
            e->args.push_back(parseExpr());
        }
        expect(TokenKind::RParen);
        return e;
      }

      // Function call: ident(args...)
      if (check(TokenKind::LParen)) {
        advance(); // (
        auto e = std::make_unique<Expr>();
        e->kind = ExprKind::Call;
        e->pos = tok.start;
        e->name = name;
        if (!check(TokenKind::RParen)) {
          e->args.push_back(parseExpr());
          while (match(TokenKind::Comma))
            e->args.push_back(parseExpr());
        }
        expect(TokenKind::RParen);
        return e;
      }

      auto e = std::make_unique<Expr>();
      e->kind = ExprKind::Ident;
      e->pos = tok.start;
      e->name = name;
      return e;
    }

    if (tok.kind == TokenKind::LParen) {
      advance(); // (
      auto e = parseExpr();
      expect(TokenKind::RParen);
      return e;
    }

    // try expr — unwrap error union or propagate
    if (tok.kind == TokenKind::Try) {
      advance();
      auto e = std::make_unique<Expr>();
      e->kind = ExprKind::TryUnwrap;
      e->pos = tok.start;
      e->lhs = parsePrimary();
      e->lhs = parsePostfix(std::move(e->lhs));
      return e;
    }

    // Address-of: &expr
    if (tok.kind == TokenKind::Amp) {
      advance();
      auto e = std::make_unique<Expr>();
      e->kind = ExprKind::AddrOf;
      e->pos = tok.start;
      e->lhs = parsePrimary();
      return e;
    }

    // Dereference: *expr
    if (tok.kind == TokenKind::Star) {
      advance();
      auto e = std::make_unique<Expr>();
      e->kind = ExprKind::Deref;
      e->pos = tok.start;
      e->lhs = parsePrimary();
      e->lhs = parsePostfix(std::move(e->lhs));
      return e;
    }

    // Unary operators: -, !, ~
    if (tok.kind == TokenKind::Minus || tok.kind == TokenKind::Bang ||
        tok.kind == TokenKind::Tilde) {
      advance();
      auto e = std::make_unique<Expr>();
      e->kind = ExprKind::UnaryOp;
      e->pos = tok.start;
      e->op = tok.kind;
      e->rhs = parsePrimary();
      return e;
    }

    llvm::errs() << "error: unexpected token '" << tokenText(tok) << "'\n";
    advance();
    return std::make_unique<Expr>();
  }

  // Parse postfix: .field, [index], and ! (force unwrap)
  ExprPtr parsePostfix(ExprPtr base) {
    while (true) {
      // Force unwrap: expr!
      if (check(TokenKind::Bang)) {
        // Only treat ! as postfix if not followed by = (which is !=)
        if (pos_ + 1 < tokens_.size() &&
            peekAt(1).kind == TokenKind::Eq)
          break;  // It's != operator, not force unwrap
        auto &bangTok = advance();
        auto e = std::make_unique<Expr>();
        e->kind = ExprKind::ForceUnwrap;
        e->pos = bangTok.start;
        e->lhs = std::move(base);
        base = std::move(e);
        continue;
      }
      if (check(TokenKind::Dot)) {
        advance(); // .
        auto &fieldTok = expect(TokenKind::Identifier);
        auto e = std::make_unique<Expr>();
        e->kind = ExprKind::FieldAccess;
        e->pos = fieldTok.start;
        e->name = tokenText(fieldTok);
        e->lhs = std::move(base);
        base = std::move(e);
        continue;
      }
      if (check(TokenKind::LBracket)) {
        advance(); // [
        auto idx = parseExpr();
        // Slice from array: arr[lo..hi]
        if (check(TokenKind::DotDot)) {
          advance(); // ..
          auto hi = parseExpr();
          expect(TokenKind::RBracket);
          auto e = std::make_unique<Expr>();
          e->kind = ExprKind::SliceFrom;
          e->pos = base->pos;
          e->lhs = std::move(base);
          e->rhs = std::move(hi);
          e->args.push_back(std::move(idx)); // lo stored in args[0]
          base = std::move(e);
          continue;
        }
        expect(TokenKind::RBracket);
        auto e = std::make_unique<Expr>();
        e->kind = ExprKind::Index;
        e->pos = base->pos;
        e->lhs = std::move(base);
        e->rhs = std::move(idx);
        base = std::move(e);
        continue;
      }
      // Cast: expr as Type
      if (check(TokenKind::As)) {
        auto &asTok = advance();
        auto e = std::make_unique<Expr>();
        e->kind = ExprKind::CastAs;
        e->pos = asTok.start;
        e->lhs = std::move(base);
        e->castType = parseType();
        base = std::move(e);
        continue;
      }
      break;
    }
    return base;
  }

  // Precedence climbing (Zig pattern).
  ExprPtr parseBinaryExpr(ExprPtr lhs, int minPrec) {
    while (true) {
      int prec = precedence(peek().kind);
      if (prec < minPrec)
        break;
      auto &opTok = advance();
      auto rhs = parsePrimary();
      rhs = parsePostfix(std::move(rhs));
      // Right-associate if next op has higher precedence
      while (precedence(peek().kind) > prec)
        rhs = parseBinaryExpr(std::move(rhs), prec + 1);
      auto binop = std::make_unique<Expr>();
      binop->kind = ExprKind::BinOp;
      binop->pos = opTok.start;
      binop->op = opTok.kind;
      binop->lhs = std::move(lhs);
      binop->rhs = std::move(rhs);
      lhs = std::move(binop);
    }
    return lhs;
  }

  ExprPtr parseExpr() {
    auto lhs = parsePrimary();
    lhs = parsePostfix(std::move(lhs));
    return parseBinaryExpr(std::move(lhs), 1);
  }

  // ---- Statements ----
  llvm::SmallVector<StmtPtr> parseBlock() {
    expect(TokenKind::LBrace);
    skipSemis();
    llvm::SmallVector<StmtPtr> stmts;
    while (!check(TokenKind::RBrace) && !check(TokenKind::Eof)) {
      stmts.push_back(parseStmt());
      skipSemis();
    }
    expect(TokenKind::RBrace);
    return stmts;
  }

  StmtPtr parseStmt() {
    auto &tok = peek();

    // return [expr]
    if (tok.kind == TokenKind::Return) {
      advance();
      auto s = std::make_unique<Stmt>();
      s->kind = StmtKind::Return;
      s->pos = tok.start;
      if (!check(TokenKind::Semicolon) && !check(TokenKind::RBrace) &&
          !check(TokenKind::Eof))
        s->expr = parseExpr();
      return s;
    }

    // let name: type = expr
    if (tok.kind == TokenKind::Let || tok.kind == TokenKind::Var) {
      bool isMut = (tok.kind == TokenKind::Var);
      advance();
      auto s = std::make_unique<Stmt>();
      s->kind = isMut ? StmtKind::Var : StmtKind::Let;
      s->pos = tok.start;
      s->varName = tokenText(expect(TokenKind::Identifier));
      if (match(TokenKind::Colon))
        s->varType = parseType();
      expect(TokenKind::Eq);
      s->expr = parseExpr();
      return s;
    }

    // if expr { ... } [else { ... }]
    if (tok.kind == TokenKind::If) {
      advance();
      auto s = std::make_unique<Stmt>();
      s->kind = StmtKind::If;
      s->pos = tok.start;
      s->expr = parseExpr();
      s->thenBody = parseBlock();
      skipSemis();
      if (match(TokenKind::Else)) {
        if (check(TokenKind::If)) {
          s->elseBody.push_back(parseStmt());
        } else {
          s->elseBody = parseBlock();
        }
      }
      return s;
    }

    // assert(expr)
    if (tok.kind == TokenKind::Assert) {
      advance();
      auto s = std::make_unique<Stmt>();
      s->kind = StmtKind::Assert;
      s->pos = tok.start;
      expect(TokenKind::LParen);
      s->expr = parseExpr();
      expect(TokenKind::RParen);
      return s;
    }

    // for name in lo..hi { ... }
    if (tok.kind == TokenKind::For) {
      advance();
      auto s = std::make_unique<Stmt>();
      s->kind = StmtKind::For;
      s->pos = tok.start;
      s->varName = tokenText(expect(TokenKind::Identifier));
      expect(TokenKind::In);
      s->expr = parseExpr();      // lo
      expect(TokenKind::DotDot);
      s->rangeEnd = parseExpr();  // hi
      s->thenBody = parseBlock();
      return s;
    }

    // break
    if (tok.kind == TokenKind::Break) {
      advance();
      auto s = std::make_unique<Stmt>();
      s->kind = StmtKind::Break;
      s->pos = tok.start;
      return s;
    }

    // continue
    if (tok.kind == TokenKind::Continue) {
      advance();
      auto s = std::make_unique<Stmt>();
      s->kind = StmtKind::Continue;
      s->pos = tok.start;
      return s;
    }

    // while expr { ... }
    if (tok.kind == TokenKind::While) {
      advance();
      auto s = std::make_unique<Stmt>();
      s->kind = StmtKind::While;
      s->pos = tok.start;
      s->expr = parseExpr();
      s->thenBody = parseBlock();
      return s;
    }

    // match expr { Variant1 => { ... }, Variant2 => { ... } }
    if (tok.kind == TokenKind::Match) {
      advance();
      auto s = std::make_unique<Stmt>();
      s->kind = StmtKind::Match;
      s->pos = tok.start;
      s->expr = parseExpr();
      expect(TokenKind::LBrace);
      skipSemis();
      while (!check(TokenKind::RBrace) && !check(TokenKind::Eof)) {
        auto variantName = tokenText(expect(TokenKind::Identifier));
        s->matchVariants.push_back(variantName);
        expect(TokenKind::FatArrow); // =>
        s->matchBodies.push_back(parseBlock());
        skipSemis();
      }
      expect(TokenKind::RBrace);
      return s;
    }

    // Expression statement, assignment, or compound assignment
    auto expr = parseExpr();

    // Assignment: lhs = expr (lhs can be ident or field access)
    if (check(TokenKind::Eq)) {
      advance();
      auto s = std::make_unique<Stmt>();
      s->kind = StmtKind::Assign;
      s->pos = expr->pos;
      // For simple ident assignment, store name for backwards compat
      if (expr->kind == ExprKind::Ident)
        s->varName = expr->name;
      // Store the full LHS expression for field/index assignment
      s->expr = parseExpr();
      // Stash the LHS in thenBody[0] via a wrapper if it's complex
      if (expr->kind != ExprKind::Ident) {
        auto wrapper = std::make_unique<Stmt>();
        wrapper->kind = StmtKind::ExprStmt;
        wrapper->expr = std::move(expr);
        s->thenBody.push_back(std::move(wrapper));
      }
      return s;
    }

    // Compound assignment: ident += expr
    if (check(TokenKind::PlusEq) || check(TokenKind::MinusEq) ||
        check(TokenKind::StarEq) || check(TokenKind::SlashEq)) {
      auto &opTok = advance();
      auto s = std::make_unique<Stmt>();
      s->kind = StmtKind::CompoundAssign;
      s->pos = expr->pos;
      s->varName = expr->name;
      s->op = opTok.kind;
      s->expr = parseExpr();
      return s;
    }

    // Expression statement
    auto s = std::make_unique<Stmt>();
    s->kind = StmtKind::ExprStmt;
    s->pos = expr->pos;
    s->expr = std::move(expr);
    return s;
  }

  // ---- Top-level ----
  EnumDef parseEnumDef() {
    auto &enumTok = expect(TokenKind::Enum);
    EnumDef ed;
    ed.pos = enumTok.start;
    ed.name = tokenText(expect(TokenKind::Identifier));
    expect(TokenKind::LBrace);
    skipSemis();
    while (!check(TokenKind::RBrace) && !check(TokenKind::Eof)) {
      ed.variants.push_back(tokenText(expect(TokenKind::Identifier)));
      skipSemis();
      // Allow trailing comma
      if (check(TokenKind::Comma)) { advance(); skipSemis(); }
    }
    expect(TokenKind::RBrace);
    return ed;
  }

  StructDef parseStructDef() {
    auto &structTok = expect(TokenKind::Struct);
    StructDef sd;
    sd.pos = structTok.start;
    sd.name = tokenText(expect(TokenKind::Identifier));
    expect(TokenKind::LBrace);
    skipSemis();
    while (!check(TokenKind::RBrace) && !check(TokenKind::Eof)) {
      Param field;
      field.name = tokenText(expect(TokenKind::Identifier));
      expect(TokenKind::Colon);
      field.type = parseType();
      sd.fields.push_back(field);
      skipSemis();
    }
    expect(TokenKind::RBrace);
    return sd;
  }

  FnDecl parseFnDecl() {
    auto &fnTok = expect(TokenKind::Fn);
    FnDecl fn;
    fn.pos = fnTok.start;
    fn.name = tokenText(expect(TokenKind::Identifier));

    // Generic type parameters: fn identity[T](x: T) -> T
    if (match(TokenKind::LBracket)) {
      if (!check(TokenKind::RBracket)) {
        fn.typeParams.push_back(tokenText(expect(TokenKind::Identifier)));
        while (match(TokenKind::Comma))
          fn.typeParams.push_back(tokenText(expect(TokenKind::Identifier)));
      }
      expect(TokenKind::RBracket);
    }

    // Parameters
    expect(TokenKind::LParen);
    if (!check(TokenKind::RParen)) {
      Param p;
      p.name = tokenText(expect(TokenKind::Identifier));
      expect(TokenKind::Colon);
      p.type = parseType();
      fn.params.push_back(p);
      while (match(TokenKind::Comma)) {
        Param p2;
        p2.name = tokenText(expect(TokenKind::Identifier));
        expect(TokenKind::Colon);
        p2.type = parseType();
        fn.params.push_back(p2);
      }
    }
    expect(TokenKind::RParen);

    // Return type
    if (match(TokenKind::Arrow))
      fn.returnType = parseType();
    else
      fn.returnType = TypeRef{"void", 0, false, false, false};

    fn.body = parseBlock();
    return fn;
  }

  TestDecl parseTestDecl() {
    auto &testTok = expect(TokenKind::Test);
    TestDecl td;
    td.pos = testTok.start;
    auto &nameTok = expect(TokenKind::StringLiteral);
    auto raw = tokenText(nameTok);
    td.name = raw.substr(1, raw.size() - 2); // strip quotes
    td.body = parseBlock();
    return td;
  }

public:
  ParserImpl(llvm::StringRef source, const llvm::SmallVector<Token> &tokens)
      : source_(source), tokens_(tokens) {}

  Module parseModule() {
    Module mod;
    skipSemis();
    while (!check(TokenKind::Eof)) {
      if (check(TokenKind::Fn)) {
        mod.functions.push_back(parseFnDecl());
      } else if (check(TokenKind::Struct)) {
        mod.structs.push_back(parseStructDef());
      } else if (check(TokenKind::Enum)) {
        mod.enums.push_back(parseEnumDef());
      } else if (check(TokenKind::Test)) {
        mod.tests.push_back(parseTestDecl());
      } else {
        llvm::errs() << "error: expected 'fn', 'struct', 'enum', or 'test', got '"
                      << tokenText(peek()) << "'\n";
        advance();
      }
      skipSemis();
    }
    return mod;
  }
};

Module parse(llvm::StringRef source, const llvm::SmallVector<Token> &tokens) {
  ParserImpl parser(source, tokens);
  return parser.parseModule();
}

} // namespace ac
