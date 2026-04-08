//! CIR code generation from the ac AST.
//!
//! Walks the AST and emits CIR operations via MLIF's builder API.
//! Modeled after Zig's AstGen.

use crate::parser::Decl;
use mlif::{Context, Module, Location};

/// Generate CIR from a parsed program.
pub fn generate(_ctx: &mut Context, _module: &Module, _decls: &[Decl]) {
    // TODO: implement codegen
    // For each FnDecl:
    //   1. Create func_type from params + return type
    //   2. Use Builder to create func.func op
    //   3. Walk body statements, emit CIR ops
    //   4. Emit func.return
}

/// Compile a module to an executable via Cranelift.
pub fn emit_executable(_ctx: &Context, _module: &Module, _output: &str) {
    // TODO: Phase 3 — Cranelift codegen
    // 1. Lower CIR → Cranelift CLIF IR
    // 2. Emit object file via cranelift-object
    // 3. Link via cc
}
