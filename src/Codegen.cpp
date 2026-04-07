//===- Codegen.cpp - ac AST to CIR emission -------------------*- C++ -*-===//
//
// Zig AstGen pattern: single-pass recursive dispatch over AST nodes.
// Emits CIR ops from cot-core, cot-memory, cot-flow, cot-structs, cot-arrays.
//
//===----------------------------------------------------------------------===//
#include "Codegen.h"

#include "arith/Ops.h"
#include "memory/Types.h"
#include "memory/Ops.h"
#include "flow/Ops.h"
#include "structs/Types.h"
#include "structs/Ops.h"
#include "arrays/Types.h"
#include "arrays/Ops.h"
#include "slices/Types.h"
#include "slices/Ops.h"
#include "optionals/Types.h"
#include "optionals/Ops.h"
#include "errors/Types.h"
#include "errors/Ops.h"
#include "test/Ops.h"

#include "mlir/Dialect/Func/IR/FuncOps.h"
#include "mlir/IR/Builders.h"
#include "mlir/IR/BuiltinOps.h"

#include "llvm/ADT/StringSwitch.h"

using namespace mlir;

namespace ac {

class CodeGenImpl {
  OpBuilder b;
  Location loc;
  StringRef source_;
  std::string filename_;
  ModuleOp module_;

  // Scope: variable name → alloca address + value type
  llvm::DenseMap<llvm::StringRef, std::pair<Value, Type>> locals_;
  // Parameters: name → SSA value (direct)
  llvm::DenseMap<llvm::StringRef, Value> params_;
  // Struct type registry: name → CIR StructType
  llvm::StringMap<cir::StructType> structTypes_;
  // Current function return type (for try propagation)
  Type funcReturnType_;
  // Loop control flow targets (for break/continue)
  Block *loopExitBlock_ = nullptr;
  Block *loopHeaderBlock_ = nullptr;

  std::pair<unsigned, unsigned> lineCol(size_t offset) const {
    unsigned line = 1, col = 1;
    for (size_t i = 0; i < offset && i < source_.size(); i++) {
      if (source_[i] == '\n') { line++; col = 1; }
      else { col++; }
    }
    return {line, col};
  }

  Location locFrom(size_t offset) {
    auto [line, col] = lineCol(offset);
    return FileLineColLoc::get(b.getContext(), StringRef(filename_),
                               line, col);
  }

  Type resolveType(const TypeRef &t) {
    // Slice type: []T
    if (t.isSlice) {
      TypeRef elemRef{t.name, 0, false, false, false, false, false};
      auto elemType = resolveType(elemRef);
      return cir::SliceType::get(b.getContext(), elemType);
    }

    // Pointer type: *T
    if (t.isPointer) {
      // All pointers are opaque in CIR (!cir.ptr)
      return cir::PointerType::get(b.getContext());
    }

    // Error union type: T!error
    if (t.isErrorUnion) {
      TypeRef inner{t.name, t.arrayLen, t.isArray, t.isOptional, false, false, false};
      auto innerType = resolveType(inner);
      return cir::ErrorUnionType::get(b.getContext(), innerType);
    }

    // Optional type: ?T
    if (t.isOptional) {
      TypeRef inner{t.name, t.arrayLen, t.isArray, false, false, false, false};
      auto innerType = resolveType(inner);
      return cir::OptionalType::get(b.getContext(), innerType);
    }

    // Array type: [N]T
    if (t.isArray) {
      TypeRef elemRef{t.name, 0, false, false, false, false, false};
      auto elemType = resolveType(elemRef);
      return cir::ArrayType::get(b.getContext(), t.arrayLen, elemType);
    }

    // Primitive types
    auto prim = llvm::StringSwitch<Type>(t.name)
        .Case("i8", b.getIntegerType(8))
        .Case("i16", b.getIntegerType(16))
        .Case("i32", b.getIntegerType(32))
        .Case("i64", b.getIntegerType(64))
        .Case("u8", b.getIntegerType(8))
        .Case("u16", b.getIntegerType(16))
        .Case("u32", b.getIntegerType(32))
        .Case("u64", b.getIntegerType(64))
        .Case("f32", b.getF32Type())
        .Case("f64", b.getF64Type())
        .Case("bool", b.getI1Type())
        .Default(Type());
    if (prim) return prim;

    // Struct type lookup
    auto it = structTypes_.find(t.name);
    if (it != structTypes_.end())
      return it->second;

    llvm::errs() << "error: unknown type '" << t.name << "'\n";
    return Type();
  }

  // Look up field index in a struct type by name
  int64_t getFieldIndex(cir::StructType sty, llvm::StringRef fieldName) {
    auto names = sty.getFieldNames();
    for (unsigned i = 0; i < names.size(); i++) {
      if (names[i].getValue() == fieldName)
        return i;
    }
    llvm::errs() << "error: no field '" << fieldName << "' in struct '"
                  << sty.getName().getValue() << "'\n";
    return -1;
  }

