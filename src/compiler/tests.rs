use std::borrow::Cow;
use std::collections::HashMap;

use proptest::prelude::*;
use proptest::sample::subsequence;

use crate::compiler::{Compile, CompileOutput};
use ecow::EcoVec;
use typst::diag::{SourceDiagnostic, Warned};
use typst::syntax::{FileId, Span};

/// A single mock file: one primary node with edges to other nodes by ID.
#[derive(Debug)]
struct MockNode {
    id: u32,
    title: String,
    body: String,
    transcludes: Vec<u32>,
    links: Vec<u32>,
}

impl Compile for MockNode {
    fn compile(&self, _id: FileId) -> Warned<Result<CompileOutput, EcoVec<SourceDiagnostic>>> {
        let node_id = format_id(self.id);

        let mut html = format!(r#"<wb-node identifier="{node_id}">"#);
        html.push_str(&format!("<wb-title>{}</wb-title>", self.title));
        html.push_str(&format!("<p>{}</p>", self.body));

        for (counter, &target) in self.transcludes.iter().enumerate() {
            let target_id = format_id(target);
            html.push_str(&format!(
                r#"<wb-transclude identifier="{target_id}" counter="{counter}"></wb-transclude>"#
            ));
        }

        for (counter, &target) in self.links.iter().enumerate() {
            let target_id = format_id(target);
            html.push_str(&format!(
                r#"<a href="wb:{target_id}" data-counter="{counter}">link</a>"#
            ));
        }

        html.push_str("</wb-node>");

        Warned {
            output: Ok(CompileOutput {
                html,
                spans: HashMap::from([(node_id, Span::detached())]),
                // TODO: test metadata propagation
                metadata: HashMap::new(),
                // Counters in the HTML elements above are consumed by extract; these
                // maps are intentionally empty (no per-edge metadata on mock nodes).
                transclusion_metadata: HashMap::new(),
                link_metadata: HashMap::new(),
                // TODO: test error cases
                errors: EcoVec::new(),
            }),
            warnings: EcoVec::new(),
        }
    }
}

fn arbitrary_mock_node(id: u32, nodes: Cow<'static, [u32]>) -> impl Strategy<Value = MockNode> {
    let transcludes = subsequence(nodes.clone(), 0..=3);
    let links = subsequence(nodes.clone(), 0..=3);

    ("[a-z]+", "[a-z]*", transcludes, links).prop_map(move |(title, body, transcludes, links)| {
        MockNode {
            id,
            title,
            body,
            transcludes,
            links,
        }
    })
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

fn format_id(id: u32) -> String {
    format!("n{id}")
}
