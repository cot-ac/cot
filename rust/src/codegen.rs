//! CIR code generation from the ac AST.
//!
//! Walks the AST and emits CIR operations via MLIF's builder API.
//! Port of the C++ Codegen (Codegen.h + Codegen.cpp).
//! Zig AstGen pattern: single-pass recursive dispatch over AST nodes.

use std::collections::HashMap;

use crate::parser::{
    Expr, ExprKind, FnDecl, Module as AcModule, Stmt, StmtKind, TypeRef,
};
use crate::scanner::TokenKind;
use mlif::ir::builder::Builder;
use mlif::ir::context::Context;
use mlif::ir::location::Location;
use mlif::ir::module::Module as MlifModule;
use mlif::{BlockId, TypeId, ValueId};

/// Generate CIR from a parsed ac module.
///
/// Returns the MLIF Module handle. The caller can then lower it to
/// machine code via `mlif::codegen::lower_module`.
pub fn generate(ctx: &mut Context, source: &str, ast: &AcModule, filename: &str) -> MlifModule {
    let loc = Location::file_line_col(filename, 1, 1);
    let module = MlifModule::new(ctx, loc);
    let module_block = module.body_block(ctx);

    let mut cg = CodeGen {
        source: source.to_string(),
        filename: filename.to_string(),
        module_block,
        params: HashMap::new(),
        locals: HashMap::new(),
        func_return_type: None,
        // Collect function signatures for call resolution
        func_sigs: HashMap::new(),
    };

    // First pass: collect all function signatures so calls can resolve return types.
    for func in &ast.functions {
        let param_types: Vec<TypeId> = func
            .params
            .iter()
            .map(|p| cg.resolve_type(ctx, &p.ty))
            .collect();
        let ret_type = cg.resolve_type(ctx, &func.return_type);
        cg.func_sigs
            .insert(func.name.clone(), (param_types, ret_type));
    }

    // Second pass: emit all functions.
    for func in &ast.functions {
        cg.emit_fn_decl(ctx, func);
    }

    module
}

struct CodeGen {
    source: String,
    filename: String,
    module_block: BlockId,

    // Per-function scope
    params: HashMap<String, ValueId>,
    locals: HashMap<String, (ValueId, TypeId)>, // alloca addr + value type
    func_return_type: Option<TypeId>,

    // Global function signature registry: name -> (param types, return type)
    func_sigs: HashMap<String, (Vec<TypeId>, TypeId)>,
}

