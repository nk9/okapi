use crate::Args;
use camino::Utf8PathBuf;

pub struct Config {
    pub working_directory: Option<Utf8PathBuf>,
    pub max_count: usize,
    pub columns: Option<String>,
    pub exclude: Vec<String>,
    pub ignore_case: bool,
    pub editor: Option<String>,
    pub pattern: Option<String>,
    pub paths: Vec<Utf8PathBuf>,
    pub extra_args: Vec<String>,
    pub file: Option<Utf8PathBuf>,
}

impl From<Args> for Config {
    fn from(args: Args) -> Self {
        Config {
            working_directory: args.working_directory,
            max_count: args.max_count,
            columns: args.columns,
            exclude: args.exclude,
            ignore_case: args.ignore_case,
            editor: args.editor,
            pattern: args.pattern,
            paths: args.paths,
            extra_args: args.extra_args,
            file: args.file,
        }
    }
}
