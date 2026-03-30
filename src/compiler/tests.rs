mod mock;
mod stateless;

use std::collections::{BTreeMap, HashMap, HashSet};

use proptest::prelude::*;
use typst::diag::SourceDiagnostic;
use typst::syntax::FileId;

use crate::compiler::{CompileDiagnostics, Compiler, OutputPlan, ProcessDiagnostics};
use crate::config::{
    BACKMATTER_TEMPLATE, LINK_TEMPLATE, NODE_TEMPLATE, RenderConfig, TRANSCLUSION_TEMPLATE,
};

use self::mock::{Event, EventConfig, MockFile, arbitrary_event_batches, arbitrary_events};
use self::stateless::process_stateless;

#[derive(Debug, Default, Clone)]
struct ProjectState {
    order: Vec<FileId>,
    files: HashMap<FileId, MockFile>,
}

impl ProjectState {
    fn apply_event(&mut self, event: &Event) {
        match event {
            Event::Create(raw_id, file) | Event::Replace(raw_id, file) => {
                let id = FileId::from_raw(*raw_id);
                self.files.insert(id, file.clone());
                self.normalize_file(id);
                self.touch(id);
            }
            Event::Update(raw_id, mutation) => {
                let id = FileId::from_raw(*raw_id);
                let file = self
                    .files
                    .get_mut(&id)
                    .expect("bug: generated update event targeted a missing file");
                file.apply_mutation(mutation);
                self.normalize_file(id);
                self.touch(id);
            }
            Event::Remove(raw_id) => {
                let id = FileId::from_raw(*raw_id);
                self.files.remove(&id);
                self.order.retain(|existing| *existing != id);
            }
        }
    }

    fn get(&self, id: FileId) -> Option<&MockFile> {
        self.files.get(&id)
    }

    fn ordered_files(&self) -> Vec<(FileId, MockFile)> {
        self.order
            .iter()
            .filter_map(|id| self.files.get(id).cloned().map(|file| (*id, file)))
            .collect()
    }

    fn touch(&mut self, id: FileId) {
        self.order.retain(|existing| *existing != id);
        self.order.push(id);
    }

