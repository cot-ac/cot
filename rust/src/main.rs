//! cot — compiler for the ac language.
//!
//! Default backend: Rust/MLIF/Cranelift (fast compilation).
//! Optional: `--llvm` flag to use the C++/MLIR/LLVM backend
//! (best optimization, links libcot_cpp.a instead).

mod scanner;
mod parser;
mod codegen;

use std::env;
use std::fs;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("usage: cot <file.ac> [--llvm] [-o output]");
        process::exit(1);
    }

    let input = &args[1];
    let source = match fs::read_to_string(input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read '{}': {}", input, e);
            process::exit(1);
        }
    };

    // TODO: implement pipeline
    // 1. Scan source into tokens
    // 2. Parse tokens into AST
    // 3. Generate CIR via MLIF
    // 4. Run construct transforms
    // 5. Lower to Cranelift and emit binary (or --llvm for MLIR/LLVM path)
    let _tokens = scanner::scan(&source);
    eprintln!("cot: not yet implemented");
    process::exit(1);
}
