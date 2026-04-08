//! cot -- compiler for the ac language.
//!
//! Pipeline: source -> scan -> parse -> codegen (CIR via MLIF) -> lower (Cranelift) -> link -> executable.
//!
//! Default backend: Rust/MLIF/Cranelift (fast compilation).

mod codegen;
mod parser;
mod scanner;

use std::env;
use std::fs;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 3 {
        eprintln!("Usage: cot <command> <file.ac> [-o output]");
        eprintln!("Commands: build, emit-cir");
        process::exit(1);
    }

    let command = &args[1];
    let input = &args[2];

    // Read source file
    let source = match fs::read_to_string(input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read '{}': {}", input, e);
            process::exit(1);
        }
    };

    // Frontend: source -> tokens -> AST -> CIR
    let tokens = scanner::scan(&source);
    let ast = parser::parse(&source, &tokens);
    let mut ctx = mlif::Context::new();
    let module = codegen::generate(&mut ctx, &source, &ast, input);

    match command.as_str() {
        "emit-cir" => {
            // Print the CIR IR
            let ir = ctx.print_op(module.op());
            print!("{}", ir);
        }
        "build" => {
            // Determine output path
            let output = if let Some(pos) = args.iter().position(|a| a == "-o") {
                args.get(pos + 1)
                    .map(|s| s.as_str())
                    .unwrap_or("a.out")
            } else {
                "a.out"
            };

            // Lower to Cranelift
            let bytes = match mlif::codegen::lower_module(&ctx, module.op()) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("error: lowering failed: {}", e);
                    process::exit(1);
                }
            };

            // Write object file
            let obj_path = format!("{}.o", output);
            if let Err(e) = mlif::codegen::write_object_file(&bytes, &obj_path) {
                eprintln!("error: write object file failed: {}", e);
                process::exit(1);
            }

            // Link executable
            if let Err(e) = mlif::codegen::link_executable(&obj_path, output) {
                eprintln!("error: linking failed: {}", e);
                process::exit(1);
            }

            // Clean up object file
            let _ = std::fs::remove_file(&obj_path);
        }
        _ => {
            eprintln!("error: unknown command '{}'", command);
            eprintln!("Commands: build, emit-cir");
            process::exit(1);
        }
    }
}