impl CodeGen {
    fn line_col(&self, offset: usize) -> (u32, u32) {
        let mut line = 1u32;
        let mut col = 1u32;
        for (i, c) in self.source.bytes().enumerate() {
            if i >= offset {
                break;
            }
            if c == b'\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        (line, col)
    }

    fn loc_from(&self, offset: usize) -> Location {
        let (line, col) = self.line_col(offset);
        Location::file_line_col(&self.filename, line, col)
    }

    fn resolve_type(&self, ctx: &mut Context, t: &TypeRef) -> TypeId {
        // For Gate 1+2 we need: i8, i16, i32, i64, u8..u64, f32, f64, bool, void
        // Other types are todo!() stubs.

        if t.is_pointer {
            todo!("pointer types not yet implemented in Rust codegen");
        }
        if t.is_optional {
            todo!("optional types not yet implemented in Rust codegen");
        }
        if t.is_error_union {
            todo!("error union types not yet implemented in Rust codegen");
        }
        if t.is_slice {
            todo!("slice types not yet implemented in Rust codegen");
        }
        if t.is_array {
            todo!("array types not yet implemented in Rust codegen");
        }

        match t.name.as_str() {
            "i8" => ctx.integer_type(8),
            "i16" => ctx.integer_type(16),
            "i32" => ctx.integer_type(32),
            "i64" => ctx.integer_type(64),
            "u8" => ctx.integer_type(8),
            "u16" => ctx.integer_type(16),
            "u32" => ctx.integer_type(32),
            "u64" => ctx.integer_type(64),
            "f32" => ctx.float_type(32),
            "f64" => ctx.float_type(64),
            "bool" => ctx.integer_type(1),
            "void" => ctx.none_type(),
            other => {
                eprintln!("error: unknown type '{}'", other);
                ctx.integer_type(32) // fallback
            }
        }
    }

    fn emit_fn_decl(&mut self, ctx: &mut Context, func: &FnDecl) {
        let loc = self.loc_from(func.pos);

        // Build function type
        let param_types: Vec<TypeId> = func
            .params
            .iter()
            .map(|p| self.resolve_type(ctx, &p.ty))
            .collect();
        let ret_type = self.resolve_type(ctx, &func.return_type);

        let result_types = if ctx.is_none_type(ret_type) {
            vec![]
        } else {
            vec![ret_type]
        };
        let func_type = ctx.function_type(&param_types, &result_types);

        // Build the function op
        let mut b = Builder::at_end(ctx, self.module_block);
        let func_op = b.build_func(&func.name, func_type, loc.clone());
        let entry_block = b.func_entry_block(func_op);

        // Clear per-function state
        self.params.clear();
        self.locals.clear();
        self.func_return_type = if result_types.is_empty() {
            None
        } else {
            Some(ret_type)
        };

        // Map parameters
        for (i, param) in func.params.iter().enumerate() {
            let val = b.block_argument(entry_block, i);
            // We store directly in params map (SSA values, no alloca needed for
            // read-only parameters in Gate 1+2 scope).
            self.params.insert(param.name.clone(), val);
        }

        // Drop the builder -- we'll use raw ctx + block for body emission
        drop(b);

        // Emit body statements
        let mut current_block = entry_block;
        for stmt in &func.body {
            self.emit_stmt(ctx, &mut current_block, stmt);
        }
    }

    fn emit_stmt(&mut self, ctx: &mut Context, block: &mut BlockId, stmt: &Stmt) {
        let loc = self.loc_from(stmt.pos);

        match stmt.kind {
            StmtKind::Return => {
                if let Some(ref expr) = stmt.expr {
                    let expected = self.func_return_type;
                    let val = self.emit_expr(ctx, *block, expr, expected);
                    let mut b = Builder::at_end(ctx, *block);
                    b.build_return(&[val], loc);
                } else {
                    let mut b = Builder::at_end(ctx, *block);
                    b.build_return(&[], loc);
                }
            }
            StmtKind::Let | StmtKind::Var => {
                // Resolve declared type if present
                let declared_type = if !stmt.var_type.name.is_empty() {
                    Some(self.resolve_type(ctx, &stmt.var_type))
                } else {
                    None
                };

                let val = self.emit_expr(ctx, *block, stmt.expr.as_ref().unwrap(), declared_type);
                let val_type = ctx.value_type(val);

                // For Gate 1+2 scope, track SSA values directly. Mutable
                // variables store their current SSA value and get updated
                // on reassignment. Immutable variables go in the params map
                // (which is used for both params and let bindings).
                if stmt.kind == StmtKind::Var {
                    self.locals
                        .insert(stmt.var_name.clone(), (val, val_type));
                } else {
                    self.params.insert(stmt.var_name.clone(), val);
                }
            }
            StmtKind::Assign => {
                // For Gate 1+2 scope, handle simple ident assignment
                if !stmt.var_name.is_empty() {
                    let old_entry = self.locals.get(&stmt.var_name);
                    let expected = old_entry.map(|e| e.1);
                    let val =
                        self.emit_expr(ctx, *block, stmt.expr.as_ref().unwrap(), expected);
                    let val_type = ctx.value_type(val);
                    self.locals
                        .insert(stmt.var_name.clone(), (val, val_type));
                } else {
                    todo!("complex assignment (field/index) not yet implemented in Rust codegen");
                }
            }
            StmtKind::ExprStmt => {
                if let Some(ref expr) = stmt.expr {
                    self.emit_expr(ctx, *block, expr, None);
                }
            }
            StmtKind::If => {
                todo!("if statements not yet implemented in Rust codegen");
            }
            StmtKind::While => {
                todo!("while statements not yet implemented in Rust codegen");
            }
            StmtKind::For => {
                todo!("for statements not yet implemented in Rust codegen");
            }
            StmtKind::Break => {
                todo!("break not yet implemented in Rust codegen");
            }
            StmtKind::Continue => {
                todo!("continue not yet implemented in Rust codegen");
            }
            StmtKind::CompoundAssign => {
                todo!("compound assignment not yet implemented in Rust codegen");
            }
            StmtKind::Assert => {
                todo!("assert not yet implemented in Rust codegen");
            }
            StmtKind::Match => {
                todo!("match not yet implemented in Rust codegen");
            }
        }
    }

    fn emit_expr(
        &mut self,
        ctx: &mut Context,
        block: BlockId,
        expr: &Expr,
        expected_type: Option<TypeId>,
    ) -> ValueId {
        let loc = self.loc_from(expr.pos);

        match expr.kind {
            ExprKind::IntLit => {
                let ty = expected_type.unwrap_or_else(|| ctx.integer_type(32));
                cot_arith::ops::build_constant_int(ctx, block, ty, expr.int_val, loc)
            }
            ExprKind::FloatLit => {
                let ty = expected_type.unwrap_or_else(|| ctx.float_type(64));
                cot_arith::ops::build_constant_float(ctx, block, ty, expr.float_val, loc)
            }
            ExprKind::BoolLit => {
                cot_arith::ops::build_constant_bool(ctx, block, expr.bool_val, loc)
            }
            ExprKind::Ident => {
                // Look up in params first, then locals
                if let Some(&val) = self.params.get(&expr.name) {
                    return val;
                }
                if let Some(&(val, _ty)) = self.locals.get(&expr.name) {
                    return val;
                }
                panic!("error: undefined variable '{}'", expr.name);
            }
            ExprKind::Call => {
                // Look up the callee's signature
                let (callee_param_types, callee_ret_type) = self
                    .func_sigs
                    .get(&expr.name)
                    .unwrap_or_else(|| panic!("error: undefined function '{}'", expr.name))
                    .clone();

                // Emit arguments with expected types from callee signature
                let mut arg_vals = Vec::new();
                for (i, arg) in expr.args.iter().enumerate() {
                    let expected = callee_param_types.get(i).copied();
                    arg_vals.push(self.emit_expr(ctx, block, arg, expected));
                }

                // Build func.call
                let result_types = if ctx.is_none_type(callee_ret_type) {
                    vec![]
                } else {
                    vec![callee_ret_type]
                };

                let mut b = Builder::at_end(ctx, block);
                let call_op = b.build_call(&expr.name, &arg_vals, &result_types, loc);
                if result_types.is_empty() {
                    // Void call -- return a dummy. Shouldn't be used as a value.
                    let dummy_ty = ctx.integer_type(32);
                    cot_arith::ops::build_constant_int(
                        ctx,
                        block,
                        dummy_ty,
                        0,
                        Location::unknown(),
                    )
                } else {
                    b.op_result(call_op, 0)
                }
            }
            ExprKind::BinOp => {
                self.emit_binop(ctx, block, expr, expected_type)
            }
            ExprKind::UnaryOp => {
                let val = self.emit_expr(ctx, block, expr.rhs.as_ref().unwrap(), expected_type);
                match expr.op {
                    TokenKind::Minus => cot_arith::ops::build_neg(ctx, block, val, loc),
                    TokenKind::Tilde => cot_arith::ops::build_bit_not(ctx, block, val, loc),
                    TokenKind::Bang => {
                        // !val = val XOR true
                        let one = cot_arith::ops::build_constant_bool(ctx, block, true, loc.clone());
                        cot_arith::ops::build_bit_xor(ctx, block, val, one, loc)
                    }
                    _ => val,
                }
            }
            ExprKind::StringLit => {
                todo!("string literals not yet implemented in Rust codegen");
            }
            ExprKind::NullLit => {
                todo!("null literals not yet implemented in Rust codegen");
            }
            ExprKind::StructLit => {
                todo!("struct literals not yet implemented in Rust codegen");
            }
            ExprKind::FieldAccess => {
                todo!("field access not yet implemented in Rust codegen");
            }
            ExprKind::ArrayLit => {
                todo!("array literals not yet implemented in Rust codegen");
            }
            ExprKind::Index => {
                todo!("index not yet implemented in Rust codegen");
            }
            ExprKind::ForceUnwrap => {
                todo!("force unwrap not yet implemented in Rust codegen");
            }
            ExprKind::TryUnwrap => {
                todo!("try unwrap not yet implemented in Rust codegen");
            }
            ExprKind::AddrOf => {
                todo!("address-of not yet implemented in Rust codegen");
            }
            ExprKind::Deref => {
                todo!("dereference not yet implemented in Rust codegen");
            }
            ExprKind::CastAs => {
                todo!("cast-as not yet implemented in Rust codegen");
            }
            ExprKind::SliceFrom => {
                todo!("slice-from not yet implemented in Rust codegen");
            }
        }
    }

    fn emit_binop(
        &mut self,
        ctx: &mut Context,
        block: BlockId,
        expr: &Expr,
        expected_type: Option<TypeId>,
    ) -> ValueId {
        let loc = self.loc_from(expr.pos);
        let lhs_expr = expr.lhs.as_ref().unwrap();
        let rhs_expr = expr.rhs.as_ref().unwrap();

        // Comparison ops
        if matches!(
            expr.op,
            TokenKind::EqEq
                | TokenKind::BangEq
                | TokenKind::Less
                | TokenKind::LessEq
                | TokenKind::Greater
                | TokenKind::GreaterEq
        ) {
            let lhs = self.emit_expr(ctx, block, lhs_expr, expected_type);
            let lhs_type = ctx.value_type(lhs);
            let rhs = self.emit_expr(ctx, block, rhs_expr, Some(lhs_type));

            // Check if float
            if ctx.is_float_type(lhs_type) {
                let pred = match expr.op {
                    TokenKind::EqEq => cot_arith::ops::FloatPredicate::Oeq,
                    TokenKind::BangEq => cot_arith::ops::FloatPredicate::One,
                    TokenKind::Less => cot_arith::ops::FloatPredicate::Olt,
                    TokenKind::LessEq => cot_arith::ops::FloatPredicate::Ole,
                    TokenKind::Greater => cot_arith::ops::FloatPredicate::Ogt,
                    TokenKind::GreaterEq => cot_arith::ops::FloatPredicate::Oge,
                    _ => unreachable!(),
                };
                return cot_arith::ops::build_cmpf(ctx, block, pred, lhs, rhs, loc);
            }

            // Integer comparison
            let pred = match expr.op {
                TokenKind::EqEq => cot_arith::ops::IntPredicate::Eq,
                TokenKind::BangEq => cot_arith::ops::IntPredicate::Ne,
                TokenKind::Less => cot_arith::ops::IntPredicate::Slt,
                TokenKind::LessEq => cot_arith::ops::IntPredicate::Sle,
                TokenKind::Greater => cot_arith::ops::IntPredicate::Sgt,
                TokenKind::GreaterEq => cot_arith::ops::IntPredicate::Sge,
                _ => unreachable!(),
            };
            return cot_arith::ops::build_cmp(ctx, block, pred, lhs, rhs, loc);
        }

        // Logical ops
        if expr.op == TokenKind::AmpAmp {
            let lhs = self.emit_expr(ctx, block, lhs_expr, None);
            let rhs = self.emit_expr(ctx, block, rhs_expr, None);
            let f = cot_arith::ops::build_constant_bool(ctx, block, false, loc.clone());
            return cot_arith::ops::build_select(ctx, block, lhs, rhs, f, loc);
        }
        if expr.op == TokenKind::PipePipe {
            let lhs = self.emit_expr(ctx, block, lhs_expr, None);
            let rhs = self.emit_expr(ctx, block, rhs_expr, None);
            let t = cot_arith::ops::build_constant_bool(ctx, block, true, loc.clone());
            return cot_arith::ops::build_select(ctx, block, lhs, t, rhs, loc);
        }

        // Arithmetic
        let lhs = self.emit_expr(ctx, block, lhs_expr, expected_type);
        let lhs_type = ctx.value_type(lhs);
        let rhs = self.emit_expr(ctx, block, rhs_expr, Some(lhs_type));

        match expr.op {
            TokenKind::Plus => cot_arith::ops::build_add(ctx, block, lhs, rhs, loc),
            TokenKind::Minus => cot_arith::ops::build_sub(ctx, block, lhs, rhs, loc),
            TokenKind::Star => cot_arith::ops::build_mul(ctx, block, lhs, rhs, loc),
            TokenKind::Slash => cot_arith::ops::build_divsi(ctx, block, lhs, rhs, loc),
            TokenKind::Percent => cot_arith::ops::build_remsi(ctx, block, lhs, rhs, loc),
            TokenKind::Amp => cot_arith::ops::build_bit_and(ctx, block, lhs, rhs, loc),
            TokenKind::Pipe => cot_arith::ops::build_bit_or(ctx, block, lhs, rhs, loc),
            TokenKind::Caret => cot_arith::ops::build_bit_xor(ctx, block, lhs, rhs, loc),
            TokenKind::Shl => cot_arith::ops::build_shl(ctx, block, lhs, rhs, loc),
            TokenKind::Shr => cot_arith::ops::build_shr(ctx, block, lhs, rhs, loc),
            _ => {
                eprintln!("error: unsupported binary op {:?}", expr.op);
                lhs
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use crate::scanner;

    fn build_module(src: &str) -> (Context, MlifModule) {
        let tokens = scanner::scan(src);
        let ast = parser::parse(src, &tokens);
        let mut ctx = Context::new();
        let module = generate(&mut ctx, src, &ast, "test.ac");
        (ctx, module)
    }

    #[test]
    fn test_codegen_hello() {
        let (ctx, module) = build_module("fn main() -> i32 {\n    return 42\n}\n");
        let ir = ctx.print_op(module.op());
        assert!(ir.contains("func.func"));
        assert!(ir.contains("cir.constant"));
        assert!(ir.contains("func.return"));
    }

    #[test]
    fn test_codegen_arithmetic() {
        let src = "fn add(a: i32, b: i32) -> i32 {\n    return a + b\n}\n\nfn main() -> i32 {\n    return add(19, 23)\n}\n";
        let (ctx, module) = build_module(src);
        let ir = ctx.print_op(module.op());
        assert!(ir.contains("func.func"));
        assert!(ir.contains("cir.add"));
        assert!(ir.contains("func.call"));
    }

    #[test]
    fn test_codegen_hello_lower() {
        let (ctx, module) = build_module("fn main() -> i32 {\n    return 42\n}\n");
        let bytes =
            mlif::codegen::lower_module(&ctx, module.op()).expect("lowering should succeed");
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_codegen_arithmetic_lower() {
        let src = "fn add(a: i32, b: i32) -> i32 {\n    return a + b\n}\n\nfn main() -> i32 {\n    return add(19, 23)\n}\n";
        let (ctx, module) = build_module(src);
        let bytes =
            mlif::codegen::lower_module(&ctx, module.op()).expect("lowering should succeed");
        assert!(!bytes.is_empty());
    }

    /// End-to-end Gate 1: hello.ac -> exit code 42
    #[test]
    fn test_gate1_hello() {
        let src = "fn main() -> i32 {\n    return 42\n}\n";
        let (ctx, module) = build_module(src);
        let bytes = mlif::codegen::lower_module(&ctx, module.op()).expect("lowering failed");

        let tmp_dir = std::env::temp_dir();
        let obj_path = tmp_dir.join("cot_gate1_hello.o");
        let exe_path = tmp_dir.join("cot_gate1_hello");

        mlif::codegen::write_object_file(&bytes, obj_path.to_str().unwrap())
            .expect("write_object_file failed");
        mlif::codegen::link_executable(obj_path.to_str().unwrap(), exe_path.to_str().unwrap())
            .expect("link_executable failed");

        let status = std::process::Command::new(exe_path.to_str().unwrap())
            .status()
            .expect("failed to run executable");

        assert_eq!(status.code(), Some(42), "Gate 1: expected exit code 42");

        let _ = std::fs::remove_file(&obj_path);
        let _ = std::fs::remove_file(&exe_path);
    }

    /// End-to-end Gate 2: arithmetic.ac -> exit code 42
    #[test]
    fn test_gate2_arithmetic() {
        let src = "fn add(a: i32, b: i32) -> i32 {\n    return a + b\n}\n\nfn main() -> i32 {\n    return add(19, 23)\n}\n";
        let (ctx, module) = build_module(src);
        let bytes = mlif::codegen::lower_module(&ctx, module.op()).expect("lowering failed");

        let tmp_dir = std::env::temp_dir();
        let obj_path = tmp_dir.join("cot_gate2_arith.o");
        let exe_path = tmp_dir.join("cot_gate2_arith");

        mlif::codegen::write_object_file(&bytes, obj_path.to_str().unwrap())
            .expect("write_object_file failed");
        mlif::codegen::link_executable(obj_path.to_str().unwrap(), exe_path.to_str().unwrap())
            .expect("link_executable failed");

        let status = std::process::Command::new(exe_path.to_str().unwrap())
            .status()
            .expect("failed to run executable");

        assert_eq!(status.code(), Some(42), "Gate 2: expected exit code 42");

        let _ = std::fs::remove_file(&obj_path);
        let _ = std::fs::remove_file(&exe_path);
    }
}
