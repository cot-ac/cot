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
use mlif::{BlockId, RegionId, TypeId, ValueId};

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
        func_region: None,
        loop_header: None,
        loop_exit: None,
        func_sigs: HashMap::new(),
        enum_defs: HashMap::new(),
        struct_defs: HashMap::new(),
    };

    // Register enum definitions.
    for e in &ast.enums {
        cg.enum_defs.insert(e.name.clone(), e.variants.clone());
    }

    // Register struct definitions.
    for s in &ast.structs {
        let fields: Vec<(String, TypeRef)> = s
            .fields
            .iter()
            .map(|f| (f.name.clone(), f.ty.clone()))
            .collect();
        cg.struct_defs.insert(s.name.clone(), fields);
    }

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

    // Third pass: emit test blocks and generate test main.
    if !ast.tests.is_empty() {
        let void_ty = ctx.none_type();
        let i32_ty = ctx.integer_type(32);

        // Emit each test as a void function.
        let mut test_names = Vec::new();
        for (i, test) in ast.tests.iter().enumerate() {
            let name = format!("test_{}", i);
            let _fn_ty = ctx.function_type(&[], &[]);
            cg.func_sigs.insert(name.clone(), (vec![], void_ty));

            let fn_decl = FnDecl {
                name: name.clone(),
                type_params: Vec::new(),
                params: Vec::new(),
                return_type: TypeRef::simple("void"),
                body: test.body.clone(),
                pos: test.pos,
            };
            cg.emit_fn_decl(ctx, &fn_decl);
            test_names.push(name);
        }

        // Generate @main that calls each test and returns 42.
        let main_fn_ty = ctx.function_type(&[], &[i32_ty]);
        let loc = Location::file_line_col(filename, 1, 1);

        let mut b = Builder::at_end(ctx, module_block);
        let func_op = b.build_func("main", main_fn_ty, loc.clone());
        let entry = b.func_entry_block(func_op);
        b.set_insertion_point_to_end(entry);

        for name in &test_names {
            b.build_call(name, &[], &[], loc.clone());
        }

        let c42 = cot_arith::ops::build_constant_int(ctx, entry, i32_ty, 42, loc.clone());
        Builder::at_end(ctx, entry).build_return(&[c42], loc);
    }

    module
}

struct CodeGen {
    source: String,
    filename: String,
    module_block: BlockId,

    // Per-function scope — parameters as SSA values (block arguments)
    params: HashMap<String, ValueId>,
    // Per-function scope — locals as (alloca_ptr, value_type)
    locals: HashMap<String, (ValueId, TypeId)>,
    func_return_type: Option<TypeId>,
    func_region: Option<RegionId>,

    // Loop context for break/continue
    loop_header: Option<BlockId>,
    loop_exit: Option<BlockId>,

    // Global function signature registry: name -> (param types, return type)
    func_sigs: HashMap<String, (Vec<TypeId>, TypeId)>,

