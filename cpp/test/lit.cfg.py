import os
import lit.formats

config.name = "cot-ac"
config.test_format = lit.formats.ShTest(True)
config.suffixes = ['.ac']
config.test_source_root = os.path.dirname(__file__)

cot_build_dir = os.environ.get('COT_BUILD_DIR',
    os.path.join(os.path.dirname(__file__), '..', 'build'))
config.substitutions.append(
    ('%cot', os.path.join(cot_build_dir, 'cot')))
config.substitutions.append(
    ('%FileCheck', '/opt/homebrew/opt/llvm@20/bin/FileCheck'))
