//! Lexical scanner for the ac language.
//!
//! Converts source text into a stream of tokens. Port of the C++ Scanner
//! (Scanner.h + Scanner.cpp). Zig Tokenizer state machine + Go-style
//! semicolon insertion.

/// A token produced by the scanner.
#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub start: usize,
    pub end: usize,
}

impl Token {
    /// Extract the token's text from the source string.
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.start..self.end]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
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
    Enum,
    Match,

    // Types
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
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
    Semicolon, // ; (synthetic -- Go pattern)
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
    FatArrow,  // =>
}

/// Go pattern: these tokens trigger semicolon insertion before next newline.
fn triggers_semicolon(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Identifier
            | TokenKind::IntLiteral
            | TokenKind::FloatLiteral
            | TokenKind::StringLiteral
            | TokenKind::BoolTrue
            | TokenKind::BoolFalse
            | TokenKind::Null
            | TokenKind::Return
            | TokenKind::Break
            | TokenKind::Continue
            // Type keywords trigger semi (e.g., "-> i32\n")
            | TokenKind::I8
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
            // Closing delimiters
            | TokenKind::RParen
            | TokenKind::RBrace
            | TokenKind::RBracket
    )
}

fn lookup_keyword(word: &str) -> TokenKind {
    match word {
        "fn" => TokenKind::Fn,
        "return" => TokenKind::Return,
        "let" => TokenKind::Let,
        "var" => TokenKind::Var,
        "if" => TokenKind::If,
        "else" => TokenKind::Else,
        "while" => TokenKind::While,
        "for" => TokenKind::For,
        "in" => TokenKind::In,
        "break" => TokenKind::Break,
        "continue" => TokenKind::Continue,
        "test" => TokenKind::Test,
        "assert" => TokenKind::Assert,
        "true" => TokenKind::BoolTrue,
        "false" => TokenKind::BoolFalse,
        "null" => TokenKind::Null,
        "orelse" => TokenKind::Orelse,
        "struct" => TokenKind::Struct,
        "try" => TokenKind::Try,
        "catch" => TokenKind::Catch,
        "as" => TokenKind::As,
        "enum" => TokenKind::Enum,
        "match" => TokenKind::Match,
        "i8" => TokenKind::I8,
        "i16" => TokenKind::I16,
        "i32" => TokenKind::I32,
        "i64" => TokenKind::I64,
        "u8" => TokenKind::U8,
        "u16" => TokenKind::U16,
        "u32" => TokenKind::U32,
        "u64" => TokenKind::U64,
        "f32" => TokenKind::F32,
        "f64" => TokenKind::F64,
        "bool" => TokenKind::Bool,
        "void" => TokenKind::Void,
        _ => TokenKind::Identifier,
    }
}

pub struct Scanner<'a> {
    source: &'a [u8],
    index: usize,
    insert_semi: bool,
}

