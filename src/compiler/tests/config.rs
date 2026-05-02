use std::ops::RangeInclusive;
use std::sync::LazyLock;

pub static CONFIG: LazyLock<Config> = LazyLock::new(Config::from_environment);

pub struct Config {
    pub transitions: RangeInclusive<usize>,
    pub create_file_node_max: usize,
    pub create_file_compile_errors_max: usize,
    pub create_file_compile_warnings_max: usize,
    pub metadata_entries_max: usize,
    pub metadata_values_max: usize,
    pub node_transclusions_max: usize,
    pub node_links_max: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            transitions: 1..=20,
            create_file_node_max: 5,
            create_file_compile_errors_max: 3,
            create_file_compile_warnings_max: 3,
            metadata_entries_max: 6,
            metadata_values_max: 4,
            node_transclusions_max: 5,
            node_links_max: 5,
        }
    }
}

impl Config {
    pub fn from_environment() -> Self {
        let default = Self::default();
        Self {
            transitions: environment_size_range(
                "WEIBIAN_PROPTEST_TRANSITIONS",
                default.transitions,
            ),
            create_file_node_max: environment_usize(
                "WEIBIAN_PROPTEST_CREATE_FILE_NODE_MAX",
                default.create_file_node_max,
            ),
            create_file_compile_errors_max: environment_usize(
                "WEIBIAN_PROPTEST_CREATE_FILE_COMPILE_ERRORS_MAX",
                default.create_file_compile_errors_max,
            ),
            create_file_compile_warnings_max: environment_usize(
                "WEIBIAN_PROPTEST_CREATE_FILE_COMPILE_WARNINGS_MAX",
                default.create_file_compile_warnings_max,
            ),
            metadata_entries_max: environment_usize(
                "WEIBIAN_PROPTEST_METADATA_ENTRIES_MAX",
                default.metadata_entries_max,
            ),
            metadata_values_max: environment_usize(
                "WEIBIAN_PROPTEST_METADATA_VALUES_MAX",
                default.metadata_values_max,
            ),
            node_transclusions_max: environment_usize(
                "WEIBIAN_PROPTEST_NODE_TRANSCLUSIONS_MAX",
                default.node_transclusions_max,
            ),
            node_links_max: environment_usize(
                "WEIBIAN_PROPTEST_NODE_LINKS_MAX",
                default.node_links_max,
            ),
        }
    }
}

fn environment_usize(name: &str, default: usize) -> usize {
    match std::env::var(name) {
        Ok(s) => s
            .parse()
            .unwrap_or_else(|_| panic!("{name} must be a non-negative integer, got {s:?}")),
        Err(_) => default,
    }
}

fn environment_size_range(name: &str, default: RangeInclusive<usize>) -> RangeInclusive<usize> {
    match std::env::var(name) {
        Ok(s) => parse_size_range(&s)
            .unwrap_or_else(|| panic!("{name} must be N, N..=M, or N..M, got {s:?}")),
        Err(_) => default,
    }
}

fn parse_size_range(s: &str) -> Option<RangeInclusive<usize>> {
    if let Some((low, high)) = s.split_once("..=") {
        Some(low.parse().ok()?..=high.parse().ok()?)
    } else if let Some((low, high)) = s.split_once("..") {
        let hi: usize = high.parse().ok()?;
        Some(low.parse().ok()?..=hi.checked_sub(1)?)
    } else {
        let n: usize = s.parse().ok()?;
        Some(n..=n)
    }
}
