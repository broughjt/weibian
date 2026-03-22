use proptest::prelude::*;

use crate::compiler::{Compile, CompileOutput};
use ecow::EcoVec;
use typst::diag::{SourceDiagnostic, Warned};
use typst::syntax::FileId;

/// A single mock file: one primary node with edges to other nodes by ID.
#[allow(dead_code)]
struct MockNode {
    id: String,
    title: String,
    transcludes: Vec<String>,
    links: Vec<String>,
}

impl Compile for MockNode {
    fn compile(&self, _id: FileId) -> Warned<Result<CompileOutput, EcoVec<SourceDiagnostic>>> {
        todo!("serialize MockNode to CompileOutput")
    }
}

proptest! {
    #[test]
    fn compile_scratch_equal_compile_incremental(_universe in mock_universe()) {
        todo!("implement scratch vs incremental comparison")
    }
}

fn mock_universe() -> impl Strategy<Value = ()> {
    Just(())
}
