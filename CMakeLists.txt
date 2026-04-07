cmake_minimum_required(VERSION 3.20)
project(cot VERSION 0.1.0 LANGUAGES CXX C)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

find_package(MLIR REQUIRED CONFIG)
find_package(LLVM REQUIRED CONFIG)

list(APPEND CMAKE_MODULE_PATH "${MLIR_CMAKE_DIR}")
list(APPEND CMAKE_MODULE_PATH "${LLVM_CMAKE_DIR}")

include(TableGen)
include(AddLLVM)
include(AddMLIR)
include(HandleLLVMOptions)

include_directories(${LLVM_INCLUDE_DIRS})
include_directories(${MLIR_INCLUDE_DIRS})
include_directories(${PROJECT_SOURCE_DIR}/src)

# COT framework
set(COT_SOURCE_DIR "${PROJECT_SOURCE_DIR}/../cot-dev" CACHE PATH
    "Path to cot-dev framework source")
set(COT_BUILD_DIR "${COT_SOURCE_DIR}/build" CACHE PATH
    "Path to cot framework build")

include_directories(${COT_SOURCE_DIR}/include)
include_directories(${COT_BUILD_DIR}/include)

# Core constructs monorepo
set(CORE_SOURCE_DIR "${PROJECT_SOURCE_DIR}/../core" CACHE PATH
    "Path to core constructs source")
set(CORE_BUILD_DIR "${CORE_SOURCE_DIR}/build" CACHE PATH
    "Path to core constructs build")

# Add include paths for all core constructs
set(CORE_CONSTRUCTS arith memory flow structs arrays slices optionals errors test enums)
foreach(construct ${CORE_CONSTRUCTS})
  include_directories("${CORE_SOURCE_DIR}/${construct}/include")
  include_directories("${CORE_BUILD_DIR}/${construct}/include")
endforeach()

add_definitions(${LLVM_DEFINITIONS})

# The cot compiler
add_llvm_executable(cot
  src/main.cpp
  src/Scanner.cpp
  src/Parser.cpp
  src/Codegen.cpp
)

target_link_libraries(cot PRIVATE
  MLIRIR
  MLIRFuncDialect
  MLIRLLVMDialect
  MLIRArithDialect
  MLIRPass
  MLIRTransforms
  MLIRTransformUtils
  MLIRLLVMCommonConversion
  MLIRArithToLLVM
  MLIRFuncToLLVM
  MLIRReconcileUnrealizedCasts
  MLIRCastInterfaces
  MLIRInferTypeOpInterface
  MLIRControlFlowInterfaces
  MLIRSideEffectInterfaces
  MLIRDLTIDialect
  MLIRTargetLLVMIRExport
  MLIRToLLVMIRTranslationRegistration
  MLIRBuiltinToLLVMIRTranslation
  MLIRLLVMToLLVMIRTranslation
)

# Import framework libraries
add_library(CIR STATIC IMPORTED)
set_target_properties(CIR PROPERTIES
  IMPORTED_LOCATION "${COT_BUILD_DIR}/lib/CIR/libCIR.a")
add_library(COTConstruct STATIC IMPORTED)
set_target_properties(COTConstruct PROPERTIES
  IMPORTED_LOCATION "${COT_BUILD_DIR}/lib/Construct/libCOTConstruct.a")
add_library(COTPipeline STATIC IMPORTED)
set_target_properties(COTPipeline PROPERTIES
  IMPORTED_LOCATION "${COT_BUILD_DIR}/lib/Pipeline/libCOTPipeline.a")

target_link_libraries(cot PRIVATE CIR COTConstruct COTPipeline)

# Force-load construct libraries (prevent dead-stripping of static ctors)
set(CORE_FORCE_LOAD_LIBS
  "arith:CotArith"
  "memory:CotMemory"
  "flow:CotFlow"
  "structs:CotStructs"
  "arrays:CotArrays"
  "slices:CotSlices"
  "optionals:CotOptionals"
  "errors:CotErrors"
  "test:CotTest"
  "enums:CotEnums"
)

foreach(entry ${CORE_FORCE_LOAD_LIBS})
  string(REPLACE ":" ";" parts ${entry})
  list(GET parts 0 dir_name)
  list(GET parts 1 lib_name)
  set(lib_path "${CORE_BUILD_DIR}/${dir_name}/lib/lib${lib_name}.a")
  if(APPLE)
    target_link_options(cot PRIVATE "-Wl,-force_load,${lib_path}")
  else()
    target_link_options(cot PRIVATE
      "-Wl,--whole-archive" "${lib_path}" "-Wl,--no-whole-archive")
  endif()
endforeach()

# Bake runtime library path for `cot test` command
target_compile_definitions(cot PRIVATE
    CIR_TEST_RUNTIME_PATH="${CORE_BUILD_DIR}/test/runtime/libcir_test.a")

llvm_map_components_to_libnames(LLVM_LIBS
  core support target
  aarch64info aarch64desc aarch64codegen aarch64asmparser
  x86info x86desc x86codegen x86asmparser
)
target_link_libraries(cot PRIVATE ${LLVM_LIBS})
