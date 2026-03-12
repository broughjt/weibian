use std::{fs, io};

use anyhow::anyhow;
use ecow::eco_format;
use typst::diag::Warned;
use typst_html::HtmlDocument;
use typst_kit::{
    downloader::SystemDownloader,
    files::{FsRoot, SystemFiles},
    packages::SystemPackages,
};
use typst_syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};
use walkdir::WalkDir;

use crate::{
    config::BuildConfig,
    file_store::FileStore,
    world::{Resources, SystemWorld},
};

const USER_AGENT: &str = "weibian";

// TODO: Consider calling builder or something
pub struct BuildState {
    file_store: FileStore<SystemFiles>,
    resources: Resources,
    config: BuildConfig,
}

impl BuildState {
    pub fn new(config: BuildConfig) -> Self {
        let downloader = SystemDownloader::new(USER_AGENT);
        let packages = SystemPackages::new(downloader);
        let file_loader = SystemFiles::new(FsRoot::new(config.root.clone()), packages);
        let file_store = FileStore::new(file_loader);

        let resources = Resources::default();

        Self {
            file_store,
            resources,
            config,
        }
    }

    pub fn build(&self) -> anyhow::Result<()> {
        if self.config.output_directory.exists() {
            fs::remove_dir_all(&self.config.output_directory)?;
        }
        fs::create_dir(&self.config.output_directory)?;

        let paths = WalkDir::new(&self.config.root)
            .into_iter()
            .filter_map(|result| match result {
                Ok(entry) => {
                    if entry.file_type().is_file() && self.config.is_match(entry.path()) {
                        Some(Ok(entry.into_path()))
                    } else {
                        None
                    }
                }
                Err(error) => Some(Err(error)),
            });

        for (counter, result) in (0_u32..).zip(paths) {
            let path = result?;
            let virtual_path = VirtualPath::virtualize(&self.config.root, &path)
                .map_err(|error| anyhow!("Failed to virtualize path: {:?}", error))?;
            let id = FileId::new(RootedPath::new(VirtualRoot::Project, virtual_path));
            let world = SystemWorld::new(id, &self.resources, &self.file_store);

            let Warned {
                output: result,
                warnings: _warnings,
            } = typst::compile::<HtmlDocument>(&world);
            let document = result.unwrap(); // TODO

            let output = typst_html::html(&document).unwrap(); // TODO

            let output_path = self
                .config
                .output_directory
                .join(eco_format!("{counter}.html").as_str());
            fs::write(output_path, output)?;
        }

        Ok(())
    }
}

// pub fn build(
//     world: &SystemWorld,
// ) -> Result<Warned<(HtmlDocument, String)>, EcoVec<SourceDiagnostic>> {
//     let Warned {
//         output: result,
//         warnings,
//     } = typst::compile::<HtmlDocument>(&world);
//     let document = result?;

//     let output = typst_html::html(&document)?;

//     Ok(Warned {
//         output: (document, output),
//         warnings,
//     })
// }