impl<'a> Scanner<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source: source.as_bytes(),
            index: 0,
            insert_semi: false,
        }
    }

    pub fn next_token(&mut self) -> Token {
        // Skip whitespace. Go pattern: stop at newlines when insert_semi is set.
        while self.index < self.source.len() {
            let c = self.source[self.index];
            if c == b' ' || c == b'\t' || c == b'\r' {
                self.index += 1;
                continue;
            }
            if c == b'\n' {
                self.index += 1;
                if self.insert_semi {
                    self.insert_semi = false;
                    return Token {
                        kind: TokenKind::Semicolon,
                        start: self.index - 1,
                        end: self.index,
                    };
                }
                continue;
            }
            break;
        }

        // EOF -- Go pattern: also produces semicolon if pending.
        if self.index >= self.source.len() {
            if self.insert_semi {
                self.insert_semi = false;
                return Token {
                    kind: TokenKind::Semicolon,
                    start: self.index,
                    end: self.index,
                };
            }
            return Token {
                kind: TokenKind::Eof,
                start: self.index,
                end: self.index,
            };
        }

        let start = self.index;
        let c = self.source[self.index];

        // Line comments: // ...
        if c == b'/'
            && self.index + 1 < self.source.len()
            && self.source[self.index + 1] == b'/'
        {
            self.index += 2;
            while self.index < self.source.len() && self.source[self.index] != b'\n' {
                self.index += 1;
            }
            return self.next_token(); // Comments don't produce tokens (Zig pattern).
        }

        // Identifiers and keywords
        if c.is_ascii_alphabetic() || c == b'_' {
            self.index += 1;
            while self.index < self.source.len() {
                let ch = self.source[self.index];
                if ch.is_ascii_alphanumeric() || ch == b'_' {
                    self.index += 1;
                } else {
                    break;
                }
            }
            let word = std::str::from_utf8(&self.source[start..self.index]).unwrap();
            let kind = lookup_keyword(word);
            self.insert_semi = triggers_semicolon(kind);
            return Token {
                kind,
                start,
                end: self.index,
            };
        }

        // Numbers: int or float
        if c.is_ascii_digit() {
            self.index += 1;
            while self.index < self.source.len()
                && (self.source[self.index].is_ascii_digit() || self.source[self.index] == b'_')
            {
                self.index += 1;
            }
            let mut kind = TokenKind::IntLiteral;
            // Check for decimal point followed by digit -> float
            if self.index < self.source.len()
                && self.source[self.index] == b'.'
                && self.index + 1 < self.source.len()
                && self.source[self.index + 1].is_ascii_digit()
            {
                self.index += 1;
                while self.index < self.source.len()
                    && (self.source[self.index].is_ascii_digit()
                        || self.source[self.index] == b'_')
                {
                    self.index += 1;
                }
                kind = TokenKind::FloatLiteral;
            }
            self.insert_semi = true;
            return Token {
                kind,
                start,
                end: self.index,
            };
        }

        // String literals
        if c == b'"' {
            self.index += 1;
            while self.index < self.source.len() && self.source[self.index] != b'"' {
                if self.source[self.index] == b'\\' && self.index + 1 < self.source.len() {
                    self.index += 1;
                }
                self.index += 1;
            }
            if self.index < self.source.len() {
                self.index += 1;
            }
            self.insert_semi = true;
            return Token {
                kind: TokenKind::StringLiteral,
                start,
                end: self.index,
            };
        }

        // Operators and punctuation
        self.index += 1;
        let kind = match c {
            b'-' => {
                if self.index < self.source.len() && self.source[self.index] == b'>' {
                    self.index += 1;
                    TokenKind::Arrow
                } else if self.index < self.source.len() && self.source[self.index] == b'=' {
                    self.index += 1;
                    TokenKind::MinusEq
                } else {
                    TokenKind::Minus
                }
            }
            b'=' => {
                if self.index < self.source.len() && self.source[self.index] == b'=' {
                    self.index += 1;
                    TokenKind::EqEq
                } else if self.index < self.source.len() && self.source[self.index] == b'>' {
                    self.index += 1;
                    TokenKind::FatArrow
                } else {
                    TokenKind::Eq
                }
            }
            b'!' => {
                if self.index < self.source.len() && self.source[self.index] == b'=' {
                    self.index += 1;
                    TokenKind::BangEq
                } else {
                    TokenKind::Bang
                }
            }
            b'<' => {
                if self.index < self.source.len() && self.source[self.index] == b'=' {
                    self.index += 1;
                    TokenKind::LessEq
                } else if self.index < self.source.len() && self.source[self.index] == b'<' {
                    self.index += 1;
                    TokenKind::Shl
                } else {
                    TokenKind::Less
                }
            }
            b'>' => {
                if self.index < self.source.len() && self.source[self.index] == b'=' {
                    self.index += 1;
                    TokenKind::GreaterEq
                } else if self.index < self.source.len() && self.source[self.index] == b'>' {
                    self.index += 1;
                    TokenKind::Shr
                } else {
                    TokenKind::Greater
                }
            }
            b'&' => {
                if self.index < self.source.len() && self.source[self.index] == b'&' {
                    self.index += 1;
                    TokenKind::AmpAmp
                } else {
                    TokenKind::Amp
                }
            }
            b'|' => {
                if self.index < self.source.len() && self.source[self.index] == b'|' {
                    self.index += 1;
                    TokenKind::PipePipe
                } else {
                    TokenKind::Pipe
                }
            }
            b'+' => {
                if self.index < self.source.len() && self.source[self.index] == b'=' {
                    self.index += 1;
                    TokenKind::PlusEq
                } else {
                    TokenKind::Plus
                }
            }
            b'*' => {
                if self.index < self.source.len() && self.source[self.index] == b'=' {
                    self.index += 1;
                    TokenKind::StarEq
                } else {
                    TokenKind::Star
                }
            }
            b'/' => {
                if self.index < self.source.len() && self.source[self.index] == b'=' {
                    self.index += 1;
                    TokenKind::SlashEq
                } else {
                    TokenKind::Slash
                }
            }
            b'.' => {
                if self.index < self.source.len() && self.source[self.index] == b'.' {
                    self.index += 1;
                    TokenKind::DotDot
                } else {
                    TokenKind::Dot
                }
            }
            b'(' => TokenKind::LParen,
            b')' => TokenKind::RParen,
            b'{' => TokenKind::LBrace,
            b'}' => TokenKind::RBrace,
            b'[' => TokenKind::LBracket,
            b']' => TokenKind::RBracket,
            b',' => TokenKind::Comma,
            b':' => TokenKind::Colon,
            b';' => TokenKind::Semicolon,
            b'%' => TokenKind::Percent,
            b'^' => TokenKind::Caret,
            b'~' => TokenKind::Tilde,
            b'?' => TokenKind::Question,
            _ => TokenKind::Invalid,
        };

        self.insert_semi = triggers_semicolon(kind);
        Token {
            kind,
            start,
            end: self.index,
        }
    }
}

