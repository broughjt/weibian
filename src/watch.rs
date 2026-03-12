pub struct BuildState {
    dependencies: ImportGraph,
    file_store: FileStore<SystemFiles>,
    resources: Resources,
    // TODO: Use BuildConfig if we ever need `root`
    output_directory: PathBuf,
    include: GlobSet,
    exclude: GlobSet,
}

impl BuildState {
    pub fn new(config: BuildConfig) -> Self {
        let dependencies = ImportGraph::default();

        let downloader = SystemDownloader::new(USER_AGENT);
        let packages = SystemPackages::new(downloader);
        let file_loader = SystemFiles::new(FsRoot::new(config.root), packages);
        let file_store = FileStore::new(file_loader);

        let resources = Resources::default();

        Self {
            dependencies,
            file_store,
            resources,
            output_directory: config.output_directory,
            include: config.include,
            exclude: config.exclude,
        }
    }
}