  Value emitExpr(const Expr &e, Type expectedType = Type()) {
    loc = locFrom(e.pos);

    // If expected type is optional and expr is not null, unwrap the
    // expected type so the inner expression gets the right type context.
    // The wrapping is handled at the call site (Let/Var).
    if (expectedType && mlir::isa<cir::OptionalType>(expectedType) &&
        e.kind != ExprKind::NullLit) {
      expectedType = mlir::cast<cir::OptionalType>(expectedType)
                         .getPayloadType();
    }

    // Same for error unions: unwrap expected type for non-error expressions.
    // error(code) is a Call with name "error" — keep the error union type.
    bool isErrorCall = (e.kind == ExprKind::Call && e.name == "error");
    if (expectedType && mlir::isa<cir::ErrorUnionType>(expectedType) &&
        !isErrorCall) {
      expectedType = mlir::cast<cir::ErrorUnionType>(expectedType)
                         .getPayloadType();
    }

    switch (e.kind) {
    case ExprKind::IntLit: {
      auto type = expectedType ? expectedType : b.getI32Type();
      auto attr = b.getIntegerAttr(type, e.intVal);
      return b.create<cir::ConstantOp>(loc, type, attr);
    }
    case ExprKind::FloatLit: {
      auto type = expectedType ? expectedType : b.getF64Type();
      auto attr = b.getFloatAttr(type, e.floatVal);
      return b.create<cir::ConstantOp>(loc, type, attr);
    }
    case ExprKind::BoolLit: {
      auto type = b.getI1Type();
      auto attr = b.getBoolAttr(e.boolVal);
      return b.create<cir::ConstantOp>(loc, type, attr);
    }
    case ExprKind::StringLit: {
      // Emit cir.string_constant → !cir.slice<i8>
      auto sliceType = cir::SliceType::get(b.getContext(), b.getIntegerType(8));
      return b.create<cir::StringConstantOp>(loc, sliceType, e.strVal);
    }
    case ExprKind::NullLit: {
      // null → cir.none : !cir.optional<T> (needs expected type)
      if (!expectedType || !mlir::isa<cir::OptionalType>(expectedType)) {
        llvm::errs() << "error: null requires optional type context\n";
        return Value();
      }
      return b.create<cir::NoneOp>(loc, expectedType);
    }
    case ExprKind::ForceUnwrap: {
      // expr! → cir.optional_payload
      auto val = emitExpr(*e.lhs);
      auto optType = mlir::dyn_cast<cir::OptionalType>(val.getType());
      if (!optType) {
        llvm::errs() << "error: force unwrap on non-optional type\n";
        return val;
      }
      return b.create<cir::OptionalPayloadOp>(loc, optType.getPayloadType(),
                                               val);
    }
    case ExprKind::Ident: {
      auto pit = params_.find(e.name);
      if (pit != params_.end())
        return pit->second;
      auto lit = locals_.find(e.name);
      if (lit != locals_.end()) {
        auto [addr, type] = lit->second;
        return b.create<cir::LoadOp>(loc, type, addr);
      }
      llvm::errs() << "error: undefined variable '" << e.name << "'\n";
      return Value();
    }
    case ExprKind::Call: {
      // error(code) → cir.wrap_error
      if (e.name == "error") {
        if (e.args.size() != 1) {
          llvm::errs() << "error: error() takes exactly 1 argument\n";
          return Value();
        }
        auto code = emitExpr(*e.args[0], b.getIntegerType(16));
        if (!expectedType || !mlir::isa<cir::ErrorUnionType>(expectedType)) {
          llvm::errs() << "error: error() requires error union type context\n";
          return Value();
        }
        return b.create<cir::WrapErrorOp>(loc, expectedType, code);
      }

      auto callee = module_.lookupSymbol<func::FuncOp>(e.name);
      if (!callee) {
        llvm::errs() << "error: undefined function '" << e.name << "'\n";
        return Value();
      }
      SmallVector<Value> args;
      auto paramTypes = callee.getFunctionType().getInputs();
      for (unsigned i = 0; i < e.args.size(); i++)
        args.push_back(emitExpr(*e.args[i],
                                i < paramTypes.size() ? paramTypes[i]
                                                      : Type()));
      auto callOp = b.create<func::CallOp>(loc, callee, args);
      return callOp.getNumResults() > 0 ? callOp.getResult(0) : Value();
    }
    case ExprKind::UnaryOp: {
      auto val = emitExpr(*e.rhs, expectedType);
      if (e.op == TokenKind::Minus)
        return b.create<cir::NegOp>(loc, val.getType(), val);
      if (e.op == TokenKind::Tilde)
        return b.create<cir::BitNotOp>(loc, val.getType(), val);
      if (e.op == TokenKind::Bang) {
        auto one = b.create<cir::ConstantOp>(
            loc, b.getI1Type(), b.getBoolAttr(true));
        return b.create<cir::BitXorOp>(loc, b.getI1Type(), val, one);
      }
      return val;
    }
    case ExprKind::StructLit: {
      // Look up the struct type
      auto it = structTypes_.find(e.name);
      if (it == structTypes_.end()) {
        llvm::errs() << "error: unknown struct '" << e.name << "'\n";
        return Value();
      }
      auto sty = it->second;
      auto fieldTypes = sty.getFieldTypes();

      // Emit field values in struct field order
      SmallVector<Value> fieldValues(fieldTypes.size());
      for (auto &fi : e.fields) {
        int64_t idx = getFieldIndex(sty, fi.name);
        if (idx < 0) return Value();
        fieldValues[idx] = emitExpr(*fi.value, fieldTypes[idx]);
      }

      return b.create<cir::StructInitOp>(loc, sty, fieldValues);
    }
    case ExprKind::FieldAccess: {
      // Emit the base expression
      auto base = emitExpr(*e.lhs);
      auto baseType = base.getType();

      // Slice field access: .len, .ptr
      if (auto sliceTy = mlir::dyn_cast<cir::SliceType>(baseType)) {
        if (e.name == "len")
          return b.create<cir::SliceLenOp>(loc, b.getI64Type(), base);
        if (e.name == "ptr") {
          auto ptrType = cir::PointerType::get(b.getContext());
          return b.create<cir::SlicePtrOp>(loc, ptrType, base);
        }
        llvm::errs() << "error: unknown slice field '" << e.name << "'\n";
        return Value();
      }

      if (auto sty = mlir::dyn_cast<cir::StructType>(baseType)) {
        int64_t idx = getFieldIndex(sty, e.name);
        if (idx < 0) return Value();
        auto fieldType = sty.getFieldTypes()[idx];
        return b.create<cir::FieldValOp>(loc, fieldType, base, idx);
      }

      llvm::errs() << "error: field access on non-struct type\n";
      return Value();
    }
    case ExprKind::ArrayLit: {
      // Emit all elements
      SmallVector<Value> elems;
      for (auto &arg : e.args)
        elems.push_back(emitExpr(*arg, expectedType ? Type() : Type()));

      if (elems.empty()) {
        llvm::errs() << "error: empty array literal\n";
        return Value();
      }

      // Infer array type from element type and count
      auto elemType = elems[0].getType();
      auto arrayType = cir::ArrayType::get(
          b.getContext(), elems.size(), elemType);
      return b.create<cir::ArrayInitOp>(loc, arrayType, elems);
    }
    case ExprKind::Index: {
      auto base = emitExpr(*e.lhs);
      auto baseType = base.getType();

      // Slice element access: s[i] → cir.slice_elem
      if (auto sliceTy = mlir::dyn_cast<cir::SliceType>(baseType)) {
        auto elemType = sliceTy.getElementType();
        auto idx = emitExpr(*e.rhs, b.getI64Type());
        return b.create<cir::SliceElemOp>(loc, elemType, base, idx);
      }

      if (auto arrTy = mlir::dyn_cast<cir::ArrayType>(baseType)) {
        auto elemType = arrTy.getElementType();
        // Constant index: use elem_val (extractvalue)
        if (e.rhs->kind == ExprKind::IntLit) {
          return b.create<cir::ElemValOp>(loc, elemType, base,
                                           e.rhs->intVal);
        }
        // Dynamic index: alloca array, GEP, load
        auto ptrType = cir::PointerType::get(b.getContext());
        auto addr = b.create<cir::AllocaOp>(loc, ptrType,
                                              TypeAttr::get(arrTy));
        b.create<cir::StoreOp>(loc, base, addr);
        auto idx = emitExpr(*e.rhs, b.getI64Type());
        auto elemPtr = b.create<cir::ElemPtrOp>(
            loc, ptrType, addr, idx, TypeAttr::get(arrTy));
        return b.create<cir::LoadOp>(loc, elemType, elemPtr);
      }

      llvm::errs() << "error: index on non-array type\n";
      return Value();
    }
    case ExprKind::SliceFrom: {
      // arr[lo..hi] → alloca array, get ptr, array_to_slice(ptr, lo, hi)
      auto base = emitExpr(*e.lhs);
      auto baseType = base.getType();

      if (auto arrTy = mlir::dyn_cast<cir::ArrayType>(baseType)) {
        auto elemType = arrTy.getElementType();
        auto ptrType = cir::PointerType::get(b.getContext());
        auto sliceType = cir::SliceType::get(b.getContext(), elemType);

        // Alloca the array to get a stable pointer
        auto addr = b.create<cir::AllocaOp>(loc, ptrType,
                                              TypeAttr::get(arrTy));
        b.create<cir::StoreOp>(loc, base, addr);

        auto lo = emitExpr(*e.args[0], b.getI64Type());
        auto hi = emitExpr(*e.rhs, b.getI64Type());
        return b.create<cir::ArrayToSliceOp>(loc, sliceType, addr, lo, hi);
      }

      llvm::errs() << "error: slice from non-array type\n";
      return Value();
    }
    case ExprKind::CastAs: {
      auto val = emitExpr(*e.lhs);
      auto srcType = val.getType();
      auto dstType = resolveType(e.castType);

      bool srcIsInt = mlir::isa<IntegerType>(srcType);
      bool dstIsInt = mlir::isa<IntegerType>(dstType);
      bool srcIsFloat = mlir::isa<FloatType>(srcType);
      bool dstIsFloat = mlir::isa<FloatType>(dstType);
      unsigned srcWidth = srcType.getIntOrFloatBitWidth();
      unsigned dstWidth = dstType.getIntOrFloatBitWidth();

      if (srcIsInt && dstIsInt) {
        if (dstWidth > srcWidth)
          return b.create<cir::ExtSIOp>(loc, dstType, val);
        if (dstWidth < srcWidth)
          return b.create<cir::TruncIOp>(loc, dstType, val);
        return val; // same width
      }
      if (srcIsFloat && dstIsFloat) {
        if (dstWidth > srcWidth)
          return b.create<cir::ExtFOp>(loc, dstType, val);
        if (dstWidth < srcWidth)
          return b.create<cir::TruncFOp>(loc, dstType, val);
        return val;
      }
      if (srcIsInt && dstIsFloat)
        return b.create<cir::SIToFPOp>(loc, dstType, val);
      if (srcIsFloat && dstIsInt)
        return b.create<cir::FPToSIOp>(loc, dstType, val);

      llvm::errs() << "error: unsupported cast\n";
      return val;
    }
    case ExprKind::AddrOf: {
      // &x → get the alloca address of local variable x
      if (e.lhs->kind != ExprKind::Ident) {
        llvm::errs() << "error: address-of requires a variable\n";
        return Value();
      }
      auto it = locals_.find(e.lhs->name);
      if (it == locals_.end()) {
        llvm::errs() << "error: undefined variable '" << e.lhs->name << "'\n";
        return Value();
      }
      return it->second.first; // return the alloca address
    }
    case ExprKind::Deref: {
      // *p → cir.load through pointer
      auto ptr = emitExpr(*e.lhs);
      if (!mlir::isa<cir::PointerType>(ptr.getType())) {
        llvm::errs() << "error: dereference of non-pointer type\n";
        return ptr;
      }
      if (!expectedType) {
        llvm::errs() << "error: dereference requires type context\n";
        return ptr;
      }
      return b.create<cir::LoadOp>(loc, expectedType, ptr);
    }
    case ExprKind::TryUnwrap: {
      // try expr → unwrap error union or propagate error to caller
      auto val = emitExpr(*e.lhs);
      auto euType = mlir::dyn_cast<cir::ErrorUnionType>(val.getType());
      if (!euType) {
        llvm::errs() << "error: try on non-error-union type\n";
        return val;
      }
      auto fnRetEU = mlir::dyn_cast_or_null<cir::ErrorUnionType>(
          funcReturnType_);
      if (!fnRetEU) {
        llvm::errs() << "error: try requires function returning error union\n";
        return val;
      }

      auto isErr = b.create<cir::IsErrorOp>(loc, b.getI1Type(), val);

      auto *parentFunc = b.getInsertionBlock()->getParentOp();
      auto &region = parentFunc->getRegion(0);
      auto *errBlock = new Block();
      auto *okBlock = new Block();

      b.create<cir::CondBrOp>(loc, isErr, ValueRange{}, ValueRange{},
                               errBlock, okBlock);

      // Error path: propagate error code
      region.push_back(errBlock);
      b.setInsertionPointToStart(errBlock);
      auto code = b.create<cir::ErrorCodeOp>(loc, b.getIntegerType(16), val);
      auto wrapped = b.create<cir::WrapErrorOp>(loc, funcReturnType_, code);
      b.create<func::ReturnOp>(loc, ValueRange{wrapped});

      // Success path: extract payload
      region.push_back(okBlock);
      b.setInsertionPointToStart(okBlock);
      return b.create<cir::ErrorPayloadOp>(
          loc, euType.getPayloadType(), val);
    }
    case ExprKind::BinOp: {
      // catch: expr catch default → !is_error ? payload : default
      if (e.op == TokenKind::Catch) {
        auto lhs = emitExpr(*e.lhs);
        auto euType = mlir::dyn_cast<cir::ErrorUnionType>(lhs.getType());
        if (!euType) {
          llvm::errs() << "error: catch on non-error-union type\n";
          return lhs;
        }
        auto isErr = b.create<cir::IsErrorOp>(loc, b.getI1Type(), lhs);
        // Negate: is_ok = !is_error
        auto one = b.create<cir::ConstantOp>(
            loc, b.getI1Type(), b.getBoolAttr(true));
        auto isOk = b.create<cir::BitXorOp>(loc, b.getI1Type(), isErr, one);
        auto payload = b.create<cir::ErrorPayloadOp>(
            loc, euType.getPayloadType(), lhs);
        auto rhs = emitExpr(*e.rhs, euType.getPayloadType());
        return b.create<cir::SelectOp>(
            loc, euType.getPayloadType(), isOk, payload, rhs);
      }

      // orelse: expr orelse default → is_non_null ? payload : default
      if (e.op == TokenKind::Orelse) {
        auto lhs = emitExpr(*e.lhs);
        auto optType = mlir::dyn_cast<cir::OptionalType>(lhs.getType());
        if (!optType) {
          llvm::errs() << "error: orelse on non-optional type\n";
          return lhs;
        }
        auto isNN = b.create<cir::IsNonNullOp>(loc, b.getI1Type(), lhs);
        auto payload = b.create<cir::OptionalPayloadOp>(
            loc, optType.getPayloadType(), lhs);
        auto rhs = emitExpr(*e.rhs, optType.getPayloadType());
        return b.create<cir::SelectOp>(
            loc, optType.getPayloadType(), isNN, payload, rhs);
      }

      // Null comparisons: expr == null, expr != null → is_non_null
      if ((e.op == TokenKind::EqEq || e.op == TokenKind::BangEq) &&
          (e.rhs->kind == ExprKind::NullLit ||
           e.lhs->kind == ExprKind::NullLit)) {
        // Determine which side is the optional
        auto &optExpr = (e.lhs->kind == ExprKind::NullLit) ? *e.rhs : *e.lhs;
        auto val = emitExpr(optExpr);
        auto isNN = b.create<cir::IsNonNullOp>(loc, b.getI1Type(), val);
        // == null → NOT is_non_null; != null → is_non_null
        if (e.op == TokenKind::EqEq) {
          auto one = b.create<cir::ConstantOp>(
              loc, b.getI1Type(), b.getBoolAttr(true));
          return b.create<cir::BitXorOp>(loc, b.getI1Type(), isNN, one);
        }
        return isNN;
      }

      // Comparison ops produce i1
      if (e.op == TokenKind::EqEq || e.op == TokenKind::BangEq ||
          e.op == TokenKind::Less || e.op == TokenKind::LessEq ||
          e.op == TokenKind::Greater || e.op == TokenKind::GreaterEq) {
        auto lhs = emitExpr(*e.lhs);
        auto rhs = emitExpr(*e.rhs, lhs.getType());

        // Float comparisons use CmpFOp
        if (mlir::isa<FloatType>(lhs.getType())) {
          auto pred = [&]() -> cir::CmpFPredicate {
            switch (e.op) {
            case TokenKind::EqEq:      return cir::CmpFPredicate::oeq;
            case TokenKind::BangEq:    return cir::CmpFPredicate::one;
            case TokenKind::Less:      return cir::CmpFPredicate::olt;
            case TokenKind::LessEq:    return cir::CmpFPredicate::ole;
            case TokenKind::Greater:   return cir::CmpFPredicate::ogt;
            case TokenKind::GreaterEq: return cir::CmpFPredicate::oge;
            default: return cir::CmpFPredicate::oeq;
            }
          }();
          auto predAttr = cir::CmpFPredicateAttr::get(b.getContext(), pred);
          return b.create<cir::CmpFOp>(loc, b.getI1Type(), predAttr, lhs, rhs);
        }

        // Integer comparisons
        auto pred = [&]() -> cir::CmpIPredicate {
          switch (e.op) {
          case TokenKind::EqEq:      return cir::CmpIPredicate::eq;
          case TokenKind::BangEq:    return cir::CmpIPredicate::ne;
          case TokenKind::Less:      return cir::CmpIPredicate::slt;
          case TokenKind::LessEq:    return cir::CmpIPredicate::sle;
          case TokenKind::Greater:   return cir::CmpIPredicate::sgt;
          case TokenKind::GreaterEq: return cir::CmpIPredicate::sge;
          default: return cir::CmpIPredicate::eq;
          }
        }();
        auto predAttr = cir::CmpIPredicateAttr::get(b.getContext(), pred);
        return b.create<cir::CmpOp>(loc, b.getI1Type(), predAttr, lhs, rhs);
      }

      // Logical ops: short-circuit via cir.select
      if (e.op == TokenKind::AmpAmp) {
        auto lhs = emitExpr(*e.lhs);
        auto rhs = emitExpr(*e.rhs);
        auto f = b.create<cir::ConstantOp>(
            loc, b.getI1Type(), b.getBoolAttr(false));
        return b.create<cir::SelectOp>(loc, b.getI1Type(), lhs, rhs, f);
      }
      if (e.op == TokenKind::PipePipe) {
        auto lhs = emitExpr(*e.lhs);
        auto rhs = emitExpr(*e.rhs);
        auto t = b.create<cir::ConstantOp>(
            loc, b.getI1Type(), b.getBoolAttr(true));
        return b.create<cir::SelectOp>(loc, b.getI1Type(), lhs, t, rhs);
      }

      // Arithmetic
      auto lhs = emitExpr(*e.lhs, expectedType);
      auto rhs = emitExpr(*e.rhs, lhs.getType());
      auto type = lhs.getType();

      switch (e.op) {
      case TokenKind::Plus:    return b.create<cir::AddOp>(loc, type, lhs, rhs);
      case TokenKind::Minus:   return b.create<cir::SubOp>(loc, type, lhs, rhs);
      case TokenKind::Star:    return b.create<cir::MulOp>(loc, type, lhs, rhs);
      case TokenKind::Slash:   return b.create<cir::DivSIOp>(loc, type, lhs, rhs);
      case TokenKind::Percent: return b.create<cir::RemSIOp>(loc, type, lhs, rhs);
      case TokenKind::Amp:     return b.create<cir::BitAndOp>(loc, type, lhs, rhs);
      case TokenKind::Pipe:    return b.create<cir::BitOrOp>(loc, type, lhs, rhs);
      case TokenKind::Caret:   return b.create<cir::BitXorOp>(loc, type, lhs, rhs);
      case TokenKind::Shl:     return b.create<cir::ShlOp>(loc, type, lhs, rhs);
      case TokenKind::Shr:     return b.create<cir::ShrOp>(loc, type, lhs, rhs);
      default:
        llvm::errs() << "error: unsupported binary op\n";
        return lhs;
      }
    }
    }
    return Value();
  }