/// Scan source text into a complete token list (including final Eof).
pub fn scan(source: &str) -> Vec<Token> {
    let mut scanner = Scanner::new(source);
    let mut tokens = Vec::new();
    loop {
        let tok = scanner.next_token();
        let is_eof = tok.kind == TokenKind::Eof;
        tokens.push(tok);
        if is_eof {
            break;
        }
    }
    tokens
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<TokenKind> {
        scan(source).iter().map(|t| t.kind).collect()
    }

    #[test]
    fn test_empty() {
        assert_eq!(kinds(""), vec![TokenKind::Eof]);
    }

    #[test]
    fn test_hello() {
        let src = "fn main() -> i32 {\n    return 42\n}\n";
        let tokens = scan(src);
        let k: Vec<TokenKind> = tokens.iter().map(|t| t.kind).collect();
        assert_eq!(
            k,
            vec![
                TokenKind::Fn,
                TokenKind::Identifier, // main
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::Arrow,
                TokenKind::I32,
                TokenKind::LBrace,
                TokenKind::Return,
                TokenKind::IntLiteral, // 42
                TokenKind::Semicolon,  // inserted after 42
                TokenKind::RBrace,
                TokenKind::Semicolon,  // inserted after }
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_semicolon_after_type() {
        // "-> i32\n" should insert semicolon after i32
        let src = "-> i32\n";
        let k = kinds(src);
        assert_eq!(
            k,
            vec![TokenKind::Arrow, TokenKind::I32, TokenKind::Semicolon, TokenKind::Eof]
        );
    }

    #[test]
    fn test_operators() {
        let src = "== != <= >= && || -> => .. << >>";
        let k = kinds(src);
        assert_eq!(
            k,
            vec![
                TokenKind::EqEq,
                TokenKind::BangEq,
                TokenKind::LessEq,
                TokenKind::GreaterEq,
                TokenKind::AmpAmp,
                TokenKind::PipePipe,
                TokenKind::Arrow,
                TokenKind::FatArrow,
                TokenKind::DotDot,
                TokenKind::Shl,
                TokenKind::Shr,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_float() {
        let src = "3.14";
        let tokens = scan(src);
        assert_eq!(tokens[0].kind, TokenKind::FloatLiteral);
        assert_eq!(tokens[0].text(src), "3.14");
    }

    #[test]
    fn test_string() {
        let src = r#""hello world""#;
        let tokens = scan(src);
        assert_eq!(tokens[0].kind, TokenKind::StringLiteral);
    }

    #[test]
    fn test_keywords() {
        let src = "fn return let var if else while for in break continue struct enum match test assert true false null orelse try catch as";
        let k = kinds(src);
        assert!(k.contains(&TokenKind::Fn));
        assert!(k.contains(&TokenKind::Return));
        assert!(k.contains(&TokenKind::Struct));
        assert!(k.contains(&TokenKind::Enum));
        assert!(k.contains(&TokenKind::Match));
        assert!(k.contains(&TokenKind::Orelse));
        assert!(k.contains(&TokenKind::Try));
        assert!(k.contains(&TokenKind::Catch));
    }

    #[test]
    fn test_line_comment() {
        let src = "42 // comment\n";
        let k = kinds(src);
        assert_eq!(
            k,
            vec![TokenKind::IntLiteral, TokenKind::Semicolon, TokenKind::Eof]
        );
    }

    #[test]
    fn test_arithmetic_file() {
        let src = "fn add(a: i32, b: i32) -> i32 {\n    return a + b\n}\n\nfn main() -> i32 {\n    return add(19, 23)\n}\n";
        let tokens = scan(src);
        // Should have proper semicolons inserted
        let k: Vec<TokenKind> = tokens.iter().map(|t| t.kind).collect();
        // Check that it ends with Eof and has no Invalid tokens
        assert_eq!(*k.last().unwrap(), TokenKind::Eof);
        assert!(!k.contains(&TokenKind::Invalid));
    }

    #[test]
    fn test_eof_semicolon_insertion() {
        // "42" at EOF should produce: IntLiteral, Semicolon, Eof
        let src = "42";
        let k = kinds(src);
        assert_eq!(
            k,
            vec![TokenKind::IntLiteral, TokenKind::Semicolon, TokenKind::Eof]
        );
    }

    #[test]
    fn test_compound_assign() {
        let src = "+= -= *= /=";
        let k = kinds(src);
        assert_eq!(
            k,
            vec![
                TokenKind::PlusEq,
                TokenKind::MinusEq,
                TokenKind::StarEq,
                TokenKind::SlashEq,
                TokenKind::Eof,
            ]
        );
    }
}
