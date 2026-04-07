//===- Parser.h - ac language parser ---------------------------*- C++ -*-===//
//
// Recursive descent parser with Zig-style precedence climbing.
// Reference: Go src/go/parser/parser.go, Zig lib/std/zig/Parse.zig
//
//===----------------------------------------------------------------------===//
#ifndef COTAC_PARSER_H
#define COTAC_PARSER_H

#include "Scanner.h"
#include <memory>

namespace ac {

struct Expr;
struct Stmt;
using ExprPtr = std::unique_ptr<Expr>;
using StmtPtr = std::unique_ptr<Stmt>;

struct TypeRef {
  llvm::StringRef name;    // "i32", "void", "Point", etc.
  int64_t arrayLen = 0;    // >0 for [N]T array types
  bool isArray = false;    // true for [N]T
  bool isOptional = false;   // true for ?T
  bool isErrorUnion = false; // true for T!error
  bool isPointer = false;    // true for *T
  bool isSlice = false;      // true for []T
};

struct Param {
  llvm::StringRef name;
  TypeRef type;
};

// Struct field for struct literal: Name { field: expr, ... }
struct FieldInit {
  llvm::StringRef name;
  ExprPtr value;
};

enum class ExprKind {
  IntLit, FloatLit, BoolLit, StringLit, NullLit, Ident, BinOp, UnaryOp, Call,
  StructLit,    // Point { x: 1, y: 2 }
  FieldAccess,  // expr.field
  ArrayLit,     // [1, 2, 3]
  Index,        // expr[expr]
  ForceUnwrap,  // expr!
  TryUnwrap,    // try expr
  AddrOf,       // &x
  Deref,        // *p
  CastAs,       // expr as Type
  SliceFrom     // arr[lo..hi]
};

struct Expr {
  ExprKind kind;
  size_t pos;

  int64_t intVal = 0;
  double floatVal = 0.0;
  bool boolVal = false;
  llvm::StringRef name;      // Ident, Call, FieldAccess, StructLit
  llvm::StringRef strVal;    // StringLit (without quotes)
  TokenKind op = TokenKind::Invalid;
  ExprPtr lhs, rhs;          // BinOp, Index (lhs=base, rhs=index)
  llvm::SmallVector<ExprPtr> args;       // Call, ArrayLit
  llvm::SmallVector<FieldInit> fields;   // StructLit
  TypeRef castType;                      // CastAs target type
};

enum class StmtKind {
  Return, ExprStmt, If, While, For, Break, Continue,
  Let, Var, Assign, CompoundAssign, Assert
};

struct Stmt {
  StmtKind kind;
  size_t pos;
  ExprPtr expr;
  ExprPtr rangeEnd;  // For loop upper bound (for i in lo..hi)
  llvm::SmallVector<StmtPtr> thenBody;
  llvm::SmallVector<StmtPtr> elseBody;
  llvm::StringRef varName;
  TypeRef varType;
  TokenKind op = TokenKind::Invalid;
};

struct StructDef {
  llvm::StringRef name;
  llvm::SmallVector<Param> fields; // reuse Param: name + type
  size_t pos;
};

struct FnDecl {
  llvm::StringRef name;
  llvm::SmallVector<Param> params;
  TypeRef returnType;
  llvm::SmallVector<StmtPtr> body;
  size_t pos;
};

struct TestDecl {
  llvm::StringRef name;          // test "name" → "name"
  llvm::SmallVector<StmtPtr> body;
  size_t pos;
};

struct Module {
  llvm::SmallVector<StructDef> structs;
  llvm::SmallVector<FnDecl> functions;
  llvm::SmallVector<TestDecl> tests;
};

Module parse(llvm::StringRef source, const llvm::SmallVector<Token> &tokens);

} // namespace ac

#endif // COTAC_PARSER_H
