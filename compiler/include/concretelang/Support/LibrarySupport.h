// Part of the Concrete Compiler Project, under the BSD3 License with Zama
// Exceptions. See
// https://github.com/zama-ai/concrete-compiler-internal/blob/master/LICENSE.txt
// for license information.

#ifndef CONCRETELANG_SUPPORT_LIBRARY_SUPPORT
#define CONCRETELANG_SUPPORT_LIBRARY_SUPPORT

#include <mlir/Dialect/LLVMIR/LLVMDialect.h>
#include <mlir/ExecutionEngine/ExecutionEngine.h>
#include <mlir/ExecutionEngine/OptUtils.h>
#include <mlir/Target/LLVMIR/Dialect/LLVMIR/LLVMToLLVMIRTranslation.h>

#include <concretelang/ServerLib/ServerLambda.h>
#include <concretelang/Support/CompilerEngine.h>
#include <concretelang/Support/Jit.h>
#include <concretelang/Support/LambdaSupport.h>

namespace mlir {
namespace concretelang {

namespace clientlib = ::concretelang::clientlib;
namespace serverlib = ::concretelang::serverlib;

/// LibraryCompilationResult is the result of a compilation to a library.
struct LibraryCompilationResult {
  /// The output path where the compilation artifact has been generated.
  std::string libraryPath;
  std::string funcName;
};

class LibrarySupport
    : public LambdaSupport<serverlib::ServerLambda, LibraryCompilationResult> {

public:
  LibrarySupport(std::string outputPath, std::string runtimeLibraryPath = "")
      : outputPath(outputPath), runtimeLibraryPath(runtimeLibraryPath) {}

  llvm::Expected<std::unique_ptr<LibraryCompilationResult>>
  compile(llvm::SourceMgr &program, CompilationOptions options) override {
    // Setup the compiler engine
    auto context = CompilationContext::createShared();
    concretelang::CompilerEngine engine(context);
    engine.setCompilationOptions(options);

    // Compile to a library
    auto library = engine.compile(program, outputPath, runtimeLibraryPath);
    if (auto err = library.takeError()) {
      return std::move(err);
    }

    if (!options.clientParametersFuncName.hasValue()) {
      return StreamStringError("Need to have a funcname to compile library");
    }

    auto result = std::make_unique<LibraryCompilationResult>();
    result->libraryPath = outputPath;
    result->funcName = *options.clientParametersFuncName;
    return std::move(result);
  }
  using LambdaSupport::compile;

  /// Load the server lambda from the compilation result.
  llvm::Expected<serverlib::ServerLambda>
  loadServerLambda(LibraryCompilationResult &result) override {
    auto lambda =
        serverlib::ServerLambda::load(result.funcName, result.libraryPath);
    if (lambda.has_error()) {
      return StreamStringError(lambda.error().mesg);
    }
    return lambda.value();
  }

  /// Load the client parameters from the compilation result.
  llvm::Expected<clientlib::ClientParameters>
  loadClientParameters(LibraryCompilationResult &result) override {
    auto path = ClientParameters::getClientParametersPath(result.libraryPath);
    auto params = ClientParameters::load(path);
    if (params.has_error()) {
      return StreamStringError(params.error().mesg);
    }
    auto param = llvm::find_if(params.value(), [&](ClientParameters param) {
      return param.functionName == result.funcName;
    });
    if (param == params.value().end()) {
      return StreamStringError("ClientLambda: cannot find function(")
             << result.funcName << ") in client parameters path(" << path
             << ")";
    }
    return *param;
  }

  /// Call the lambda with the public arguments.
  llvm::Expected<std::unique_ptr<clientlib::PublicResult>>
  serverCall(serverlib::ServerLambda lambda,
             clientlib::PublicArguments &args) override {
    return lambda.call(args);
  }

private:
  std::string outputPath;
  std::string runtimeLibraryPath;
};

} // namespace concretelang
} // namespace mlir

#endif