  void emitStmt(const Stmt &s) {
    loc = locFrom(s.pos);

    switch (s.kind) {
    case StmtKind::Return: {
      if (s.expr) {
        auto val = emitExpr(*s.expr, funcReturnType_);
        // Auto-wrap: returning T from function returning T!error
        if (funcReturnType_ &&
            mlir::isa<cir::ErrorUnionType>(funcReturnType_) &&
            !mlir::isa<cir::ErrorUnionType>(val.getType())) {
          val = b.create<cir::WrapResultOp>(loc, funcReturnType_, val);
        }
        b.create<func::ReturnOp>(loc, ValueRange{val});
      } else {
        b.create<func::ReturnOp>(loc, ValueRange{});
      }
      break;
    }
    case StmtKind::Let:
    case StmtKind::Var: {
      // If type annotation is optional, resolve it and use for context
      Type declaredType;
      if (s.varType.name.size())
        declaredType = resolveType(s.varType);

      auto val = emitExpr(*s.expr, declaredType);
      auto type = val.getType();

      // Implicit wrap: assigning non-optional value to optional variable
      if (declaredType && mlir::isa<cir::OptionalType>(declaredType) &&
          !mlir::isa<cir::OptionalType>(type)) {
        val = b.create<cir::WrapOptionalOp>(loc, declaredType, val);
        type = declaredType;
      }

      // Implicit wrap: assigning T to T!error variable
      if (declaredType && mlir::isa<cir::ErrorUnionType>(declaredType) &&
          !mlir::isa<cir::ErrorUnionType>(type)) {
        val = b.create<cir::WrapResultOp>(loc, declaredType, val);
        type = declaredType;
      }

      auto ptrType = cir::PointerType::get(b.getContext());
      auto addr = b.create<cir::AllocaOp>(loc, ptrType,
                                            TypeAttr::get(type));
      b.create<cir::StoreOp>(loc, val, addr);
      locals_[s.varName] = {addr, type};
      break;
    }
    case StmtKind::Assign: {
      auto it = locals_.find(s.varName);
      if (it == locals_.end()) {
        llvm::errs() << "error: undefined variable '" << s.varName << "'\n";
        break;
      }
      auto val = emitExpr(*s.expr, it->second.second);
      b.create<cir::StoreOp>(loc, val, it->second.first);
      break;
    }
    case StmtKind::CompoundAssign: {
      auto it = locals_.find(s.varName);
      if (it == locals_.end()) {
        llvm::errs() << "error: undefined variable '" << s.varName << "'\n";
        break;
      }
      auto [addr, type] = it->second;
      auto cur = b.create<cir::LoadOp>(loc, type, addr);
      auto rhs = emitExpr(*s.expr, type);
      Value result;
      switch (s.op) {
      case TokenKind::PlusEq:  result = b.create<cir::AddOp>(loc, type, cur, rhs); break;
      case TokenKind::MinusEq: result = b.create<cir::SubOp>(loc, type, cur, rhs); break;
      case TokenKind::StarEq:  result = b.create<cir::MulOp>(loc, type, cur, rhs); break;
      case TokenKind::SlashEq: result = b.create<cir::DivSIOp>(loc, type, cur, rhs); break;
      default: result = cur; break;
      }
      b.create<cir::StoreOp>(loc, result, addr);
      break;
    }
    case StmtKind::If: {
      auto cond = emitExpr(*s.expr);
      auto *parentFunc = b.getInsertionBlock()->getParentOp();
      auto &region = parentFunc->getRegion(0);

      auto *thenBlock = new Block();
      auto *mergeBlock = new Block();
      Block *elseBlock = s.elseBody.empty() ? mergeBlock : new Block();

      b.create<cir::CondBrOp>(loc, cond, ValueRange{}, ValueRange{},
                               thenBlock, elseBlock);

      region.push_back(thenBlock);
      b.setInsertionPointToStart(thenBlock);
      for (auto &st : s.thenBody)
        emitStmt(*st);
      bool thenTerminates = !thenBlock->empty() &&
          b.getInsertionBlock()->back().hasTrait<OpTrait::IsTerminator>();
      if (!thenTerminates)
        b.create<cir::BrOp>(loc, ValueRange{}, mergeBlock);

      bool elseTerminates = false;
      if (!s.elseBody.empty()) {
        region.push_back(elseBlock);
        b.setInsertionPointToStart(elseBlock);
        for (auto &st : s.elseBody)
          emitStmt(*st);
        elseTerminates = !elseBlock->empty() &&
            b.getInsertionBlock()->back().hasTrait<OpTrait::IsTerminator>();
        if (!elseTerminates)
          b.create<cir::BrOp>(loc, ValueRange{}, mergeBlock);
      }

      if (!thenTerminates || !elseTerminates || s.elseBody.empty()) {
        region.push_back(mergeBlock);
        b.setInsertionPointToStart(mergeBlock);
      } else {
        delete mergeBlock;
        auto *deadBlock = new Block();
        region.push_back(deadBlock);
        b.setInsertionPointToStart(deadBlock);
      }
      break;
    }
    case StmtKind::While: {
      auto *parentFunc = b.getInsertionBlock()->getParentOp();
      auto &region = parentFunc->getRegion(0);

      auto *headerBlock = new Block();
      auto *bodyBlock = new Block();
      auto *exitBlock = new Block();

      auto *prevExit = loopExitBlock_;
      auto *prevHeader = loopHeaderBlock_;
      loopExitBlock_ = exitBlock;
      loopHeaderBlock_ = headerBlock;

      b.create<cir::BrOp>(loc, ValueRange{}, headerBlock);

      region.push_back(headerBlock);
      b.setInsertionPointToStart(headerBlock);
      auto cond = emitExpr(*s.expr);
      b.create<cir::CondBrOp>(loc, cond, ValueRange{}, ValueRange{},
                               bodyBlock, exitBlock);

      region.push_back(bodyBlock);
      b.setInsertionPointToStart(bodyBlock);
      for (auto &st : s.thenBody)
        emitStmt(*st);
      if (b.getInsertionBlock()->empty() ||
          !b.getInsertionBlock()->back().hasTrait<OpTrait::IsTerminator>())
        b.create<cir::BrOp>(loc, ValueRange{}, headerBlock);

      loopExitBlock_ = prevExit;
      loopHeaderBlock_ = prevHeader;

      region.push_back(exitBlock);
      b.setInsertionPointToStart(exitBlock);
      break;
    }
    case StmtKind::For: {
      // Desugar: for i in lo..hi { body }
      //   → let i = lo; while i < hi { body; latch: i += 1 → header }
      // continue → latch (not header, to ensure increment)
      auto *parentFunc = b.getInsertionBlock()->getParentOp();
      auto &region = parentFunc->getRegion(0);

      // Initialize loop variable
      auto loVal = emitExpr(*s.expr);
      auto counterType = loVal.getType();
      auto ptrType = cir::PointerType::get(b.getContext());
      auto addr = b.create<cir::AllocaOp>(loc, ptrType,
                                            TypeAttr::get(counterType));
      b.create<cir::StoreOp>(loc, loVal, addr);
      locals_[s.varName] = {addr, counterType};

      auto hiVal = emitExpr(*s.rangeEnd, counterType);

      auto *headerBlock = new Block();
      auto *bodyBlock = new Block();
      auto *latchBlock = new Block();  // increment block
      auto *exitBlock = new Block();

      auto *prevExit = loopExitBlock_;
      auto *prevHeader = loopHeaderBlock_;
      loopExitBlock_ = exitBlock;
      loopHeaderBlock_ = latchBlock;  // continue → latch (increment first)

      b.create<cir::BrOp>(loc, ValueRange{}, headerBlock);

      // Header: i < hi?
      region.push_back(headerBlock);
      b.setInsertionPointToStart(headerBlock);
      auto cur = b.create<cir::LoadOp>(loc, counterType, addr);
      auto predAttr = cir::CmpIPredicateAttr::get(
          b.getContext(), cir::CmpIPredicate::slt);
      auto cond = b.create<cir::CmpOp>(
          loc, b.getI1Type(), predAttr, cur, hiVal);
      b.create<cir::CondBrOp>(loc, cond, ValueRange{}, ValueRange{},
                               bodyBlock, exitBlock);

      // Body
      region.push_back(bodyBlock);
      b.setInsertionPointToStart(bodyBlock);
      for (auto &st : s.thenBody)
        emitStmt(*st);
      if (b.getInsertionBlock()->empty() ||
          !b.getInsertionBlock()->back().hasTrait<OpTrait::IsTerminator>())
        b.create<cir::BrOp>(loc, ValueRange{}, latchBlock);

      // Latch: i += 1 → header
      region.push_back(latchBlock);
      b.setInsertionPointToStart(latchBlock);
      auto curVal = b.create<cir::LoadOp>(loc, counterType, addr);
      auto one = b.create<cir::ConstantOp>(
          loc, counterType, b.getIntegerAttr(counterType, 1));
      auto next = b.create<cir::AddOp>(loc, counterType, curVal, one);
      b.create<cir::StoreOp>(loc, next, addr);
      b.create<cir::BrOp>(loc, ValueRange{}, headerBlock);

      loopExitBlock_ = prevExit;
      loopHeaderBlock_ = prevHeader;

      region.push_back(exitBlock);
      b.setInsertionPointToStart(exitBlock);
      break;
    }
    case StmtKind::Break: {
      if (!loopExitBlock_) {
        llvm::errs() << "error: break outside loop\n";
        break;
      }
      b.create<cir::BrOp>(loc, ValueRange{}, loopExitBlock_);
      break;
    }
    case StmtKind::Continue: {
      if (!loopHeaderBlock_) {
        llvm::errs() << "error: continue outside loop\n";
        break;
      }
      b.create<cir::BrOp>(loc, ValueRange{}, loopHeaderBlock_);
      break;
    }
    case StmtKind::Assert: {
      auto cond = emitExpr(*s.expr);
      // Generate diagnostic message from source location + expression text
      auto [line, col] = lineCol(s.pos);
      std::string msg = filename_ + ":" + std::to_string(line) +
                        ": assertion failed";
      b.create<cir::AssertOp>(loc, cond, b.getStringAttr(msg));
      break;
    }
    case StmtKind::ExprStmt:
      emitExpr(*s.expr);
      break;
    }
  }