    fn normalize_file(&mut self, id: FileId) {
        let reserved = self
            .files
            .iter()
            .filter(|(other_id, _)| **other_id != id)
            .flat_map(|(_, file)| node_identifiers(file))
            .collect::<HashSet<_>>();

        if let Some(file) = self.files.get_mut(&id) {
            file.normalize_identifiers_against(&reserved);
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Observation {
    fs: HashMap<String, String>,
    compile_diagnostics: BTreeMap<u16, (Vec<String>, Vec<String>)>,
    process_diagnostics: BTreeMap<u16, Vec<String>>,
}

proptest! {
    #[test]
    fn compile_scratch_equal_compile_incremental_batched(
        batches in arbitrary_event_batches(&EventConfig::from_environment())
    ) {
        let config = render_config();

        let scratch = {
            let mut project = ProjectState::default();
            for event in batches.iter().flatten() {
                project.apply_event(event);
            }
            observe_scratch(&project, &config)
        };

        let incremental = {
            let mut project = ProjectState::default();
            let mut compiler = Compiler::default();
            let mut fs = HashMap::new();

            for batch in &batches {
                for event in batch {
                    apply_event_to_compiler(event, &mut project, &mut compiler);
                }
                apply(compiler.process(&config).unwrap(), &mut fs);
            }

            observe_incremental(&compiler, fs)
        };

        prop_assert_eq!(scratch, incremental);
    }

    #[test]
    fn compile_scratch_equal_compile_incremental(
        events in arbitrary_events(&EventConfig::from_environment())
    ) {
        let config = render_config();

        let scratch = {
            let mut project = ProjectState::default();
            for event in &events {
                project.apply_event(event);
            }
            observe_scratch(&project, &config)
        };

        let incremental = {
            let mut project = ProjectState::default();
            let mut compiler = Compiler::default();
            let mut fs = HashMap::new();

            for event in &events {
                apply_event_to_compiler(event, &mut project, &mut compiler);
                apply(compiler.process(&config).unwrap(), &mut fs);
            }

            observe_incremental(&compiler, fs)
        };

        prop_assert_eq!(scratch, incremental);
    }

    #[test]
    fn output_plan_writes_and_deletes_are_disjoint(
        events in arbitrary_events(&EventConfig::from_environment())
    ) {
        let config = render_config();
        let mut project = ProjectState::default();
        let mut compiler = Compiler::default();

        for event in &events {
            apply_event_to_compiler(event, &mut project, &mut compiler);
        }

        let plan = compiler.process(&config).unwrap();
        let writes: HashSet<&str> = plan.writes.keys().map(String::as_str).collect();
        let deletes: HashSet<&str> = plan.deletes.iter().map(String::as_str).collect();

        prop_assert!(writes.is_disjoint(&deletes));
    }

    #[test]
    fn process_is_idempotent(events in arbitrary_events(&EventConfig::from_environment())) {
        let config = render_config();
        let mut project = ProjectState::default();
        let mut compiler = Compiler::default();

        for event in &events {
            apply_event_to_compiler(event, &mut project, &mut compiler);
        }
        compiler.process(&config).unwrap();

        let second = compiler.process(&config).unwrap();
        prop_assert!(second.writes.is_empty(), "expected no writes on second process call, got {:?}", second.writes.keys().collect::<Vec<_>>());
        prop_assert!(second.deletes.is_empty(), "expected no deletes on second process call, got {:?}", second.deletes);
    }

    #[test]
    fn compile_incremental_matches_reference(
        events in arbitrary_events(&EventConfig::from_environment())
    ) {
        let config = render_config();
        let mut project = ProjectState::default();
        let mut incremental = Compiler::default();
        let mut incremental_fs = HashMap::new();

        for event in &events {
            apply_event_to_compiler(event, &mut project, &mut incremental);
            apply(incremental.process(&config).unwrap(), &mut incremental_fs);

            let incremental_observation = observe_incremental(&incremental, incremental_fs.clone());
            let reference_observation = observe_reference(&project, &config);
            prop_assert_eq!(incremental_observation, reference_observation);
        }
    }
}

fn apply_event_to_compiler(event: &Event, project: &mut ProjectState, compiler: &mut Compiler) {
    match event {
        Event::Create(raw_id, _) | Event::Update(raw_id, _) | Event::Replace(raw_id, _) => {
            let id = FileId::from_raw(*raw_id);
            project.apply_event(event);
            let file = project
                .get(id)
                .expect("bug: project state lost file immediately after create/update/replace");
            compiler.update(file, id);
        }
        Event::Remove(raw_id) => {
            let id = FileId::from_raw(*raw_id);
            project.apply_event(event);
            compiler.remove(id);
        }
    }
}

fn observe_scratch(project: &ProjectState, config: &RenderConfig) -> Observation {
    let mut compiler = Compiler::default();
    let mut fs = HashMap::new();

    for (id, file) in project.ordered_files() {
        compiler.update(&file, id);
    }
    apply(compiler.process(config).unwrap(), &mut fs);

    observe_incremental(&compiler, fs)
}

fn observe_reference(project: &ProjectState, config: &RenderConfig) -> Observation {
    let (fs, compile_diagnostics, process_diagnostics) =
        process_stateless(&project.ordered_files(), config).unwrap();

    Observation {
        fs,
        compile_diagnostics: canonical_compile_diagnostics(&compile_diagnostics),
        process_diagnostics: canonical_process_diagnostics(&process_diagnostics),
    }
}

fn observe_incremental(compiler: &Compiler, fs: HashMap<String, String>) -> Observation {
    Observation {
        fs,
        compile_diagnostics: canonical_compile_diagnostics(compiler.compile_diagnostics()),
        process_diagnostics: canonical_process_diagnostics(compiler.process_diagnostics()),
    }
}

fn canonical_compile_diagnostics(
    diagnostics: &CompileDiagnostics,
) -> BTreeMap<u16, (Vec<String>, Vec<String>)> {
    diagnostics
        .iter()
        .map(|(file_id, (warnings, errors))| {
            let mut warnings = warnings
                .iter()
                .map(canonical_diagnostic)
                .collect::<Vec<_>>();
            warnings.sort();

            let mut errors = errors.iter().map(canonical_diagnostic).collect::<Vec<_>>();
            errors.sort();

            (file_id.into_raw().get(), (warnings, errors))
        })
        .collect()
}

fn canonical_process_diagnostics(diagnostics: &ProcessDiagnostics) -> BTreeMap<u16, Vec<String>> {
    diagnostics
        .iter()
        .map(|(file_id, diagnostics)| {
            let mut diagnostics = diagnostics
                .iter()
                .map(canonical_diagnostic)
                .collect::<Vec<_>>();
            diagnostics.sort();
            (file_id.into_raw().get(), diagnostics)
        })
        .collect()
}

fn canonical_diagnostic(diagnostic: &SourceDiagnostic) -> String {
    let mut message = diagnostic.message.to_string();
    if let Some(names) = message.strip_prefix("transclusion cycle: ") {
        let mut names = names.split(", ").collect::<Vec<_>>();
        names.sort();
        message = format!("transclusion cycle: {}", names.join(", "));
    }

    format!("{:?}: {message}", diagnostic.severity)
}

fn node_identifiers(file: &MockFile) -> impl Iterator<Item = String> + '_ {
    std::iter::once(file.node.identifier.clone()).chain(
        file.subnodes
            .iter()
            .map(|subnode| subnode.identifier.clone()),
    )
}

fn apply(plan: OutputPlan, fs: &mut HashMap<String, String>) {
    for (k, v) in plan.writes {
        fs.insert(k, v);
    }
    for k in plan.deletes {
        fs.remove(&k);
    }
}

fn render_config() -> RenderConfig {
    let node_template = concat!(
        "<node-title>{{ node.title_text }}</node-title>",
        "<node-tag>{{ node.metadata[\"tag\"] | join(\",\") }}</node-tag>",
        "{{ node.body | safe }}",
        "{{ node.backmatter | safe }}",
    )
    .to_string();
    let transclusion_template = concat!(
        "<transclude-tag>{{ transclusion.transclusion_metadata[\"tag\"] | join(\",\") }}</transclude-tag>",
        "{%- if transclusion.resolved -%}",
        "<transclude-title>{{ transclusion.title_text }}</transclude-title>",
        "<transclude-node-tag>{{ transclusion.metadata[\"tag\"] | join(\",\") }}</transclude-node-tag>",
        "{{ transclusion.body | safe }}",
        "{%- else -%}",
        r#"<wb-missing identifier="{{ transclusion.identifier }}"></wb-missing>"#,
        "{%- endif -%}",
    )
    .to_string();
    let link_template = concat!(
        "<link-tag>{{ link.link_metadata[\"tag\"] | join(\",\") }}</link-tag>",
        "{%- if link.resolved -%}",
        "<link-title>{{ link.title_text }}</link-title>",
        "<link-node-tag>{{ link.metadata[\"tag\"] | join(\",\") }}</link-node-tag>",
        r#"<span class="link local"><a href="{{ link.href }}">"#,
        "{%- if link.content %}{{ link.content | safe }}{%- else %}{{ link.title | safe }}{%- endif %}",
        "</a></span>",
        "{%- else -%}",
        r#"<span class="link local"><a href="{{ link.href }}">{{ link.content | safe }}</a></span>"#,
        "{%- endif -%}",
    )
    .to_string();
    let backmatter_template = concat!(
        "{%- for n in backmatter.contexts -%}",
        "<ctx>{{ n.id }}|{{ n.title_text }}</ctx>",
        "{%- endfor -%}",
        "{%- for n in backmatter.backlinks -%}",
        "<bl>{{ n.id }}|{{ n.title_text }}</bl>",
        "{%- endfor -%}",
        "{%- for n in backmatter.outlinks -%}",
        "<ol>{{ n.id }}|{{ n.title_text }}</ol>",
        "{%- endfor -%}",
    )
    .to_string();

    let mut environment = minijinja::Environment::new();
    environment
        .add_template_owned(NODE_TEMPLATE, node_template)
        .unwrap();
    environment
        .add_template_owned(TRANSCLUSION_TEMPLATE, transclusion_template)
        .unwrap();
    environment
        .add_template_owned(LINK_TEMPLATE, link_template)
        .unwrap();
    environment
        .add_template_owned(BACKMATTER_TEMPLATE, backmatter_template)
        .unwrap();

    RenderConfig {
        root_directory: "/".to_string(),
        trailing_slash: false,
        index_node: "index".to_string(),
        domain: String::new(),
        environment,
    }
}
