//===- Scanner.h - ac language lexer ---------------------------*- C++ -*-===//
//
// Zig Tokenizer state machine + Go insertSemi pattern.
// Reference: Zig lib/std/zig/Tokenizer.zig, Go src/go/scanner/scanner.go
//
//===----------------------------------------------------------------------===//
#ifndef COTAC_SCANNER_H
#define COTAC_SCANNER_H

#include "llvm/ADT/StringRef.h"
#include "llvm/ADT/SmallVector.h"

namespace ac {

enum class TokenKind {
  // Sentinel
  Eof,
  Invalid,

  // Literals
  Identifier,
  IntLiteral,
  FloatLiteral,
  StringLiteral,
  BoolTrue,
  BoolFalse,

  // Keywords
  Fn,
  Return,
  Let,
  Var,
  If,
  Else,
  While,
  For,
  In,
  Break,
  Continue,
  Test,
  Assert,
  Null,
  Orelse,
  Struct,
  Try,
  Catch,
  As,

  // Types
  I8, I16, I32, I64,
  U8, U16, U32, U64,
  F32, F64,
  Bool,
  Void,

  // Punctuation
  LParen,    // (
  RParen,    // )
  LBrace,    // {
  RBrace,    // }
  LBracket,  // [
  RBracket,  // ]
  Comma,     // ,
  Colon,     // :
  Semicolon, // ; (synthetic — Go pattern)
  Dot,       // .

  // Operators
  Plus,      // +
  Minus,     // -
  Star,      // *
  Slash,     // /
  Percent,   // %
  Amp,       // &
  Pipe,      // |
  Caret,     // ^
  Tilde,     // ~
  Bang,      // !
  Eq,        // =
  Less,      // <
  Greater,   // >

  // Multi-char operators
  Arrow,     // ->
  EqEq,      // ==
  BangEq,    // !=
  LessEq,    // <=
  GreaterEq, // >=
  AmpAmp,    // &&
  PipePipe,  // ||
  PlusEq,    // +=
  MinusEq,   // -=
  StarEq,    // *=
  SlashEq,   // /=
  Shl,       // <<
  Shr,       // >>
  DotDot,    // ..
  Question,  // ?
};

struct Token {
  TokenKind kind;
  size_t start;
  size_t end;
};

class Scanner {
public:
  explicit Scanner(llvm::StringRef source);

  Token next();
  llvm::StringRef text(const Token &tok) const;

private:
  llvm::StringRef source_;
  size_t index_ = 0;
  bool insertSemi_ = false;

  static bool triggersSemicolon(TokenKind kind);
};

llvm::SmallVector<Token> scanAll(llvm::StringRef source);

} // namespace ac

#endif // COTAC_SCANNER_H
