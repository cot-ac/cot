//===- main.cpp - cotac CLI driver ----------------------------*- C++ -*-===//
//
// The ac language compiler. Uses COT framework for the backend pipeline.
// Reference: DESIGN.md Section 10.1
//
//===----------------------------------------------------------------------===//
#include "Scanner.h"
#include "Parser.h"
#include "Codegen.h"

#include "cot/Pipeline/Pipeline.h"
#include "cot/Construct/Construct.h"
#include "cot/CIR/CIRDialect.h"

#include "mlir/Dialect/Arith/IR/Arith.h"
#include "mlir/Dialect/Func/IR/FuncOps.h"
#include "mlir/Dialect/LLVMIR/LLVMDialect.h"
#include "mlir/IR/MLIRContext.h"

#include "llvm/Support/MemoryBuffer.h"
#include "llvm/Support/raw_ostream.h"

using namespace mlir;

int main(int argc, char **argv) {
  if (argc < 3) {
    llvm::errs() << "Usage: cotac <command> <file> [output]\n"
                 << "Commands: build, emit-cir, emit-llvm\n";
    return 1;
  }

  llvm::StringRef command = argv[1];
  llvm::StringRef inputFile = argv[2];

  // Read source file
  auto bufferOrErr = llvm::MemoryBuffer::getFile(inputFile);
  if (!bufferOrErr) {
    llvm::errs() << "error: cannot open " << inputFile << "\n";
    return 1;
  }
  auto source = (*bufferOrErr)->getBuffer();

  // Initialize MLIR context
  MLIRContext ctx;
  ctx.getOrLoadDialect<cir::CIRDialect>();
  ctx.getOrLoadDialect<func::FuncDialect>();
  ctx.getOrLoadDialect<arith::ArithDialect>();
  ctx.getOrLoadDialect<LLVM::LLVMDialect>();

  // Register construct ops (linked via -force_load)
  for (auto &construct : cot::getConstructRegistry())
    construct->registerOpsAndTypes(ctx);

  // Frontend: source → tokens → AST → CIR
  auto tokens = ac::scanAll(source);
  auto ast = ac::parse(source, tokens);
  auto module = ac::codegen(ctx, source, ast, inputFile);

  if (!module) {
    llvm::errs() << "error: codegen failed\n";
    return 1;
  }

  // Pipeline
  cot::PipelineBuilder pipeline(&ctx);

  if (command == "emit-cir") {
    if (failed(pipeline.runToTypedCIR(*module)))
      return 1;
    module->print(llvm::outs());
    return 0;
  }

  if (command == "emit-llvm") {
    if (failed(pipeline.runToLLVM(*module)))
      return 1;
    module->print(llvm::outs());
    return 0;
  }

  if (command == "build") {
    llvm::StringRef outputPath = (argc > 3) ? argv[3] : "a.out";
    if (failed(pipeline.emitBinary(*module, outputPath)))
      return 1;
    return 0;
  }

  llvm::errs() << "error: unknown command '" << command << "'\n";
  return 1;
}
