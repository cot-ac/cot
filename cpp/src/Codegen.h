//===- Codegen.h - ac AST to CIR emission ---------------------*- C++ -*-===//
//
// Zig AstGen pattern: single-pass recursive dispatch over AST nodes.
// Reference: Zig lib/std/zig/AstGen.zig
//
//===----------------------------------------------------------------------===//
#ifndef COTAC_CODEGEN_H
#define COTAC_CODEGEN_H

#include "Parser.h"
#include "mlir/IR/BuiltinOps.h"

namespace ac {

mlir::OwningOpRef<mlir::ModuleOp> codegen(mlir::MLIRContext &ctx,
                                           llvm::StringRef source,
                                           const Module &mod,
                                           llvm::StringRef filename);

} // namespace ac

#endif // COTAC_CODEGEN_H