    // Enum definitions: name -> variant names (index = tag value)
    enum_defs: HashMap<String, Vec<String>>,
    // Struct definitions: name -> field (name, type_ref) pairs
    struct_defs: HashMap<String, Vec<(String, TypeRef)>>,
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
        if t.is_pointer {
            return cot_memory::types::ptr_type(ctx);
        }
        if t.is_optional {
            let inner_ty = self.resolve_type(ctx, &TypeRef::simple(&t.name));
            return cot_optionals::types::optional_type(ctx, inner_ty);
        }
        if t.is_error_union {
            let inner_ty = self.resolve_type(ctx, &TypeRef::simple(&t.name));
            return cot_errors::types::error_union_type(ctx, inner_ty);
        }
        if t.is_slice {
            let elem_ty = self.resolve_type(ctx, &TypeRef::simple(&t.name));
            return cot_slices::types::slice_type(ctx, elem_ty);
        }
        if t.is_array {
            let elem_ty = self.resolve_type(ctx, &TypeRef::simple(&t.name));
            return cot_arrays::types::array_type(ctx, t.array_len, elem_ty);
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
                // Enum types are i32 (tag value)
                if self.enum_defs.contains_key(other) {
                    return ctx.integer_type(32);
                }
                // Struct types are pointers (stack-allocated, passed by pointer)
                if self.struct_defs.contains_key(other) {
                    return cot_memory::types::ptr_type(ctx);
                }
                eprintln!("error: unknown type '{}'", other);
                ctx.integer_type(32) // fallback
            }
        }
    }

    /// Create a new block in the current function's region.
    fn create_block(&self, ctx: &mut Context) -> BlockId {
        let block = ctx.create_block();
        if let Some(region) = self.func_region {
            ctx.region_push_block(region, block);
        }
        block
    }

    /// Check if a block ends with a terminator.
    fn is_block_terminated(&self, ctx: &Context, block: BlockId) -> bool {
        let mut last_op = None;
        for op in ctx.block_ops(block) {
            last_op = Some(op);
        }
        match last_op {
            Some(op) => {
                let name = ctx[op].name();
                name == "func.return"
                    || name == "cir.br"
                    || name == "cir.condbr"
                    || name == "cir.switch"
                    || name == "cir.trap"
            }
            None => false,
        }
    }

    /// If the type is optional<T> or error_union<T>, return T. Otherwise return the type as-is.
    fn unwrap_wrapper_type(&self, ctx: &Context, ty: Option<TypeId>) -> Option<TypeId> {
        ty.map(|t| {
            match ctx.type_kind(t) {
                mlif::TypeKind::Extension(ext)
                    if ext.dialect == "cir"
                        && (ext.name == "optional" || ext.name == "error_union") =>
                {
                    ext.type_params.first().copied().unwrap_or(t)
                }
                _ => t,
            }
        })
    }

    fn is_aggregate_type(&self, ctx: &Context, ty: TypeId) -> bool {
        matches!(
            ctx.type_kind(ty),
            mlif::TypeKind::Extension(ext) if ext.dialect == "cir"
                && matches!(ext.name.as_str(), "optional" | "error_union" | "slice" | "struct" | "array" | "tagged_union")
        )
    }

    fn is_optional_type(&self, ctx: &Context, ty: TypeId) -> bool {
        matches!(ctx.type_kind(ty), mlif::TypeKind::Extension(ext) if ext.dialect == "cir" && ext.name == "optional")
    }

    fn is_error_union_type(&self, ctx: &Context, ty: TypeId) -> bool {
        matches!(ctx.type_kind(ty), mlif::TypeKind::Extension(ext) if ext.dialect == "cir" && ext.name == "error_union")
    }

    fn is_none_type_val(&self, _ctx: &Context, _val: ValueId) -> bool {
        false // conservative — null literals are already wrapped by NullLit handler
    }

    /// Find the struct type name for a variable (by looking at func sig or param type name).
    fn find_struct_type_for_var(&self, var_name: &str) -> Option<String> {
        // Check if there's a struct type that matches any known struct name
        // This is a simple heuristic — in a real compiler we'd have a proper type table
        for (sname, _) in &self.struct_defs {
            // Check locals and params for variables that might be this struct type
            // For now, just match by variable name convention or return first struct
            let _ = var_name; // We'd need type tracking for a proper impl
            let _ = sname;
        }
        // Fallback: check if there's only one struct defined
        if self.struct_defs.len() == 1 {
            return self.struct_defs.keys().next().cloned();
        }
        None
    }

    /// Try to find the enum name for a match expression.
    fn find_enum_name_for_match(&self, stmt: &Stmt) -> Option<String> {
        // Check if any variant in the match corresponds to a known enum
        for variant in &stmt.match_variants {
            for (ename, variants) in &self.enum_defs {
                if variants.contains(variant) {
                    return Some(ename.clone());
                }
            }
        }
        None
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
        drop(b);

        // Store function region for creating new blocks
        self.func_region = Some(ctx[func_op].region(0));

        // Clear per-function state
        self.params.clear();
        self.locals.clear();
        self.func_return_type = if result_types.is_empty() {
            None
        } else {
            Some(ret_type)
        };
        self.loop_header = None;
        self.loop_exit = None;

        // Map parameters — stay as SSA values (block arguments dominate all blocks)
        {
            let b = Builder::at_end(ctx, entry_block);
            for (i, param) in func.params.iter().enumerate() {
                let val = b.block_argument(entry_block, i);
                self.params.insert(param.name.clone(), val);
            }
        }

        // Emit body statements
        let mut current_block = entry_block;
        for stmt in &func.body {
            self.emit_stmt(ctx, &mut current_block, stmt);
        }

        // Add implicit void return if function is void and block not terminated
        if result_types.is_empty() && !self.is_block_terminated(ctx, current_block) {
            Builder::at_end(ctx, current_block)
                .build_return(&[], self.loc_from(func.pos));
        }
    }

    fn emit_stmt(&mut self, ctx: &mut Context, block: &mut BlockId, stmt: &Stmt) {
        // Don't emit into a terminated block
        if self.is_block_terminated(ctx, *block) {
            return;
        }

        let loc = self.loc_from(stmt.pos);

        match stmt.kind {
            StmtKind::Return => {
                if let Some(ref expr) = stmt.expr {
                    let expected = self.func_return_type;
                    let mut val = self.emit_expr(ctx, block, expr, expected);

                    // Auto-wrap: returning plain value from error-returning function
                    if let Some(ret_ty) = expected {
                        let val_ty = ctx.value_type(val);
                        if self.is_error_union_type(ctx, ret_ty)
                            && !self.is_error_union_type(ctx, val_ty)
                        {
                            val = cot_errors::ops::build_wrap_result(
                                ctx, *block, val, loc.clone(),
                            );
                        }
                        // Auto-wrap: returning plain value to optional-returning function
                        if self.is_optional_type(ctx, ret_ty)
                            && !self.is_optional_type(ctx, val_ty)
                        {
                            val = cot_optionals::ops::build_wrap_optional(
                                ctx, *block, val, loc.clone(),
                            );
                        }
                    }

                    Builder::at_end(ctx, *block).build_return(&[val], loc);
                } else {
                    Builder::at_end(ctx, *block).build_return(&[], loc);
                }
            }
            StmtKind::Let | StmtKind::Var => {
                // Resolve type
                let declared_type = if !stmt.var_type.name.is_empty() {
                    Some(self.resolve_type(ctx, &stmt.var_type))
                } else {
                    None
                };

                let mut init_val =
                    self.emit_expr(ctx, block, stmt.expr.as_ref().unwrap(), declared_type);

                // Auto-wrap: assigning plain value to optional variable
                if let Some(decl_ty) = declared_type {
                    let val_ty = ctx.value_type(init_val);
                    if self.is_optional_type(ctx, decl_ty)
                        && !self.is_optional_type(ctx, val_ty)
                        && !self.is_none_type_val(ctx, init_val)
                    {
                        init_val = cot_optionals::ops::build_wrap_optional(
                            ctx, *block, init_val, loc.clone(),
                        );
                    }
                }
                let val_type = ctx.value_type(init_val);

                // For aggregate types passed as pointers, alloca stores the pointer (i64),
                // not the aggregate itself.
                let alloca_type = if self.is_aggregate_type(ctx, val_type) {
                    ctx.integer_type(64)
                } else {
                    val_type
                };

                // Alloca + store for all locals (needed for control flow)
                let alloca =
                    cot_memory::ops::build_alloca(ctx, *block, alloca_type, loc.clone());
                cot_memory::ops::build_store(ctx, *block, init_val, alloca, loc);
                self.locals
                    .insert(stmt.var_name.clone(), (alloca, val_type));
            }
            StmtKind::Assign => {
                if !stmt.var_name.is_empty() {
                    if let Some(&(alloca, val_type)) = self.locals.get(&stmt.var_name) {
                        let val = self.emit_expr(
                            ctx,
                            block,
                            stmt.expr.as_ref().unwrap(),
                            Some(val_type),
                        );
                        cot_memory::ops::build_store(ctx, *block, val, alloca, loc);
                    } else {
                        panic!("error: assignment to undefined variable '{}'", stmt.var_name);
                    }
                } else {
                    todo!("complex assignment (field/index) not yet implemented in Rust codegen");
                }
            }
            StmtKind::ExprStmt => {
                if let Some(ref expr) = stmt.expr {
                    self.emit_expr(ctx, block, expr, None);
                }
            }
            StmtKind::If => {
                let cond = self.emit_expr(ctx, block, stmt.expr.as_ref().unwrap(), None);

                let then_block = self.create_block(ctx);
                let else_block = self.create_block(ctx);
                let merge_block = self.create_block(ctx);

                cot_flow::ops::build_condbr(
                    ctx, *block, cond, then_block, else_block, loc.clone(),
                );

                // Emit then body
                let mut then_current = then_block;
                for s in &stmt.then_body {
                    self.emit_stmt(ctx, &mut then_current, s);
                }
                if !self.is_block_terminated(ctx, then_current) {
                    cot_flow::ops::build_br(ctx, then_current, merge_block, loc.clone());
                }

                // Emit else body
                let mut else_current = else_block;
                if stmt.else_body.is_empty() {
                    // No else — just branch to merge
                    cot_flow::ops::build_br(ctx, else_current, merge_block, loc);
                } else {
                    for s in &stmt.else_body {
                        self.emit_stmt(ctx, &mut else_current, s);
                    }
                    if !self.is_block_terminated(ctx, else_current) {
                        cot_flow::ops::build_br(ctx, else_current, merge_block, loc);
                    }
                }

                *block = merge_block;
            }
            StmtKind::While => {
                let mut header = self.create_block(ctx);
                let body_block = self.create_block(ctx);
                let exit_block = self.create_block(ctx);

                // Branch to loop header
                cot_flow::ops::build_br(ctx, *block, header, loc.clone());

                // Header: evaluate condition, branch to body or exit
                let cond =
                    self.emit_expr(ctx, &mut header, stmt.expr.as_ref().unwrap(), None);
                cot_flow::ops::build_condbr(
                    ctx, header, cond, body_block, exit_block, loc.clone(),
                );

                // Save and set loop context
                let prev_header = self.loop_header.replace(header);
                let prev_exit = self.loop_exit.replace(exit_block);

                // Emit body
                let mut body_current = body_block;
                for s in &stmt.then_body {
                    self.emit_stmt(ctx, &mut body_current, s);
                }
                if !self.is_block_terminated(ctx, body_current) {
                    cot_flow::ops::build_br(ctx, body_current, header, loc);
                }

                // Restore loop context
                self.loop_header = prev_header;
                self.loop_exit = prev_exit;

                *block = exit_block;
            }
            StmtKind::Break => {
                if let Some(exit) = self.loop_exit {
                    cot_flow::ops::build_br(ctx, *block, exit, loc);
                } else {
                    panic!("error: break outside of loop");
                }
            }
            StmtKind::Continue => {
                if let Some(header) = self.loop_header {
                    cot_flow::ops::build_br(ctx, *block, header, loc);
                } else {
                    panic!("error: continue outside of loop");
                }
            }
            StmtKind::CompoundAssign => {
                if let Some(&(alloca, val_type)) = self.locals.get(&stmt.var_name) {
                    // Load current value
                    let current =
                        cot_memory::ops::build_load(ctx, *block, alloca, val_type, loc.clone());

                    // Emit RHS
                    let rhs = self.emit_expr(
                        ctx,
                        block,
                        stmt.expr.as_ref().unwrap(),
                        Some(val_type),
                    );

                    // Apply operation
                    let new_val = match stmt.op {
                        TokenKind::PlusEq => {
                            cot_arith::ops::build_add(ctx, *block, current, rhs, loc.clone())
                        }
                        TokenKind::MinusEq => {
                            cot_arith::ops::build_sub(ctx, *block, current, rhs, loc.clone())
                        }
                        TokenKind::StarEq => {
                            cot_arith::ops::build_mul(ctx, *block, current, rhs, loc.clone())
                        }
                        TokenKind::SlashEq => {
                            cot_arith::ops::build_divsi(ctx, *block, current, rhs, loc.clone())
                        }
                        // No PercentEq token in scanner yet
                        _ => {
                            eprintln!("error: unsupported compound assign op {:?}", stmt.op);
                            current
                        }
                    };

                    // Store back
                    cot_memory::ops::build_store(ctx, *block, new_val, alloca, loc);
                } else {
                    panic!(
                        "error: compound assignment to undefined variable '{}'",
                        stmt.var_name
                    );
                }
            }
            StmtKind::For => {
                // for i in start..end { body }
                let i32_ty = ctx.integer_type(32);

                // Evaluate start and end in current block
                let start_val =
                    self.emit_expr(ctx, block, stmt.expr.as_ref().unwrap(), Some(i32_ty));
                let end_val =
                    self.emit_expr(ctx, block, stmt.range_end.as_ref().unwrap(), Some(i32_ty));
                let val_type = ctx.value_type(start_val);

                // Alloca the loop variable
                let alloca =
                    cot_memory::ops::build_alloca(ctx, *block, val_type, loc.clone());
                cot_memory::ops::build_store(ctx, *block, start_val, alloca, loc.clone());
                self.locals
                    .insert(stmt.var_name.clone(), (alloca, val_type));

                let header = self.create_block(ctx);
                let body_block = self.create_block(ctx);
                let latch = self.create_block(ctx); // increment block
                let exit_block = self.create_block(ctx);

                cot_flow::ops::build_br(ctx, *block, header, loc.clone());

                // Header: check i < end
                let i_val =
                    cot_memory::ops::build_load(ctx, header, alloca, val_type, loc.clone());
                let cond = cot_arith::ops::build_cmp(
                    ctx,
                    header,
                    cot_arith::ops::IntPredicate::Slt,
                    i_val,
                    end_val,
                    loc.clone(),
                );
                cot_flow::ops::build_condbr(
                    ctx, header, cond, body_block, exit_block, loc.clone(),
                );

                // Latch: increment i, branch back to header
                let cur =
                    cot_memory::ops::build_load(ctx, latch, alloca, val_type, loc.clone());
                let one =
                    cot_arith::ops::build_constant_int(ctx, latch, val_type, 1, loc.clone());
                let next =
                    cot_arith::ops::build_add(ctx, latch, cur, one, loc.clone());
                cot_memory::ops::build_store(ctx, latch, next, alloca, loc.clone());
                cot_flow::ops::build_br(ctx, latch, header, loc.clone());

                // Set loop context: continue → latch (increment then re-check)
                let prev_header = self.loop_header.replace(latch);
                let prev_exit = self.loop_exit.replace(exit_block);

                // Emit body
                let mut body_current = body_block;
                for s in &stmt.then_body {
                    self.emit_stmt(ctx, &mut body_current, s);
                }
                if !self.is_block_terminated(ctx, body_current) {
                    cot_flow::ops::build_br(ctx, body_current, latch, loc);
                }

                // Restore loop context
                self.loop_header = prev_header;
                self.loop_exit = prev_exit;

                *block = exit_block;
            }
            StmtKind::Assert => {
                let cond = self.emit_expr(ctx, block, stmt.expr.as_ref().unwrap(), None);
                let ok_block = self.create_block(ctx);
                let fail_block = self.create_block(ctx);
                cot_flow::ops::build_condbr(ctx, *block, cond, ok_block, fail_block, loc.clone());
                cot_flow::ops::build_trap(ctx, fail_block, loc);
                *block = ok_block;
            }
            StmtKind::Match => {
                // match expr { Variant => { body } ... }
                let match_val = self.emit_expr(ctx, block, stmt.expr.as_ref().unwrap(), None);
                let i32_ty = ctx.integer_type(32);

                // Find the enum name from the match expression type
                let enum_name = self.find_enum_name_for_match(stmt);

                let merge_block = self.create_block(ctx);
                let num_arms = stmt.match_variants.len();

                // Build comparison chain
                let mut current = *block;
                for (i, variant_name) in stmt.match_variants.iter().enumerate() {
                    // Find variant index
                    let tag = if let Some(ref ename) = enum_name {
                        let variants = self.enum_defs.get(ename).unwrap();
                        variants.iter().position(|v| v == variant_name)
                            .unwrap_or_else(|| panic!("error: unknown variant '{}'", variant_name))
                            as i64
                    } else {
                        i as i64 // fallback: use arm index
                    };

                    let arm_block = self.create_block(ctx);
                    let tag_val = cot_arith::ops::build_constant_int(ctx, current, i32_ty, tag, loc.clone());
                    let cmp = cot_arith::ops::build_cmp(
                        ctx, current, cot_arith::ops::IntPredicate::Eq, match_val, tag_val, loc.clone(),
                    );

                    if i < num_arms - 1 {
                        let next_check = self.create_block(ctx);
                        cot_flow::ops::build_condbr(ctx, current, cmp, arm_block, next_check, loc.clone());
                        current = next_check;
                    } else {
                        // Last arm: unconditionally go to arm (acts as default)
                        cot_flow::ops::build_br(ctx, current, arm_block, loc.clone());
                    }

                    // Emit arm body
                    let mut arm_current = arm_block;
                    for s in &stmt.match_bodies[i] {
                        self.emit_stmt(ctx, &mut arm_current, s);
                    }
                    if !self.is_block_terminated(ctx, arm_current) {
                        cot_flow::ops::build_br(ctx, arm_current, merge_block, loc.clone());
                    }
                }

                *block = merge_block;
            }
        }
    }

    fn emit_expr(
        &mut self,
        ctx: &mut Context,
        block: &mut BlockId,
        expr: &Expr,
        expected_type: Option<TypeId>,
    ) -> ValueId {
        let loc = self.loc_from(expr.pos);

        match expr.kind {
            ExprKind::IntLit => {
                let ty = self.unwrap_wrapper_type(ctx, expected_type)
                    .unwrap_or_else(|| ctx.integer_type(32));
                cot_arith::ops::build_constant_int(ctx, *block, ty, expr.int_val, loc)
            }
            ExprKind::FloatLit => {
                let ty = self.unwrap_wrapper_type(ctx, expected_type)
                    .unwrap_or_else(|| ctx.float_type(64));
                cot_arith::ops::build_constant_float(ctx, *block, ty, expr.float_val, loc)
            }
            ExprKind::BoolLit => {
                cot_arith::ops::build_constant_bool(ctx, *block, expr.bool_val, loc)
            }
            ExprKind::Ident => {
                // Look up in params first (SSA values), then locals (alloca)
                if let Some(&val) = self.params.get(&expr.name) {
                    return val;
                }
                if let Some(&(alloca, val_type)) = self.locals.get(&expr.name) {
                    return cot_memory::ops::build_load(ctx, *block, alloca, val_type, loc);
                }
                panic!("error: undefined variable '{}'", expr.name);
            }
            ExprKind::Call => {
                // Special: error(code) → wrap_error
                if expr.name == "error" && expr.args.len() == 1 {
                    let i16_ty = ctx.integer_type(16);
                    let code = self.emit_expr(ctx, block, &expr.args[0], Some(i16_ty));
                    // Determine payload type from expected error_union type
                    let payload_ty = if let Some(ety) = expected_type {
                        match ctx.type_kind(ety) {
                            mlif::TypeKind::Extension(ext)
                                if ext.dialect == "cir" && ext.name == "error_union" =>
                            {
                                ext.type_params[0]
                            }
                            _ => ctx.integer_type(32),
                        }
                    } else {
                        ctx.integer_type(32)
                    };
                    return cot_errors::ops::build_wrap_error(
                        ctx, *block, code, payload_ty, loc,
                    );
                }

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

                let mut b = Builder::at_end(ctx, *block);
                let call_op = b.build_call(&expr.name, &arg_vals, &result_types, loc);
                if result_types.is_empty() {
                    // Void call — return a dummy. Shouldn't be used as a value.
                    let dummy_ty = ctx.integer_type(32);
                    cot_arith::ops::build_constant_int(
                        ctx,
                        *block,
                        dummy_ty,
                        0,
                        Location::unknown(),
                    )
                } else {
                    b.op_result(call_op, 0)
                }
            }
            ExprKind::BinOp => self.emit_binop(ctx, block, expr, expected_type),
            ExprKind::UnaryOp => {
                let val = self.emit_expr(ctx, block, expr.rhs.as_ref().unwrap(), expected_type);
                match expr.op {
                    TokenKind::Minus => cot_arith::ops::build_neg(ctx, *block, val, loc),
                    TokenKind::Tilde => cot_arith::ops::build_bit_not(ctx, *block, val, loc),
                    TokenKind::Bang => {
                        // !val = val XOR true
                        let one =
                            cot_arith::ops::build_constant_bool(ctx, *block, true, loc.clone());
                        cot_arith::ops::build_bit_xor(ctx, *block, val, one, loc)
                    }
                    _ => val,
                }
            }
            ExprKind::StringLit => {
                // "hello" → cir.string_constant → !cir.slice<i8>
                let i8_ty = ctx.integer_type(8);
                let slice_ty = cot_slices::types::slice_type(ctx, i8_ty);
                cot_slices::ops::build_string_constant(
                    ctx, *block, &expr.str_val, slice_ty, loc,
                )
            }
            ExprKind::NullLit => {
                // null → cir.none of expected optional type
                let opt_ty = expected_type.unwrap_or_else(|| {
                    let i32_ty = ctx.integer_type(32);
                    cot_optionals::types::optional_type(ctx, i32_ty)
                });
                cot_optionals::ops::build_none(ctx, *block, opt_ty, loc)
            }
            ExprKind::StructLit => {
                // Point { x: 1, y: 2 } — alloca struct, store each field
                let struct_name = &expr.name;
                let fields_def = self
                    .struct_defs
                    .get(struct_name)
                    .unwrap_or_else(|| panic!("error: unknown struct '{}'", struct_name))
                    .clone();

                // Compute total size and field offsets (all i32 for now = 4 bytes each)
                let mut field_sizes = Vec::new();
                let mut field_offsets = Vec::new();
                let mut offset: u32 = 0;
                for (_, fty) in &fields_def {
                    let ty = self.resolve_type(ctx, fty);
                    let size = mlif::codegen::types::type_byte_size(ctx, ty);
                    field_offsets.push(offset);
                    field_sizes.push(size);
                    offset += size;
                }
                let total_size = offset;

                // Create a stack slot by allocating total_size bytes
                let alloca_ty = ctx.integer_type((total_size * 8).max(8) as u32);
                let ptr = cot_memory::ops::build_alloca(ctx, *block, alloca_ty, loc.clone());

                // Store each field
                for field_init in &expr.fields {
                    // Find field index
                    let fidx = fields_def
                        .iter()
                        .position(|(name, _)| name == &field_init.name)
                        .unwrap_or_else(|| {
                            panic!("error: unknown field '{}' in struct '{}'", field_init.name, struct_name)
                        });
                    let fty = self.resolve_type(ctx, &fields_def[fidx].1);
                    let val = self.emit_expr(ctx, block, &field_init.value, Some(fty));

                    if field_offsets[fidx] == 0 {
                        // Store directly at base
                        cot_memory::ops::build_store(ctx, *block, val, ptr, loc.clone());
                    } else {
                        // Compute field address: ptr + offset
                        let i64_ty = ctx.integer_type(64);
                        let off_val = cot_arith::ops::build_constant_int(
                            ctx, *block, i64_ty, field_offsets[fidx] as i64, loc.clone(),
                        );
                        let field_ptr = cot_arith::ops::build_add(ctx, *block, ptr, off_val, loc.clone());
                        cot_memory::ops::build_store(ctx, *block, val, field_ptr, loc.clone());
                    }
                }

                ptr // Return pointer to struct
            }
            ExprKind::FieldAccess => {
                let field_name = &expr.name;
                let lhs_expr = expr.lhs.as_ref().unwrap();

                // Check if this is an enum constant (e.g., Color.Red)
                if lhs_expr.kind == ExprKind::Ident {
                    if let Some(variants) = self.enum_defs.get(&lhs_expr.name) {
                        if let Some(idx) = variants.iter().position(|v| v == field_name) {
                            let i32_ty = ctx.integer_type(32);
                            return cot_arith::ops::build_constant_int(
                                ctx, *block, i32_ty, idx as i64, loc,
                            );
                        }
                    }
                }

                // Check if this is a slice field access (s.len or s.ptr)
                let lhs_val = self.emit_expr(ctx, block, lhs_expr, None);
                let lhs_cir_ty = ctx.value_type(lhs_val);
                if let mlif::TypeKind::Extension(ext) = ctx.type_kind(lhs_cir_ty) {
                    if ext.dialect == "cir" && ext.name == "slice" {
                        let i64_ty = ctx.integer_type(64);
                        return match field_name.as_str() {
                            "len" => cot_slices::ops::build_slice_len(ctx, *block, lhs_val, loc),
                            "ptr" => {
                                let ptr_ty = ctx.extension_type(
                                    mlif::ExtensionType::new("cir", "ptr"));
                                cot_slices::ops::build_slice_ptr(ctx, *block, lhs_val, ptr_ty, loc)
                            }
                            _ => panic!("error: unknown slice field '{}'", field_name),
                        };
                    }
                }

                // Struct field access (e.g., p.x)
                let struct_ptr = lhs_val;

                // Find struct type from lhs expression
                let struct_name = if lhs_expr.kind == ExprKind::Ident {
                    // Look up the variable's type to find struct name
                    self.find_struct_type_for_var(&lhs_expr.name)
                } else {
                    None
                };

                if let Some(sname) = struct_name {
                    let fields_def = self.struct_defs.get(&sname).cloned()
                        .unwrap_or_else(|| panic!("error: unknown struct '{}'", sname));

                    let fidx = fields_def.iter().position(|(n, _)| n == field_name)
                        .unwrap_or_else(|| panic!("error: unknown field '{}' in struct '{}'", field_name, sname));

                    let fty = self.resolve_type(ctx, &fields_def[fidx].1);

                    // Compute field offset
                    let mut offset: u32 = 0;
                    for i in 0..fidx {
                        let ty = self.resolve_type(ctx, &fields_def[i].1);
                        offset += mlif::codegen::types::type_byte_size(ctx, ty);
                    }

                    if offset == 0 {
                        cot_memory::ops::build_load(ctx, *block, struct_ptr, fty, loc)
                    } else {
                        let i64_ty = ctx.integer_type(64);
                        let off_val = cot_arith::ops::build_constant_int(
                            ctx, *block, i64_ty, offset as i64, loc.clone(),
                        );
                        let field_ptr = cot_arith::ops::build_add(ctx, *block, struct_ptr, off_val, loc.clone());
                        cot_memory::ops::build_load(ctx, *block, field_ptr, fty, loc)
                    }
                } else {
                    panic!("error: cannot determine struct type for field access");
                }
            }
            ExprKind::ArrayLit => {
                // [10, 20, 12] — alloca array, store each element
                // If expected_type is an array type, extract the element type.
                let elem_ty = match expected_type {
                    Some(ty) => {
                        match ctx.type_kind(ty) {
                            mlif::TypeKind::Extension(ext)
                                if ext.dialect == "cir" && ext.name == "array" =>
                            {
                                ext.type_params[0]
                            }
                            _ => ty,
                        }
                    }
                    None => ctx.integer_type(32),
                };
                let elem_size = mlif::codegen::types::type_byte_size(ctx, elem_ty);
                let num_elems = expr.args.len() as u32;
                let total_size = elem_size * num_elems;

                let alloca_ty = ctx.integer_type((total_size * 8) as u32);
                let ptr = cot_memory::ops::build_alloca(ctx, *block, alloca_ty, loc.clone());

                let i64_ty = ctx.integer_type(64);
                for (i, arg) in expr.args.iter().enumerate() {
                    let val = self.emit_expr(ctx, block, arg, Some(elem_ty));
                    if i == 0 {
                        cot_memory::ops::build_store(ctx, *block, val, ptr, loc.clone());
                    } else {
                        let off = cot_arith::ops::build_constant_int(
                            ctx, *block, i64_ty, (i as u32 * elem_size) as i64, loc.clone(),
                        );
                        let elem_ptr = cot_arith::ops::build_add(ctx, *block, ptr, off, loc.clone());
                        cot_memory::ops::build_store(ctx, *block, val, elem_ptr, loc.clone());
                    }
                }

                ptr // Return pointer to array
            }
            ExprKind::Index => {
                // s[i] or arr[i]
                let i64_ty = ctx.integer_type(64);
                let i32_ty = ctx.integer_type(32);
                let base = self.emit_expr(ctx, block, expr.lhs.as_ref().unwrap(), None);
                let idx = self.emit_expr(ctx, block, expr.rhs.as_ref().unwrap(), Some(i64_ty));

                // Check if this is a slice index (s[i] → cir.slice_elem)
                let base_ty = ctx.value_type(base);
                if let mlif::TypeKind::Extension(ext) = ctx.type_kind(base_ty) {
                    if ext.dialect == "cir" && ext.name == "slice" {
                        let elem_ty = ext.type_params[0];
                        return cot_slices::ops::build_slice_elem(
                            ctx, *block, base, idx, elem_ty, loc,
                        );
                    }
                }

                // Array index: arr[i] — load from array base + index * elem_size
                let array_ptr = base;
                let result_ty = expected_type.unwrap_or(i32_ty);
                let elem_size = mlif::codegen::types::type_byte_size(ctx, result_ty);

                if elem_size == 1 {
                    let elem_ptr = cot_arith::ops::build_add(ctx, *block, array_ptr, idx, loc.clone());
                    cot_memory::ops::build_load(ctx, *block, elem_ptr, result_ty, loc)
                } else {
                    let size_val = cot_arith::ops::build_constant_int(
                        ctx, *block, i64_ty, elem_size as i64, loc.clone(),
                    );
                    let offset = cot_arith::ops::build_mul(ctx, *block, idx, size_val, loc.clone());
                    let elem_ptr = cot_arith::ops::build_add(ctx, *block, array_ptr, offset, loc.clone());
                    cot_memory::ops::build_load(ctx, *block, elem_ptr, result_ty, loc)
                }
            }
            ExprKind::ForceUnwrap => {
                // a! → optional_payload(a)
                let inner = self.emit_expr(ctx, block, expr.lhs.as_ref().unwrap(), None);
                let result_ty = expected_type.unwrap_or_else(|| ctx.integer_type(32));
                cot_optionals::ops::build_optional_payload(ctx, *block, inner, result_ty, loc)
            }
            ExprKind::TryUnwrap => {
                // try expr → evaluate expr, if is_error return the error, else extract payload
                let err_val = self.emit_expr(ctx, block, expr.lhs.as_ref().unwrap(), None);
                let is_err = cot_errors::ops::build_is_error(ctx, *block, err_val, loc.clone());

                let ok_block = self.create_block(ctx);
                let err_block = self.create_block(ctx);
                cot_flow::ops::build_condbr(ctx, *block, is_err, err_block, ok_block, loc.clone());

                // err_block: propagate the error — return the error union as-is
                Builder::at_end(ctx, err_block).build_return(&[err_val], loc.clone());

                // ok_block: extract payload and continue here
                let result_ty = expected_type.unwrap_or_else(|| ctx.integer_type(32));
                let payload = cot_errors::ops::build_error_payload(
                    ctx, ok_block, err_val, result_ty, loc,
                );
                // Update current block — subsequent code emits into ok_block
                *block = ok_block;
                payload
            }
            ExprKind::AddrOf => {
                // &x — return the alloca pointer directly
                let inner = expr.lhs.as_ref().unwrap();
                if inner.kind == ExprKind::Ident {
                    if let Some(&(alloca, _)) = self.locals.get(&inner.name) {
                        return alloca;
                    }
                }
                panic!("error: addr_of requires an lvalue identifier");
            }
            ExprKind::Deref => {
                // *p — load through the pointer
                let ptr = self.emit_expr(ctx, block, expr.lhs.as_ref().unwrap(), None);
                let result_ty = expected_type.unwrap_or_else(|| ctx.integer_type(32));
                cot_memory::ops::build_load(ctx, *block, ptr, result_ty, loc)
            }
            ExprKind::CastAs => {
                let input = self.emit_expr(ctx, block, expr.lhs.as_ref().unwrap(), None);
                let input_ty = ctx.value_type(input);
                let target_ty = self.resolve_type(ctx, &expr.cast_type);
                let input_is_float = ctx.is_float_type(input_ty);
                let target_is_float = ctx.is_float_type(target_ty);

                if input_is_float && target_is_float {
                    let input_w = ctx.float_type_width(input_ty).unwrap_or(0);
                    let target_w = ctx.float_type_width(target_ty).unwrap_or(0);
                    if target_w > input_w {
                        cot_arith::ops::build_extf(ctx, *block, input, target_ty, loc)
                    } else if target_w < input_w {
                        cot_arith::ops::build_truncf(ctx, *block, input, target_ty, loc)
                    } else {
                        input
                    }
                } else if input_is_float {
                    cot_arith::ops::build_fptosi(ctx, *block, input, target_ty, loc)
                } else if target_is_float {
                    cot_arith::ops::build_sitofp(ctx, *block, input, target_ty, loc)
                } else {
                    let input_w = ctx.integer_type_width(input_ty).unwrap_or(0);
                    let target_w = ctx.integer_type_width(target_ty).unwrap_or(0);
                    if target_w > input_w {
                        cot_arith::ops::build_extsi(ctx, *block, input, target_ty, loc)
                    } else if target_w < input_w {
                        cot_arith::ops::build_trunci(ctx, *block, input, target_ty, loc)
                    } else {
                        input
                    }
                }
            }
            ExprKind::SliceFrom => {
                // arr[lo..hi] → cir.array_to_slice
                let array_ptr = self.emit_expr(ctx, block, expr.lhs.as_ref().unwrap(), None);
                let array_cir_ty = ctx.value_type(array_ptr);

                // Determine element type from the array type.
                let elem_ty = match ctx.type_kind(array_cir_ty) {
                    mlif::TypeKind::Extension(ext)
                        if ext.dialect == "cir" && ext.name == "array" =>
                    {
                        ext.type_params[0]
                    }
                    _ => ctx.integer_type(32),
                };

                let slice_ty = cot_slices::types::slice_type(ctx, elem_ty);
                let i64_ty = ctx.integer_type(64);
                let lo = self.emit_expr(ctx, block, &expr.args[0], Some(i64_ty));
                let hi = self.emit_expr(ctx, block, expr.rhs.as_ref().unwrap(), Some(i64_ty));
                cot_slices::ops::build_array_to_slice(
                    ctx, *block, array_ptr, lo, hi, slice_ty, loc,
                )
            }
        }
    }

    fn emit_binop(
        &mut self,
        ctx: &mut Context,
        block: &mut BlockId,
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
                return cot_arith::ops::build_cmpf(ctx, *block, pred, lhs, rhs, loc);
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
            return cot_arith::ops::build_cmp(ctx, *block, pred, lhs, rhs, loc);
        }

        // orelse: a orelse b → is_non_null(a) ? optional_payload(a) : b
        if expr.op == TokenKind::Orelse {
            let lhs = self.emit_expr(ctx, block, lhs_expr, None);
            let is_some = cot_optionals::ops::build_is_non_null(ctx, *block, lhs, loc.clone());
            let result_ty = expected_type.unwrap_or_else(|| ctx.integer_type(32));
            let payload =
                cot_optionals::ops::build_optional_payload(ctx, *block, lhs, result_ty, loc.clone());
            let rhs = self.emit_expr(ctx, block, rhs_expr, Some(result_ty));
            return cot_arith::ops::build_select(ctx, *block, is_some, payload, rhs, loc);
        }

        // catch: result catch default → is_error(result) ? default : error_payload(result)
        if expr.op == TokenKind::Catch {
            let lhs = self.emit_expr(ctx, block, lhs_expr, None);
            let is_err = cot_errors::ops::build_is_error(ctx, *block, lhs, loc.clone());
            let result_ty = expected_type.unwrap_or_else(|| ctx.integer_type(32));
            let payload =
                cot_errors::ops::build_error_payload(ctx, *block, lhs, result_ty, loc.clone());
            let rhs = self.emit_expr(ctx, block, rhs_expr, Some(result_ty));
            return cot_arith::ops::build_select(ctx, *block, is_err, rhs, payload, loc);
        }

        // Logical ops
        if expr.op == TokenKind::AmpAmp {
            let lhs = self.emit_expr(ctx, block, lhs_expr, None);
            let rhs = self.emit_expr(ctx, block, rhs_expr, None);
            let f = cot_arith::ops::build_constant_bool(ctx, *block, false, loc.clone());
            return cot_arith::ops::build_select(ctx, *block, lhs, rhs, f, loc);
        }
        if expr.op == TokenKind::PipePipe {
            let lhs = self.emit_expr(ctx, block, lhs_expr, None);
            let rhs = self.emit_expr(ctx, block, rhs_expr, None);
            let t = cot_arith::ops::build_constant_bool(ctx, *block, true, loc.clone());
            return cot_arith::ops::build_select(ctx, *block, lhs, t, rhs, loc);
        }

        // Arithmetic
        let lhs = self.emit_expr(ctx, block, lhs_expr, expected_type);
        let lhs_type = ctx.value_type(lhs);
        let rhs = self.emit_expr(ctx, block, rhs_expr, Some(lhs_type));

        match expr.op {
            TokenKind::Plus => cot_arith::ops::build_add(ctx, *block, lhs, rhs, loc),
            TokenKind::Minus => cot_arith::ops::build_sub(ctx, *block, lhs, rhs, loc),
            TokenKind::Star => cot_arith::ops::build_mul(ctx, *block, lhs, rhs, loc),
            TokenKind::Slash => cot_arith::ops::build_divsi(ctx, *block, lhs, rhs, loc),
            TokenKind::Percent => cot_arith::ops::build_remsi(ctx, *block, lhs, rhs, loc),
            TokenKind::Amp => cot_arith::ops::build_bit_and(ctx, *block, lhs, rhs, loc),
            TokenKind::Pipe => cot_arith::ops::build_bit_or(ctx, *block, lhs, rhs, loc),
            TokenKind::Caret => cot_arith::ops::build_bit_xor(ctx, *block, lhs, rhs, loc),
            TokenKind::Shl => cot_arith::ops::build_shl(ctx, *block, lhs, rhs, loc),
            TokenKind::Shr => cot_arith::ops::build_shr(ctx, *block, lhs, rhs, loc),
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

    fn build_registry() -> mlif::LoweringRegistry {
        let mut registry = mlif::LoweringRegistry::new();
        registry.register(Box::new(cot_arith::lowering::ArithLowering));
        registry.register(Box::new(cot_memory::lowering::MemoryLowering));
        registry.register(Box::new(cot_flow::lowering::FlowLowering));
        registry.register(Box::new(cot_optionals::lowering::OptionalsLowering));
        registry.register(Box::new(cot_errors::lowering::ErrorsLowering));
        registry.register(Box::new(cot_structs::lowering::StructsLowering));
        registry.register(Box::new(cot_arrays::lowering::ArraysLowering));
        registry.register(Box::new(cot_enums::lowering::EnumsLowering));
        registry.register(Box::new(cot_slices::lowering::SlicesLowering));
        registry.register(Box::new(cot_test::lowering::TestLowering));
        registry.register(Box::new(cot_unions::lowering::UnionsLowering));
        registry
    }

    fn compile_and_run(src: &str, test_name: &str) -> i32 {
        let (ctx, module) = build_module(src);
        let registry = build_registry();
        let bytes = mlif::codegen::lower_module(&ctx, module.op(), Some(&registry))
            .expect(&format!("{}: lowering failed", test_name));

        let tmp = std::env::temp_dir();
        let obj = tmp.join(format!("cot_test_{}.o", test_name));
        let exe = tmp.join(format!("cot_test_{}", test_name));

        mlif::codegen::write_object_file(&bytes, obj.to_str().unwrap())
            .expect(&format!("{}: write failed", test_name));
        mlif::codegen::link_executable(obj.to_str().unwrap(), exe.to_str().unwrap())
            .expect(&format!("{}: link failed", test_name));

        let status = std::process::Command::new(exe.to_str().unwrap())
            .status()
            .expect(&format!("{}: execute failed", test_name));

        let _ = std::fs::remove_file(&obj);
        let _ = std::fs::remove_file(&exe);

        status.code().unwrap_or(-1)
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
        let registry = build_registry();
        let bytes = mlif::codegen::lower_module(&ctx, module.op(), Some(&registry))
            .expect("lowering should succeed");
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_codegen_arithmetic_lower() {
        let src = "fn add(a: i32, b: i32) -> i32 {\n    return a + b\n}\n\nfn main() -> i32 {\n    return add(19, 23)\n}\n";
        let (ctx, module) = build_module(src);
        let registry = build_registry();
        let bytes = mlif::codegen::lower_module(&ctx, module.op(), Some(&registry))
            .expect("lowering should succeed");
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_gate1_hello() {
        assert_eq!(compile_and_run("fn main() -> i32 {\n    return 42\n}\n", "gate1_hello"), 42);
    }

    #[test]
    fn test_gate2_arithmetic() {
        let src = "fn add(a: i32, b: i32) -> i32 {\n    return a + b\n}\n\nfn main() -> i32 {\n    return add(19, 23)\n}\n";
        assert_eq!(compile_and_run(src, "gate2_arith"), 42);
    }

    #[test]
    fn test_variables() {
        let src = "fn main() -> i32 {\n    let x: i32 = 10\n    var y: i32 = 20\n    y = y + 12\n    return x + y\n}\n";
        assert_eq!(compile_and_run(src, "variables"), 42);
    }

    #[test]
    fn test_ifelse() {
        let src = "fn abs(x: i32) -> i32 {\n    if x < 0 {\n        return -x\n    } else {\n        return x\n    }\n}\n\nfn main() -> i32 {\n    return abs(-42)\n}\n";
        assert_eq!(compile_and_run(src, "ifelse"), 42);
    }

    #[test]
    fn test_while_loop() {
        let src = "fn main() -> i32 {\n    var sum: i32 = 0\n    var i: i32 = 1\n    while i <= 9 {\n        sum += i\n        i += 1\n    }\n    return sum\n}\n";
        assert_eq!(compile_and_run(src, "while_loop"), 45);
    }

    #[test]
    fn test_for_loop() {
        let src = "test \"basic for\" {\n  var sum: i32 = 0\n  for i in 0..10 {\n    sum += i\n  }\n  assert(sum == 45)\n}\n";
        assert_eq!(compile_and_run(src, "for_loop"), 42);
    }

    #[test]
    fn test_for_break() {
        let src = "test \"for break\" {\n  var sum: i32 = 0\n  for i in 0..100 {\n    if i == 5 {\n      break\n    }\n    sum += i\n  }\n  assert(sum == 10)\n}\n";
        assert_eq!(compile_and_run(src, "for_break"), 42);
    }

    #[test]
    fn test_for_continue() {
        let src = "test \"for continue\" {\n  var sum: i32 = 0\n  for i in 0..10 {\n    if i == 3 {\n      continue\n    }\n    sum += i\n  }\n  assert(sum == 42)\n}\n";
        assert_eq!(compile_and_run(src, "for_continue"), 42);
    }

    #[test]
    fn test_casts() {
        let src = "test \"int widening\" {\n  let x: i8 = 42\n  let y: i32 = x as i32\n  assert(y == 42)\n}\n";
        assert_eq!(compile_and_run(src, "casts"), 42);
    }

    #[test]
    fn test_pointers() {
        let src = "test \"addr and deref\" {\n  var x: i32 = 10\n  let p: *i32 = &x\n  let val: i32 = *p\n  assert(val == 10)\n}\n";
        assert_eq!(compile_and_run(src, "pointers"), 42);
    }

    #[test]
    fn test_enums() {
        let src = "enum Color {\n  Red\n  Green\n  Blue\n}\n\nfn color_value(c: Color) -> i32 {\n  let tag: i32 = c as i32\n  return tag\n}\n\ntest \"enum constants\" {\n  let r: Color = Color.Red\n  let b: Color = Color.Blue\n  assert(color_value(r) == 0)\n  assert(color_value(b) == 2)\n}\n";
        assert_eq!(compile_and_run(src, "enums"), 42);
    }

    #[test]
    fn test_match() {
        let src = "enum Dir {\n  North\n  South\n  West\n}\n\nfn score(d: Dir) -> i32 {\n  var result: i32 = 0\n  match d {\n    North => { result = 10 }\n    South => { result = 20 }\n    West => { result = 42 }\n  }\n  return result\n}\n\ntest \"match west\" {\n  assert(score(Dir.West) == 42)\n}\n";
        assert_eq!(compile_and_run(src, "match_stmt"), 42);
    }

    #[test]
    fn test_structs() {
        let src = "struct Point {\n  x: i32\n  y: i32\n}\n\nfn make_point(x: i32, y: i32) -> Point {\n  return Point { x: x, y: y }\n}\n\nfn sum_point(p: Point) -> i32 {\n  return p.x + p.y\n}\n\nfn main() -> i32 {\n  let p = make_point(20, 22)\n  return sum_point(p)\n}\n";
        assert_eq!(compile_and_run(src, "structs"), 42);
    }

    #[test]
    fn test_optional_wrap_unwrap() {
        let src = "test \"wrap\" {\n  let a: ?i32 = 42\n  let val: i32 = a!\n  assert(val == 42)\n}\n";
        assert_eq!(compile_and_run(src, "opt_wrap"), 42);
    }

    #[test]
    fn test_optional_null_orelse() {
        let src = "test \"null\" {\n  let b: ?i32 = null\n  let c: i32 = b orelse 10\n  assert(c == 10)\n}\n";
        assert_eq!(compile_and_run(src, "opt_null"), 42);
    }

    #[test]
    fn test_error_success() {
        let src = "fn divide(a: i32, b: i32) -> i32!error {\n  if b == 0 {\n    return error(1)\n  }\n  return a / b\n}\n\ntest \"ok\" {\n  let r: i32!error = divide(10, 2)\n  let v: i32 = r catch 0\n  assert(v == 5)\n}\n";
        assert_eq!(compile_and_run(src, "err_ok"), 42);
    }

    #[test]
    fn test_error_div_zero() {
        let src = "fn divide(a: i32, b: i32) -> i32!error {\n  if b == 0 {\n    return error(1)\n  }\n  return a / b\n}\n\ntest \"err\" {\n  let r: i32!error = divide(10, 0)\n  let v: i32 = r catch -1\n  assert(v == -1)\n}\n";
        assert_eq!(compile_and_run(src, "err_fail"), 42);
    }

    #[test]
    fn test_arrays() {
        let src = "fn sum3(a: i32, b: i32, c: i32) -> i32 {\n  return a + b + c\n}\n\nfn main() -> i32 {\n  let arr = [10, 20, 12]\n  let first = arr[0]\n  let second = arr[1]\n  let third = arr[2]\n  return sum3(first, second, third)\n}\n";
        assert_eq!(compile_and_run(src, "arrays"), 42);
    }
}
