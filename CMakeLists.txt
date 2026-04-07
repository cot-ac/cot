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

# Construct repos
set(COT_CORE_SOURCE_DIR "${PROJECT_SOURCE_DIR}/../cot-core" CACHE PATH "")
set(COT_CORE_BUILD_DIR "${COT_CORE_SOURCE_DIR}/build" CACHE PATH "")
include_directories(${COT_CORE_SOURCE_DIR}/include)
include_directories(${COT_CORE_BUILD_DIR}/include)

set(COT_MEMORY_SOURCE_DIR "${PROJECT_SOURCE_DIR}/../cot-memory" CACHE PATH "")
set(COT_MEMORY_BUILD_DIR "${COT_MEMORY_SOURCE_DIR}/build" CACHE PATH "")
include_directories(${COT_MEMORY_SOURCE_DIR}/include)
include_directories(${COT_MEMORY_BUILD_DIR}/include)

set(COT_FLOW_SOURCE_DIR "${PROJECT_SOURCE_DIR}/../cot-flow" CACHE PATH "")
set(COT_FLOW_BUILD_DIR "${COT_FLOW_SOURCE_DIR}/build" CACHE PATH "")
include_directories(${COT_FLOW_SOURCE_DIR}/include)
include_directories(${COT_FLOW_BUILD_DIR}/include)

set(COT_STRUCTS_SOURCE_DIR "${PROJECT_SOURCE_DIR}/../cot-structs" CACHE PATH "")
set(COT_STRUCTS_BUILD_DIR "${COT_STRUCTS_SOURCE_DIR}/build" CACHE PATH "")
include_directories(${COT_STRUCTS_SOURCE_DIR}/include)
include_directories(${COT_STRUCTS_BUILD_DIR}/include)

set(COT_ARRAYS_SOURCE_DIR "${PROJECT_SOURCE_DIR}/../cot-arrays" CACHE PATH "")
set(COT_ARRAYS_BUILD_DIR "${COT_ARRAYS_SOURCE_DIR}/build" CACHE PATH "")
include_directories(${COT_ARRAYS_SOURCE_DIR}/include)
include_directories(${COT_ARRAYS_BUILD_DIR}/include)

set(COT_SLICES_SOURCE_DIR "${PROJECT_SOURCE_DIR}/../cot-slices" CACHE PATH "")
set(COT_SLICES_BUILD_DIR "${COT_SLICES_SOURCE_DIR}/build" CACHE PATH "")
include_directories(${COT_SLICES_SOURCE_DIR}/include)
include_directories(${COT_SLICES_BUILD_DIR}/include)

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
if(APPLE)
  target_link_options(cot PRIVATE
    "-Wl,-force_load,${COT_CORE_BUILD_DIR}/lib/libCotCore.a"
    "-Wl,-force_load,${COT_MEMORY_BUILD_DIR}/lib/libCotMemory.a"
    "-Wl,-force_load,${COT_FLOW_BUILD_DIR}/lib/libCotFlow.a"
    "-Wl,-force_load,${COT_STRUCTS_BUILD_DIR}/lib/libCotStructs.a"
    "-Wl,-force_load,${COT_ARRAYS_BUILD_DIR}/lib/libCotArrays.a"
    "-Wl,-force_load,${COT_SLICES_BUILD_DIR}/lib/libCotSlices.a")
else()
  target_link_options(cot PRIVATE
    "-Wl,--whole-archive"
    "${COT_CORE_BUILD_DIR}/lib/libCotCore.a"
    "${COT_MEMORY_BUILD_DIR}/lib/libCotMemory.a"
    "${COT_FLOW_BUILD_DIR}/lib/libCotFlow.a"
    "${COT_STRUCTS_BUILD_DIR}/lib/libCotStructs.a"
    "${COT_ARRAYS_BUILD_DIR}/lib/libCotArrays.a"
    "${COT_SLICES_BUILD_DIR}/lib/libCotSlices.a"
    "-Wl,--no-whole-archive")
endif()

llvm_map_components_to_libnames(LLVM_LIBS
  core support target
  aarch64info aarch64desc aarch64codegen aarch64asmparser
  x86info x86desc x86codegen x86asmparser
)
target_link_libraries(cot PRIVATE ${LLVM_LIBS})
