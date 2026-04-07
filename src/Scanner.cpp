//===- Scanner.cpp - ac language lexer -------------------------*- C++ -*-===//
//
// Zig Tokenizer state machine + Go insertSemi pattern.
// Reference: Zig lib/std/zig/Tokenizer.zig, Go src/go/scanner/scanner.go
//
//===----------------------------------------------------------------------===//
#include "Scanner.h"

#include "llvm/ADT/StringSwitch.h"

namespace ac {

Scanner::Scanner(llvm::StringRef source) : source_(source) {}

llvm::StringRef Scanner::text(const Token &tok) const {
  return source_.substr(tok.start, tok.end - tok.start);
}

// Go pattern: these tokens trigger semicolon insertion before next newline.
bool Scanner::triggersSemicolon(TokenKind kind) {
  switch (kind) {
  case TokenKind::Identifier:
  case TokenKind::IntLiteral:
  case TokenKind::FloatLiteral:
  case TokenKind::StringLiteral:
  case TokenKind::BoolTrue:
  case TokenKind::BoolFalse:
  case TokenKind::Null:
  case TokenKind::Return:
  case TokenKind::Break:
  case TokenKind::Continue:
  // Type keywords trigger semi (e.g., "-> i32\n")
  case TokenKind::I8: case TokenKind::I16:
  case TokenKind::I32: case TokenKind::I64:
  case TokenKind::U8: case TokenKind::U16:
  case TokenKind::U32: case TokenKind::U64:
  case TokenKind::F32: case TokenKind::F64:
  case TokenKind::Bool: case TokenKind::Void:
  // Closing delimiters
  case TokenKind::RParen:
  case TokenKind::RBrace:
  case TokenKind::RBracket:
    return true;
  default:
    return false;
  }
}

static TokenKind lookupKeyword(llvm::StringRef word) {
  return llvm::StringSwitch<TokenKind>(word)
      .Case("fn", TokenKind::Fn)
      .Case("return", TokenKind::Return)
      .Case("let", TokenKind::Let)
      .Case("var", TokenKind::Var)
      .Case("if", TokenKind::If)
      .Case("else", TokenKind::Else)
      .Case("while", TokenKind::While)
      .Case("for", TokenKind::For)
      .Case("in", TokenKind::In)
      .Case("break", TokenKind::Break)
      .Case("continue", TokenKind::Continue)
      .Case("test", TokenKind::Test)
      .Case("assert", TokenKind::Assert)
      .Case("true", TokenKind::BoolTrue)
      .Case("false", TokenKind::BoolFalse)
      .Case("null", TokenKind::Null)
      .Case("orelse", TokenKind::Orelse)
      .Case("struct", TokenKind::Struct)
      .Case("i8", TokenKind::I8)
      .Case("i16", TokenKind::I16)
      .Case("i32", TokenKind::I32)
      .Case("i64", TokenKind::I64)
      .Case("u8", TokenKind::U8)
      .Case("u16", TokenKind::U16)
      .Case("u32", TokenKind::U32)
      .Case("u64", TokenKind::U64)
      .Case("f32", TokenKind::F32)
      .Case("f64", TokenKind::F64)
      .Case("bool", TokenKind::Bool)
      .Case("void", TokenKind::Void)
      .Default(TokenKind::Identifier);
}

Token Scanner::next() {
  // Skip whitespace. Go pattern: stop at newlines when insertSemi is set.
  while (index_ < source_.size()) {
    char c = source_[index_];
    if (c == ' ' || c == '\t' || c == '\r') {
      index_++;
      continue;
    }
    if (c == '\n') {
      index_++;
      if (insertSemi_) {
        insertSemi_ = false;
        return Token{TokenKind::Semicolon, index_ - 1, index_};
      }
      continue;
    }
    break;
  }

  // EOF — Go pattern: also produces semicolon if pending.
  if (index_ >= source_.size()) {
    if (insertSemi_) {
      insertSemi_ = false;
      return Token{TokenKind::Semicolon, index_, index_};
    }
    return Token{TokenKind::Eof, index_, index_};
  }

  size_t start = index_;
  char c = source_[index_];

  // Line comments: // ...
  if (c == '/' && index_ + 1 < source_.size() && source_[index_ + 1] == '/') {
    index_ += 2;
    while (index_ < source_.size() && source_[index_] != '\n')
      index_++;
    return next(); // Comments don't produce tokens (Zig pattern).
  }

  // Identifiers and keywords
  if ((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || c == '_') {
    index_++;
    while (index_ < source_.size()) {
      char ch = source_[index_];
      if ((ch >= 'a' && ch <= 'z') || (ch >= 'A' && ch <= 'Z') ||
          (ch >= '0' && ch <= '9') || ch == '_')
        index_++;
      else
        break;
    }
    auto word = source_.substr(start, index_ - start);
    auto kind = lookupKeyword(word);
    insertSemi_ = triggersSemicolon(kind);
    return Token{kind, start, index_};
  }

  // Numbers: int or float
  if (c >= '0' && c <= '9') {
    index_++;
    while (index_ < source_.size() &&
           ((source_[index_] >= '0' && source_[index_] <= '9') ||
            source_[index_] == '_'))
      index_++;
    auto kind = TokenKind::IntLiteral;
    // Check for decimal point followed by digit → float
    if (index_ < source_.size() && source_[index_] == '.' &&
        index_ + 1 < source_.size() && source_[index_ + 1] >= '0' &&
        source_[index_ + 1] <= '9') {
      index_++;
      while (index_ < source_.size() &&
             ((source_[index_] >= '0' && source_[index_] <= '9') ||
              source_[index_] == '_'))
        index_++;
      kind = TokenKind::FloatLiteral;
    }
    insertSemi_ = true;
    return Token{kind, start, index_};
  }

  // String literals
  if (c == '"') {
    index_++;
    while (index_ < source_.size() && source_[index_] != '"') {
      if (source_[index_] == '\\' && index_ + 1 < source_.size())
        index_++;
      index_++;
    }
    if (index_ < source_.size())
      index_++;
    insertSemi_ = true;
    return Token{TokenKind::StringLiteral, start, index_};
  }

  // Operators and punctuation
  index_++;
  TokenKind kind = TokenKind::Invalid;
  switch (c) {
  case '-':
    if (index_ < source_.size() && source_[index_] == '>') {
      index_++;
      kind = TokenKind::Arrow;
    } else if (index_ < source_.size() && source_[index_] == '=') {
      index_++;
      kind = TokenKind::MinusEq;
    } else {
      kind = TokenKind::Minus;
    }
    break;
  case '=':
    if (index_ < source_.size() && source_[index_] == '=') {
      index_++;
      kind = TokenKind::EqEq;
    } else {
      kind = TokenKind::Eq;
    }
    break;
  case '!':
    if (index_ < source_.size() && source_[index_] == '=') {
      index_++;
      kind = TokenKind::BangEq;
    } else {
      kind = TokenKind::Bang;
    }
    break;
  case '<':
    if (index_ < source_.size() && source_[index_] == '=') {
      index_++;
      kind = TokenKind::LessEq;
    } else if (index_ < source_.size() && source_[index_] == '<') {
      index_++;
      kind = TokenKind::Shl;
    } else {
      kind = TokenKind::Less;
    }
    break;
  case '>':
    if (index_ < source_.size() && source_[index_] == '=') {
      index_++;
      kind = TokenKind::GreaterEq;
    } else if (index_ < source_.size() && source_[index_] == '>') {
      index_++;
      kind = TokenKind::Shr;
    } else {
      kind = TokenKind::Greater;
    }
    break;
  case '&':
    if (index_ < source_.size() && source_[index_] == '&') {
      index_++;
      kind = TokenKind::AmpAmp;
    } else {
      kind = TokenKind::Amp;
    }
    break;
  case '|':
    if (index_ < source_.size() && source_[index_] == '|') {
      index_++;
      kind = TokenKind::PipePipe;
    } else {
      kind = TokenKind::Pipe;
    }
    break;
  case '+':
    if (index_ < source_.size() && source_[index_] == '=') {
      index_++;
      kind = TokenKind::PlusEq;
    } else {
      kind = TokenKind::Plus;
    }
    break;
  case '*':
    if (index_ < source_.size() && source_[index_] == '=') {
      index_++;
      kind = TokenKind::StarEq;
    } else {
      kind = TokenKind::Star;
    }
    break;
  case '/':
    if (index_ < source_.size() && source_[index_] == '=') {
      index_++;
      kind = TokenKind::SlashEq;
    } else {
      kind = TokenKind::Slash;
    }
    break;
  case '.':
    if (index_ < source_.size() && source_[index_] == '.') {
      index_++;
      kind = TokenKind::DotDot;
    } else {
      kind = TokenKind::Dot;
    }
    break;
  case '(': kind = TokenKind::LParen; break;
  case ')': kind = TokenKind::RParen; break;
  case '{': kind = TokenKind::LBrace; break;
  case '}': kind = TokenKind::RBrace; break;
  case '[': kind = TokenKind::LBracket; break;
  case ']': kind = TokenKind::RBracket; break;
  case ',': kind = TokenKind::Comma; break;
  case ':': kind = TokenKind::Colon; break;
  case ';': kind = TokenKind::Semicolon; break;
  case '%': kind = TokenKind::Percent; break;
  case '^': kind = TokenKind::Caret; break;
  case '~': kind = TokenKind::Tilde; break;
  case '?': kind = TokenKind::Question; break;
  default: kind = TokenKind::Invalid; break;
  }

  insertSemi_ = triggersSemicolon(kind);
  return Token{kind, start, index_};
}

llvm::SmallVector<Token> scanAll(llvm::StringRef source) {
  Scanner scanner(source);
  llvm::SmallVector<Token> tokens;
  while (true) {
    Token tok = scanner.next();
    tokens.push_back(tok);
    if (tok.kind == TokenKind::Eof)
      break;
  }
  return tokens;
}

} // namespace ac