  void emitStructDef(const StructDef &sd) {
    SmallVector<StringAttr> fieldNames;
    SmallVector<Type> fieldTypes;
    for (auto &f : sd.fields) {
      fieldNames.push_back(StringAttr::get(b.getContext(), f.name));
      fieldTypes.push_back(resolveType(f.type));
    }
    auto sty = cir::StructType::get(
        b.getContext(), StringAttr::get(b.getContext(), sd.name),
        fieldNames, fieldTypes);
    structTypes_[sd.name] = sty;
  }

  void emitFnDecl(const FnDecl &fn) {
    loc = locFrom(fn.pos);

    SmallVector<Type> paramTypes;
    for (auto &p : fn.params)
      paramTypes.push_back(resolveType(p.type));

    SmallVector<Type> resultTypes;
    auto retType = resolveType(fn.returnType);
    if (retType)
      resultTypes.push_back(retType);

    funcReturnType_ = retType;

    auto funcType = b.getFunctionType(paramTypes, resultTypes);
    auto funcOp = b.create<func::FuncOp>(loc, fn.name, funcType);

    auto *entryBlock = funcOp.addEntryBlock();
    b.setInsertionPointToStart(entryBlock);

    params_.clear();
    locals_.clear();
    for (unsigned i = 0; i < fn.params.size(); i++)
      params_[fn.params[i].name] = entryBlock->getArgument(i);

    for (auto &s : fn.body)
      emitStmt(*s);

    // Ensure every reachable block has a terminator. Remove dead blocks.
    SmallVector<Block *> deadBlocks;
    for (auto &block : funcOp.getBody()) {
      if (block.hasNoPredecessors() && &block != &funcOp.getBody().front()) {
        deadBlocks.push_back(&block);
        continue;
      }
      if (!block.empty() && block.back().hasTrait<OpTrait::IsTerminator>())
        continue;
      b.setInsertionPointToEnd(&block);
      if (resultTypes.empty()) {
        b.create<func::ReturnOp>(loc, ValueRange{});
      } else if (mlir::isa<cir::ErrorUnionType>(resultTypes[0])) {
        // Default return for error union: wrap 0 as result
        auto euType = mlir::cast<cir::ErrorUnionType>(resultTypes[0]);
        auto zero = b.create<cir::ConstantOp>(
            loc, euType.getPayloadType(),
            b.getIntegerAttr(euType.getPayloadType(), 0));
        auto wrapped = b.create<cir::WrapResultOp>(loc, resultTypes[0], zero);
        b.create<func::ReturnOp>(loc, ValueRange{wrapped});
      } else {
        auto zero = b.create<cir::ConstantOp>(
            loc, resultTypes[0], b.getIntegerAttr(resultTypes[0], 0));
        b.create<func::ReturnOp>(loc, ValueRange{zero});
      }
    }
    for (auto *dead : deadBlocks)
      dead->erase();

    b.setInsertionPointToEnd(module_.getBody());
  }

public:
  CodeGenImpl(MLIRContext &ctx, StringRef source, StringRef filename)
      : b(&ctx), loc(b.getUnknownLoc()), source_(source),
        filename_(filename.str()) {
    module_ = ModuleOp::create(loc);
    b.setInsertionPointToEnd(module_.getBody());
  }

  void emitTestDecl(const TestDecl &td) {
    loc = locFrom(td.pos);

    // Create cir.test_case "name" { body }
    auto testOp = b.create<cir::TestCaseOp>(loc, td.name);
    auto *bodyBlock = new Block();
    testOp.getBody().push_back(bodyBlock);
    b.setInsertionPointToStart(bodyBlock);

    // Reset scope for test isolation
    locals_.clear();
    params_.clear();

    for (auto &s : td.body)
      emitStmt(*s);

    // cir.test_case has NoTerminator — no terminator needed
    b.setInsertionPointToEnd(module_.getBody());
  }

  ModuleOp emit(const Module &mod) {
    // Register struct types first (so functions can reference them)
    for (auto &sd : mod.structs)
      emitStructDef(sd);
    for (auto &fn : mod.functions)
      emitFnDecl(fn);
    for (auto &td : mod.tests)
      emitTestDecl(td);
    return module_;
  }
};

OwningOpRef<ModuleOp> codegen(MLIRContext &ctx, StringRef source,
                               const Module &mod, StringRef filename) {
  CodeGenImpl cg(ctx, source, filename);
  return cg.emit(mod);
}

} // namespace ac
