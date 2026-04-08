//! Lexical scanner for the ac language.
//!
//! Converts source text into a stream of tokens. Modeled after
//! Zig's Tokenizer + Go's semicolon insertion.

/// A token produced by the scanner.
#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub lexeme: String,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    // Literals
    IntLiteral,
    FloatLiteral,
    StringLiteral,
    Identifier,

    // Keywords (subset — expanded as ac language grows)
    Fn,
    Return,
    Let,
    Var,
    If,
    Else,
    While,
    For,
    True,
    False,

    // Punctuation
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    Comma,
    Colon,
    Semicolon,
    Arrow,

    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Equal,
    EqualEqual,
    Bang,
    BangEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,

    // Special
    Eof,
}

/// Scan source text into tokens.
pub fn scan(source: &str) -> Vec<Token> {
    // TODO: implement scanner
    let _ = source;
    vec![Token {
        kind: TokenKind::Eof,
        lexeme: String::new(),
        line: 1,
        col: 1,
    }]
}
